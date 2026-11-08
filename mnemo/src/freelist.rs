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
    pub fn pages(&self) -> u64 {
        self.runs.values().map(|&(c, _)| c).sum()
    }

    /// FMT-9c retention: pages a snapshot at `min_snapshot_txn` keeps from
    /// reuse (freed after it).
    pub fn pinned_pages(&self, min_snapshot_txn: u64) -> u64 {
        self.runs
            .values()
            .filter(|&&(_, f)| f > min_snapshot_txn)
            .map(|&(c, _)| c)
            .sum()
    }

    /// FMT-9c: if an eligible run ends exactly at `end` (the file's tail),
    /// take it off the list and return its first page, so the file can
    /// shrink to it.
    pub fn take_tail(
        &mut self,
        end: u64,
        committed_txn: u64,
        min_snapshot_txn: u64,
    ) -> Option<u64> {
        let (&p, &(c, f)) = self.runs.range(..end).next_back()?;
        if p + c == end && f <= committed_txn && f <= min_snapshot_txn {
            self.runs.remove(&p);
            Some(p)
        } else {
            None
        }
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
        // Only runs freed by the same transaction coalesce here. Merging an
        // eligible run with a just-freed neighbour would make the whole
        // run wait out the neighbour's quarantine (and a file's tail,
        // re-freed by every commit, would never come off).
        let mut start = page;
        let mut len = count;
        if let Some((&p, &(c, f))) = self.runs.range(..=page).next_back() {
            if p + c > page {
                return Err(MnemoError::Invalid(format!(
                    "free list: double free of page {page} (overlaps run {p}+{c})"
                )));
            }
            if p + c == page && f == freed_at {
                self.runs.remove(&p);
                start = p;
                len += c;
            }
        }
        // Neighbour after.
        if let Some((&p, &(c, f))) = self.runs.range(page..).next() {
            if p < end {
                return Err(MnemoError::Invalid(format!(
                    "free list: double free of page {p} (inside freed run {page}+{count})"
                )));
            }
            if p == end && f == freed_at {
                self.runs.remove(&p);
                len += c;
            }
        }
        self.runs.insert(start, (len, freed_at));
        Ok(())
    }

    /// First-fit allocation of `count` contiguous eligible pages. Returns
    /// the first page, or `None` when no eligible run is long enough (the
    /// caller then extends the file).
    pub fn alloc(&mut self, count: u64, committed_txn: u64, min_snapshot_txn: u64) -> Option<u64> {
        if count == 0 {
            return None;
        }
        let eligible = |f: u64| f <= committed_txn && f <= min_snapshot_txn;
        let fits = |runs: &BTreeMap<u64, (u64, u64)>| {
            runs.iter()
                .find(|(_, &(c, f))| c >= count && eligible(f))
                .map(|(&p, &(c, f))| (p, c, f))
        };
        let (page, c, f) = match fits(&self.runs) {
            Some(hit) => hit,
            None => {
                // Adjacent runs freed by different transactions are kept
                // apart; once both are eligible they can serve one
                // allocation together.
                self.merge_eligible(committed_txn, min_snapshot_txn);
                fits(&self.runs)?
            }
        };
        self.runs.remove(&page);
        if c > count {
            self.runs.insert(page + count, (c - count, f));
        }
        Some(page)
    }

    /// Merge adjacent runs that are both eligible. Eligibility only grows
    /// (the committed txn and the oldest retained snapshot only move
    /// forward), so the merged run, stamped with the later `freed_at`,
    /// stays eligible.
    pub fn merge_eligible(&mut self, committed_txn: u64, min_snapshot_txn: u64) {
        let eligible = |f: u64| f <= committed_txn && f <= min_snapshot_txn;
        let runs: Vec<(u64, u64, u64)> = self.runs.iter().map(|(&p, &(c, f))| (p, c, f)).collect();
        let mut merged: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
        let mut cur: Option<(u64, u64, u64)> = None;
        for (p, c, f) in runs {
            cur = match cur {
                Some((cp, cc, cf)) if cp + cc == p && eligible(cf) && eligible(f) => {
                    Some((cp, cc + c, cf.max(f)))
                }
                Some((cp, cc, cf)) => {
                    merged.insert(cp, (cc, cf));
                    Some((p, c, f))
                }
                None => Some((p, c, f)),
            };
        }
        if let Some((cp, cc, cf)) = cur {
            merged.insert(cp, (cc, cf));
        }
        self.runs = merged;
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
        fl.free(5, 12, 3).unwrap();
        assert_eq!(fl.len(), 1, "same transaction: coalesced");
        fl.free(6, 8, 2).unwrap();
        assert_eq!(fl.len(), 2, "different transactions stay apart");
        // A 7-page allocation needs both; they merge once both are eligible.
        assert_eq!(fl.alloc(7, 5, u64::MAX), None, "txn 6 isn't committed");
        assert_eq!(fl.alloc(7, 6, u64::MAX), Some(8));
        assert_eq!(fl.len(), 0);
        fl.free(6, 10, 2).unwrap();
        fl.free(6, 12, 2).unwrap();
        assert_eq!(fl.alloc(0, 6, u64::MAX), None);
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
    fn tail_runs_come_off_only_when_eligible() {
        let mut fl = FreeList::default();
        fl.free(3, 10, 5).unwrap(); // pages 10..15
        assert_eq!(fl.take_tail(16, 9, u64::MAX), None, "not at the tail");
        assert_eq!(fl.take_tail(15, 2, u64::MAX), None, "not committed");
        assert_eq!(fl.take_tail(15, 9, 2), None, "pinned by a snapshot");
        assert_eq!(fl.pinned_pages(2), 5);
        assert_eq!(fl.take_tail(15, 9, 9), Some(10));
        assert_eq!(fl.pages(), 0);
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
