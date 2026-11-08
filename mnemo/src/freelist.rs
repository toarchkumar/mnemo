//! FMT-9 (a): the quarantined free list.
//!
//! A v9 file reuses pages that no committed state needs any more. Each free
//! run records the transaction that stopped referencing it (`freed_at`):
//! the state at `freed_at - 1` still used those pages, and so may every
//! retained snapshot older than `freed_at`. The allocator may hand a run
//! out only when
//!
//! * `freed_at <= committed_txn` — the freeing transaction is durable, so a
//!   crash can't bring back a state that still references the run (data
//!   pages are written *before* `wal::commit`, so a page freed and reused
//!   inside one transaction would overwrite a page the pre-crash state
//!   needs); and
//! * `freed_at <= min_snapshot_txn` — no retained snapshot references it.
//!
//! The list is persisted as a CoW page run under the header's `free`
//! pointer, written in the freeing transaction's own commit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{MnemoError, Result};

/// One free run: `count` pages from `page`, unreferenced since `freed_at`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FreeRun {
    pub freed_at: u64,
    pub page: u64,
    pub count: u64,
}

/// The free list: non-overlapping runs keyed by first page. Adjacent runs
/// are coalesced (the merged run takes the later `freed_at`, which can
/// only delay reuse, never hasten it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FreeList {
    /// `page -> (count, freed_at)`.
    runs: BTreeMap<u64, (u64, u64)>,
}

impl FreeList {
    /// Rebuild from the persisted form, rejecting overlaps (a corrupt or
    /// hostile list must not make the allocator hand out a page twice).
    pub fn from_runs(runs: Vec<FreeRun>) -> Result<FreeList> {
        let mut fl = FreeList::default();
        for r in runs {
            fl.free(r.freed_at, r.page, r.count)?;
        }
        Ok(fl)
    }

    /// The persisted form, in page order.
    pub fn to_runs(&self) -> Vec<FreeRun> {
        self.runs
            .iter()
            .map(|(&page, &(count, freed_at))| FreeRun {
                freed_at,
                page,
                count,
            })
            .collect()
    }

    /// Number of runs.
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// Total free pages (eligible or not).
    #[cfg(test)]
    pub fn pages(&self) -> u64 {
        self.runs.values().map(|&(c, _)| c).sum()
    }

    /// Free `count` pages from `page`, unreferenced since `freed_at`.
    /// Overlap with a run already on the list is a double free and an
    /// error: it would let two live objects share a page.
    pub fn free(&mut self, freed_at: u64, page: u64, count: u64) -> Result<()> {
        if count == 0 {
            return Ok(());
        }
        if page == 0 {
            return Err(MnemoError::Invalid(
                "free list: page 0 is the header".into(),
            ));
        }
        let end = page
            .checked_add(count)
            .ok_or_else(|| MnemoError::Invalid("free list: run overflows".into()))?;
        // Neighbour before.
        let mut start = page;
        let mut len = count;
        let mut at = freed_at;
        if let Some((&p, &(c, f))) = self.runs.range(..=page).next_back() {
            if p + c > page {
                return Err(MnemoError::Invalid(format!(
                    "free list: double free of page {page} (overlaps run {p}+{c})"
                )));
            }
            if p + c == page {
                self.runs.remove(&p);
                start = p;
                len += c;
                at = at.max(f);
            }
        }
        // Neighbour after.
        if let Some((&p, &(c, f))) = self.runs.range(page..).next() {
            if p < end {
                return Err(MnemoError::Invalid(format!(
                    "free list: double free of page {p} (inside freed run {page}+{count})"
                )));
            }
            if p == end {
                self.runs.remove(&p);
                len += c;
                at = at.max(f);
            }
        }
        self.runs.insert(start, (len, at));
        Ok(())
    }

    /// First-fit allocation of `count` contiguous eligible pages. Returns
    /// the first page, or `None` when no eligible run is long enough (the
    /// caller then extends the file).
    pub fn alloc(&mut self, count: u64, committed_txn: u64, min_snapshot_txn: u64) -> Option<u64> {
        if count == 0 {
            return None;
        }
        let (&page, &(c, f)) = self
            .runs
            .iter()
            .find(|(_, &(c, f))| c >= count && f <= committed_txn && f <= min_snapshot_txn)?;
        self.runs.remove(&page);
        if c > count {
            self.runs.insert(page + count, (c - count, f));
        }
        Some(page)
    }

    /// Take `count` pages from `page` back off the list wherever they
    /// overlap it (a snapshot restore makes them live again). Pages not
    /// on the list are ignored.
    pub fn unfree(&mut self, page: u64, count: u64) {
        if count == 0 {
            return;
        }
        let end = page.saturating_add(count);
        let overlapping: Vec<(u64, u64, u64)> = self
            .runs
            .range(..end)
            .rev()
            .take_while(|(&p, &(c, _))| p + c > page || p >= page)
            .filter(|(&p, &(c, _))| p + c > page && p < end)
            .map(|(&p, &(c, f))| (p, c, f))
            .collect();
        for (p, c, f) in overlapping {
            self.runs.remove(&p);
            if p < page {
                self.runs.insert(p, (page - p, f));
            }
            if p + c > end {
                self.runs.insert(end, (p + c - end, f));
            }
        }
    }

