//! Snapshots and point-in-time recovery (restore, retention), and sessions.

use super::*;

impl Mnemo {
    /// Begin a conversation [`Session`](crate::Session) for `agent_id`.
    ///
    /// The session borrows the database for its lifetime, records turns as
    /// working memory, and consolidates them into episodic memory when closed.
    pub fn session(&mut self, agent_id: impl Into<String>) -> crate::session::Session<'_> {
        crate::session::Session::new(self, agent_id.into())
    }

    // --- snapshots & point-in-time recovery ------------------------------

    /// Every committed transaction, oldest first — the restore points
    /// available to [`Mnemo::restore_to`] and [`Mnemo::restore_to_time`].
    ///
    /// Each `flush` appends one snapshot. Because the storage engine is
    /// append-only, the pages a past flush wrote are still on disk, so any
    /// listed snapshot can be reinstated exactly. The history reaches back to
    /// the last [`Mnemo::compact_file`], which reclaims space by collapsing
    /// it.
    pub fn snapshots(&self) -> Vec<SnapshotInfo> {
        let mut v: Vec<SnapshotInfo> = self
            .manifest
            .iter()
            .map(|s| SnapshotInfo {
                txn_id: s.txn_id,
                created_at: s.created_at,
                memory_count: s.memory_count,
            })
            .collect();
        v.sort_by_key(|s| s.txn_id);
        v
    }

    /// Load a past snapshot's state and commit it as a new transaction.
    pub(super) fn apply_snapshot(&mut self, snap: &Snapshot) -> Result<()> {
        let catalog = load_catalog(
            &mut self.pager,
            snap.catalog_start,
            snap.catalog_pages,
            snap.catalog_len,
            snap.catalog_gen(),
        )?;
        let (catalog, _, _) = apply_delta_chain(&mut self.pager, catalog, snap.delta_head())?;
        // FMT-9c.2: the restored catalog is written whole.
        self.catalog_rewrite = true;
        let ann = load_index(
            &mut self.pager,
            snap.index_start,
            snap.index_pages,
            snap.index_len,
            snap.index_gen(),
            self.dimensions,
        )?;
        let mut ann = ann;
        if self.is_v9() {
            self.reclaim_for_restore(&catalog);
            if let Some(a) = ann.as_mut() {
                apply_index_chain(&mut self.pager, a, snap.index_delta_head())?;
                a.enable_log();
            }
            self.index_rebase = true;
            // FMT-9d: the snapshot's BM25 segments (quarantined while it's
            // retained) describe its documents.
            let bptr = snap.gens.map_or(RunPtr::default(), |g| tuple_ptr(g.bm25));
            let manifest = load_bm25_manifest(&mut self.pager, bptr)?;
            // Its runs come back off the free list; the current ones it
            // doesn't share are freed, and the header points at its run.
            let runs = |ptr: RunPtr, m: &Bm25Manifest| {
                let mut v: Vec<(u64, u64)> = m.segments.iter().map(|t| (t.0, t.1)).collect();
                if !ptr.is_empty() {
                    v.push((ptr.start, ptr.pages));
                }
                v
            };
            let then = runs(bptr, &manifest);
            let now = runs(self.header.bm25, &self.bm25.manifest);
            self.pending_free.retain(|r| !then.contains(r));
            for &(p, c) in &then {
                self.free.unfree(p, c);
            }
            for r in now {
                if !then.contains(&r) {
                    self.release_pages(r.0, r.1);
                }
            }
            self.header.bm25 = bptr;
            self.free_dirty = true;
            self.bm25 = Bm25State {
                manifest,
                ..Bm25State::default()
            };
        }
        self.index = build_id_index(&catalog);
        self.catalog = catalog;
        // FMT-9b: the column and the packed pages described the catalog
        // being replaced.
        self.col = VecColumn::from_catalog(&self.catalog);
        self.recs = RecPages::from_catalog(&self.catalog);
        self.ann = ann;
        self.ann_unloaded = false;
        // The handle caches describe the catalog being replaced.
        self.key_index = None;
        self.superseded = None;
        self.layers = None;
        self.agent_set = None;
        // PERF-1: the snapshot's index may predate its catalog; bring it up
        // to date before it's persisted again.
        let snap_index_gen = snap.index_gen().max(snap.index_delta_head().gen);
        self.reconcile_ann_with_catalog(snap.index_start, snap_index_gen)?;
        // Re-commit as a fresh transaction: crash-safe, and itself recorded
        // as a new snapshot so a restore can always be undone.
        self.dirty_catalog = true;
        self.dirty_index = true;
        self.index_persist.forced = true;
        self.flush()
    }

    /// FMT-9c: v9 snapshot retention, applied to the staged manifest (the
    /// new snapshot already appended): keep the newest `max_snapshots`
    /// plus any younger than 24 hours, then drop the oldest while retained
    /// snapshots pin more than a quarter of the file's live pages. The
    /// newest snapshot always stays.
    ///
    /// The pin cap is a quarter of the live pages or [`V9_PIN_FLOOR_PAGES`]
    /// (1 MiB), whichever is larger, so tiny files keep their history.
    /// `extra` is what this commit frees (pinned by every older snapshot).
    ///
    /// An explicit count (`set_max_snapshots`, or a non-default
    /// `MnemoConfig::max_snapshots`) replaces the policy: exactly the
    /// newest `n` are kept (`0` keeps all), whatever they pin.
    pub(super) fn apply_retention(&self, manifest: &mut Vec<Snapshot>, extra: u64) {
        if self.snapshot_cap_explicit {
            if self.max_snapshots > 0 {
                while manifest.len() > self.max_snapshots {
                    manifest.remove(0);
                }
            }
            return;
        }
        let now = memory::now_secs();
        if self.max_snapshots > 0 {
            while manifest.len() > self.max_snapshots
                && now - manifest[0].created_at > V9_SNAPSHOT_KEEP_SECS
            {
                manifest.remove(0);
            }
        }
        let live_pages = self.header.next_page.saturating_sub(self.free.pages());
        let cap = (live_pages / 4).max(V9_PIN_FLOOR_PAGES);
        while manifest.len() > 1 {
            let pinned = self.free.pinned_pages(manifest[0].txn_id) + extra;
            if pinned <= cap {
                break;
            }
            manifest.remove(0);
        }
    }

    /// FMT-9 (a): a restore makes the snapshot's records live again and the
    /// current ones (those the snapshot doesn't have) dead. The snapshot's
    /// record runs may be on the free list (freed after the snapshot, and
    /// quarantined while it's retained), so they come back off it, and off
    /// this transaction's pending frees; the current runs it doesn't share
    /// are freed by the restoring commit.
    pub(super) fn reclaim_for_restore(&mut self, restored: &[CatalogEntry]) {
        // Record runs, deduplicated (FMT-9b.2: packed records share a page);
        // rows still waiting to be packed own no page yet.
        let runs = |cat: &[CatalogEntry]| -> std::collections::BTreeMap<u64, u64> {
            cat.iter()
                .filter(|e| !e.deleted && e.start_page != 0)
                .map(|e| (e.start_page, e.page_count as u64))
                .collect()
        };
        let live_then = runs(restored);
        let live_now = runs(&self.catalog);
        let overlaps = |p: u64, c: u64| {
            live_then
                .range(..p + c)
                .next_back()
                .is_some_and(|(&q, &n)| q + n > p)
        };
        let dying: Vec<(u64, u64)> = live_now
            .iter()
            .filter(|(p, _)| !live_then.contains_key(p))
            .map(|(&p, &c)| (p, c))
            .collect();
        // FMT-9b: vector pages hold many rows, so they're compared as a
        // set: the snapshot's come back, the current ones it lacks go.
        let vec_then: std::collections::BTreeSet<u64> = restored
            .iter()
            .filter(|e| !e.deleted)
            .filter_map(|e| e.v9.map(|r| r.vec_page))
            .filter(|&p| p != 0)
            .collect();
        let vec_dying: Vec<u64> = self
            .col
            .page_rows
            .keys()
            .copied()
            .filter(|p| !vec_then.contains(p))
            .collect();
        self.pending_free
            .retain(|&(p, c)| !(overlaps(p, c) || (c == 1 && vec_then.contains(&p))));
        for (&p, &c) in &live_then {
            self.free.unfree(p, c);
        }
        for &p in &vec_then {
            self.free.unfree(p, 1);
        }
        for (p, c) in dying {
            self.release_pages(p, c);
        }
        for p in vec_dying {
            self.release_pages(p, 1);
        }
        self.free_dirty = true;
    }

    /// Restore the database to the snapshot produced by transaction `txn_id`.
    ///
    /// The restore is itself a new committed transaction (and a new
    /// snapshot), so it is crash-safe and reversible — restoring forward to a
    /// later snapshot afterwards works just as well.
    pub fn restore_to(&mut self, txn_id: u64) -> Result<SnapshotInfo> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let snap = self
            .manifest
            .iter()
            .find(|s| s.txn_id == txn_id)
            .cloned()
            .ok_or_else(|| {
                let oldest = self.manifest.iter().map(|s| s.txn_id).min();
                match oldest {
                    Some(o) if self.is_v9() && txn_id < o => MnemoError::NotFound(format!(
                        "snapshot for transaction {txn_id}: older than the retention window \
                         (the newest {} snapshots and any from the last 24 hours, with at \
                         most a quarter of the file pinned); the oldest kept is {o}",
                        self.max_snapshots
                    )),
                    _ => MnemoError::NotFound(format!("snapshot for transaction {txn_id}")),
                }
            })?;
        self.apply_snapshot(&snap)?;
        Ok(SnapshotInfo {
            txn_id: snap.txn_id,
            created_at: snap.created_at,
            memory_count: snap.memory_count,
        })
    }

    /// Restore the database to the latest snapshot committed at or before
    /// `unix_secs`. Returns [`MnemoError::NotFound`] if no snapshot is that
    /// old. Like [`Mnemo::restore_to`], the restore is a new transaction.
    pub fn restore_to_time(&mut self, unix_secs: i64) -> Result<SnapshotInfo> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let snap = self
            .manifest
            .iter()
            .filter(|s| s.created_at <= unix_secs)
            .max_by_key(|s| s.txn_id)
            .cloned()
            .ok_or_else(|| {
                MnemoError::NotFound(format!("no snapshot at or before time {unix_secs}"))
            })?;
        self.apply_snapshot(&snap)?;
        Ok(SnapshotInfo {
            txn_id: snap.txn_id,
            created_at: snap.created_at,
            memory_count: snap.memory_count,
        })
    }
}