    /// True if any page of `page..page+count` is on the list.
    pub fn overlaps(&self, page: u64, count: u64) -> bool {
        let end = page.saturating_add(count);
        self.runs
            .range(..end)
            .next_back()
            .is_some_and(|(&p, &(c, _))| p + c > page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn coalesces_and_splits() {
        let mut fl = FreeList::default();
        fl.free(5, 10, 2).unwrap();
        fl.free(7, 12, 3).unwrap();
        fl.free(6, 8, 2).unwrap();
        assert_eq!(fl.len(), 1);
        assert_eq!(
            fl.to_runs(),
            vec![FreeRun {
                freed_at: 7,
                page: 8,
                count: 7
            }]
        );
        assert_eq!(fl.alloc(3, 7, u64::MAX), Some(8));
        assert_eq!(fl.pages(), 4);
        fl.unfree(12, 1);
        assert_eq!(fl.pages(), 3);
        assert!(!fl.overlaps(12, 1));
        assert!(fl.overlaps(11, 1));
        assert!(fl.overlaps(13, 2));
    }

    #[test]
    fn quarantine_blocks_uncommitted_and_snapshot_pinned_runs() {
        let mut fl = FreeList::default();
        fl.free(10, 100, 4).unwrap();
        // Freed by the transaction being built (txn 10, committed 9): no.
        assert_eq!(fl.alloc(1, 9, u64::MAX), None);
        // Committed, but a snapshot at txn 8 still references it: no.
        assert_eq!(fl.alloc(1, 10, 8), None);
        // Committed and no older snapshot retained: yes.
        assert_eq!(fl.alloc(1, 10, 10), Some(100));
    }

    #[test]
    fn double_free_is_an_error() {
        let mut fl = FreeList::default();
        fl.free(1, 10, 4).unwrap();
        assert!(fl.free(2, 12, 1).is_err());
        assert!(fl.free(2, 8, 3).is_err());
        assert!(fl.free(2, 13, 5).is_err());
        assert!(fl.free(2, 0, 1).is_err());
        assert!(fl.free(2, 14, 1).is_ok());
        assert!(FreeList::from_runs(vec![
            FreeRun {
                freed_at: 1,
                page: 5,
                count: 3
            },
            FreeRun {
                freed_at: 1,
                page: 6,
                count: 1
            },
        ])
        .is_err());
    }

    #[derive(Debug, Clone)]
    enum Op {
        Alloc(u64),
        FreeOne(usize),
        Commit,
        Snapshot,
        DropSnapshot,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => (1u64..4).prop_map(Op::Alloc),
            3 => any::<usize>().prop_map(Op::FreeOne),
            2 => Just(Op::Commit),
            1 => Just(Op::Snapshot),
            1 => Just(Op::DropSnapshot),
        ]
    }

    proptest! {
        /// FMT-9 (a) property: across random alloc / free / commit /
        /// snapshot sequences, no allocation returns a page that was freed
        /// by a transaction that isn't committed yet or that a retained
        /// snapshot may still reference, and no page is ever handed out
        /// while it's still live.
        #[test]
        fn alloc_never_returns_quarantined_or_live_pages(ops in proptest::collection::vec(op(), 1..200)) {
            let mut fl = FreeList::default();
            let mut next_page = 1u64;
            let mut committed = 0u64;
            // Live runs: (page, count).
            let mut live: Vec<(u64, u64)> = Vec::new();
            // Retained snapshots, by txn id.
            let mut snaps: Vec<u64> = Vec::new();
            // Shadow map: page -> freed_at, for pages on the list.
            let mut freed_at: std::collections::HashMap<u64, u64> = Default::default();
            for op in ops {
                let txn = committed + 1;
                let min_snap = snaps.iter().copied().min().unwrap_or(u64::MAX);
                match op {
                    Op::Alloc(n) => {
                        let page = match fl.alloc(n, committed, min_snap) {
                            Some(p) => {
                                for q in p..p + n {
                                    let f = freed_at.remove(&q).expect("allocated page was on the list");
                                    prop_assert!(f <= committed, "page {q} freed at {f}, committed {committed}");
                                    prop_assert!(f <= min_snap, "page {q} freed at {f}, oldest snapshot {min_snap}");
                                }
                                p
                            }
                            None => {
                                let p = next_page;
                                next_page += n;
                                p
                            }
                        };
                        for &(lp, lc) in &live {
                            prop_assert!(page + n <= lp || lp + lc <= page, "overlaps a live run");
                        }
                        live.push((page, n));
                    }
                    Op::FreeOne(i) => {
                        if !live.is_empty() {
                            let (p, c) = live.swap_remove(i % live.len());
                            fl.free(txn, p, c).unwrap();
                            for q in p..p + c {
                                freed_at.insert(q, txn);
                            }
                        }
                    }
                    Op::Commit => committed = txn,
                    Op::Snapshot => snaps.push(committed),
                    Op::DropSnapshot => {
                        if !snaps.is_empty() {
                            snaps.remove(0);
                        }
                    }
                }
                prop_assert_eq!(fl.pages(), freed_at.len() as u64);
            }
        }
    }
}
