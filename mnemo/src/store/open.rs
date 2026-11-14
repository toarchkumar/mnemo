//! Create, open (read-write, read-only, raw key) and the handle's basic accessors and per-handle settings.

use super::*;

/// Chunk size for the torn-header scan's streaming read of the file.
/// We walk the file 1 MiB at a time looking for 8 KiB-aligned `MWAL`
/// magic. There is **no** cap on how far a given candidate's frame
/// chain may extend — a 1M-vector IVF+PQ relocation can push a single
/// transaction well past 16 MiB — so each candidate is read frame by
/// frame via [`wal::scan_committed_txn`] rather than slurped into a
/// bounded buffer. (DUR-1 R-002.1 part 2.)
const TORN_HEADER_SCAN_CHUNK: usize = 1 << 20; // 1 MiB

/// The header a set of recovery frames would write to page 0, if they hold
/// one that parses.
fn page_zero_header(frames: &[(u64, Vec<u8>)]) -> Option<Header> {
    frames
        .iter()
        .find(|(page_no, _)| *page_no == 0)
        .and_then(|(_, bytes)| Header::from_page(bytes).ok())
}

/// Whether `h` is a v9 header sealed under `dek` (RB-007.2). The version
/// matters: `validate_seal` passes any header older than v7, which has no
/// seal.
fn sealed_under(h: &Header, dek: &[u8; crypto::KEY_LEN]) -> bool {
    h.version >= VERSION_V9 && h.validate_seal(dek).is_ok()
}

/// Walk the file for every 8 KiB-aligned page whose first four bytes are
/// `MWAL`, treating each as a candidate WAL-region start, and return the
/// frames of the highest-`txn_id` committed transaction whose header
/// frame fully validates under `passphrase`.
///
/// A candidate is accepted only if:
/// * `wal::scan_committed_txn` returns a committed transaction (chained
///   CRC verifies, single `txn_id`, `KIND_COMMIT` closes it);
/// * that transaction contains a `(0, header_page)` frame;
/// * the header parses with `wal_start == candidate_page_no` and
///   `wal_seq == txn_id` (so a frame cannot forge a region it was not
///   written into);
/// * deriving the KEK from `passphrase + header.salt`, unwrapping the
///   wrapped DEK, and validating the header seal all succeed.
///
/// Returns `Ok(None)` when the scan finds no valid candidate — the
/// caller is expected to propagate the original parse error so
/// non-mnemo files still report `BadMagic`.
///
/// DUR-1 (R-002.1 part 2): there is no fixed scan window — the whole
/// file is scanned. Reading at page granularity (not byte granularity)
/// is sufficient because every WAL region in the format is page-aligned
/// and every WAL commit writes its first frame at the region's start.
fn scan_header_recovery_candidates(
    file: &mut File,
    secret: &[u8],
    held_dek: Option<&[u8; crypto::KEY_LEN]>,
) -> Result<Option<Vec<wal::Frame>>> {
    let file_len = file.seek(SeekFrom::End(0))?;
    if file_len < PAGE_SIZE as u64 * 2 {
        return Ok(None); // No room for even one WAL page past the header.
    }

    let mut best: Option<(u64, Vec<wal::Frame>)> = None;
    let page_sz = PAGE_SIZE as u64;
    let mut offset = page_sz; // Skip page 0.

    // Stream through the file in 1 MiB chunks, scanning each chunk for
    // page-aligned `MWAL` candidates. Each candidate's frame chain is
    // then read incrementally via `wal::scan_committed_txn`, which has
    // no window cap — a 1M-vector IVF+PQ relocation can span tens of
    // MiB in one committed transaction.
    let mut chunk = vec![0u8; TORN_HEADER_SCAN_CHUNK];
    while offset < file_len {
        let want = (TORN_HEADER_SCAN_CHUNK as u64).min(file_len - offset) as usize;
        chunk.resize(want, 0);
        file.seek(SeekFrom::Start(offset))?;
        let mut filled = 0;
        while filled < want {
            match file.read(&mut chunk[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        if filled == 0 {
            break;
        }
        let mut inner = 0usize;
        while inner + 4 <= filled {
            if &chunk[inner..inner + 4] == wal::FRAME_MAGIC_BYTES {
                let page_no = (offset + inner as u64) / page_sz;
                if let Some((txn_id, frames)) =
                    try_recover_candidate(file, page_no, secret, held_dek)?
                {
                    let keep = match &best {
                        None => true,
                        Some((id, _)) => txn_id > *id,
                    };
                    if keep {
                        best = Some((txn_id, frames));
                    }
                }
            }
            inner += PAGE_SIZE;
        }
        offset += filled as u64;
    }

    Ok(best.map(|(_, f)| f))
}

/// Try to recover a committed transaction from `candidate_page_no`,
/// returning `(txn_id, frames)` only if the candidate's
/// `(0, header_page)` frame fully validates: the frame chain commits
/// (via [`wal::scan_committed_txn`]'s streaming reader), the header
/// parses with `wal_start == candidate_page_no` **and**
/// `wal_seq == txn_id`, the DEK unwraps under `passphrase`, and the
/// AES-GCM seal validates. DUR-1 (R-002.1 item 2) requires the
/// `wal_seq == txn_id` check and no fixed scan window — the streaming
/// reader handles transactions of arbitrary size.
fn try_recover_candidate(
    file: &mut File,
    candidate_page_no: u64,
    secret: &[u8],
    held_dek: Option<&[u8; crypto::KEY_LEN]>,
) -> Result<Option<(u64, Vec<wal::Frame>)>> {
    let Some((txn_id, frames)) = wal::scan_committed_txn(file, candidate_page_no)? else {
        return Ok(None);
    };

    // Must contain a header frame at page 0.
    let header_bytes = match frames.iter().find(|(p, _)| *p == 0) {
        Some((_, b)) => b,
        None => return Ok(None),
    };
    if header_bytes.len() != PAGE_SIZE {
        return Ok(None);
    }
    let mut hbuf = [0u8; PAGE_SIZE];
    hbuf.copy_from_slice(header_bytes);
    let candidate = match Header::from_page(&hbuf) {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };

    // The frame's claimed region must match where we found it, and the
    // header frame's `wal_seq` must match the committed transaction's
    // `txn_id` — this is what pins the frame to the region it was
    // written into and keeps a stale checkpointed-and-copied frame
    // from being accepted as a new region's root.
    if candidate.wal_start != candidate_page_no || candidate.wal_seq != txn_id {
        return Ok(None);
    }
    // RB-007.2: a shared handle's data key, when the candidate's seal
    // verifies under it, without the KDF.
    if let Some(held) = held_dek {
        if sealed_under(&candidate, held) {
            return Ok(Some((txn_id, frames)));
        }
    }
    // DEK unwrap under this candidate's own salt + KDF params.
    let kdf = KdfParams {
        m_cost: candidate.m_cost,
        t_cost: candidate.t_cost,
        p_cost: candidate.p_cost,
    };
    let kek = match crypto::derive_kek(secret, &candidate.salt, kdf) {
        Ok(k) => k,
        Err(_) => return Ok(None),
    };
    let dek = match crypto::unwrap_dek(&kek, &candidate.dek_nonce, &candidate.wrapped_dek) {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };
    if candidate.validate_seal(&dek).is_err() {
        return Ok(None);
    }

    Ok(Some((txn_id, frames)))
}

impl Mnemo {
    /// Create a brand-new encrypted database at `path`.
    pub fn create(
        path: impl Into<PathBuf>,
        passphrase: &str,
        config: MnemoConfig,
    ) -> Result<Mnemo> {
        Self::create_secret(path, passphrase.as_bytes(), SecretKind::Passphrase, config)
    }

    /// PERF-1 step 7 (R-004.5): create a database unlocked by a **raw
    /// 256-bit key** instead of a passphrase, for keys held by an OS
    /// keychain or keystore. The key must be uniformly random (see
    /// [`Mnemo::generate_key`]); it gets no stretching. The header records
    /// minimal Argon2id parameters ([`KdfParams::raw_key`]), so opening
    /// costs microseconds instead of tens of milliseconds. `config.kdf` is
    /// ignored. No format change: the key goes through the same KEK
    /// derivation a passphrase does.
    pub fn create_with_key(
        path: impl Into<PathBuf>,
        key: &[u8; crypto::KEY_LEN],
        config: MnemoConfig,
    ) -> Result<Mnemo> {
        let config = MnemoConfig {
            kdf: KdfParams::raw_key(),
            ..config
        };
        Self::create_secret(path, key, SecretKind::Key, config)
    }

    /// A fresh random 256-bit key for [`Mnemo::create_with_key`], from the
    /// thread-local CSPRNG (seeded from the OS). Store it in a keychain;
    /// it can't be recovered from the file.
    pub fn generate_key() -> [u8; crypto::KEY_LEN] {
        *crypto::random_dek()
    }

    pub(super) fn create_secret(
        path: impl Into<PathBuf>,
        secret: &[u8],
        kind: SecretKind,
        config: MnemoConfig,
    ) -> Result<Mnemo> {
        Self::create_secret_as(path.into(), secret, kind, config, Create::Truncate, None)
    }

    /// [`Mnemo::create_secret`] for a temp file: `Create::New` refuses a
    /// name that already exists (a symlink included), with `AlreadyExists`.
    /// `salt` is the salt to keep (a compaction's, 12.3's salt rule), or
    /// `None` for a fresh one.
    pub(super) fn create_secret_new(
        path: &Path,
        secret: &[u8],
        kind: SecretKind,
        config: MnemoConfig,
        salt: Option<[u8; crypto::SALT_LEN]>,
    ) -> Result<Mnemo> {
        Self::create_secret_as(path.to_path_buf(), secret, kind, config, Create::New, salt)
    }

    /// Where every create meets. 12.3: a passphrase with raw-key parameters
    /// is refused here, before anything touches the disk (the keys note,
    /// section 1, rule 2).
    pub(super) fn create_secret_as(
        path: PathBuf,
        secret: &[u8],
        kind: SecretKind,
        config: MnemoConfig,
        create: Create,
        salt: Option<[u8; crypto::SALT_LEN]>,
    ) -> Result<Mnemo> {
        crypto::check_kdf_for(kind, config.kdf)?;
        if config.dimensions == 0 {
            return Err(MnemoError::Invalid("dimensions must be > 0".into()));
        }
        let version = resolve_format_version(config.format_version)?;
        if version >= VERSION_V9 && veccol::per_page(config.dimensions) == 0 {
            return Err(MnemoError::Invalid(format!(
                "a v9 file supports up to {} dimensions for now (one fp16 vector \
                 must fit a page); vector entries that span pages are not supported yet",
                (PAYLOAD - veccol::PAGE_HEADER - 4) / 2
            )));
        }
        let flags = if version >= VERSION_V9 && config.store_float_vectors {
            FLAG_ENCRYPTED | FLAG_FLOAT_VECTORS
        } else {
            FLAG_ENCRYPTED
        };
        // Exclusive lock covers the write path; second writer sees `Locked`.
        let file = open_with_lock(&path, /*write=*/ true, create)?;

        let salt = salt.unwrap_or_else(crypto::random_salt);
        let dek = crypto::random_dek();
        let kek = crypto::derive_kek(secret, &salt, config.kdf)?;
        let dek_nonce = crypto::random_nonce();
        let wrapped = crypto::wrap_dek(&kek, &dek_nonce, &dek)?;
        if wrapped.len() != WRAPPED_DEK_LEN {
            return Err(MnemoError::Crypto("unexpected wrapped DEK length".into()));
        }
        let mut wrapped_dek = [0u8; WRAPPED_DEK_LEN];
        wrapped_dek.copy_from_slice(&wrapped);

        let mut header = Header {
            version,
            page_size: PAGE_SIZE as u32,
            flags,
            dimensions: config.dimensions as u32,
            created_at: memory::now_secs(),
            write_counter: 0,
            // Page 0 is the header; pages 1..=wal_pages are the WAL;
            // record/catalog/index pages start after it. Initial WAL size is
            // chosen per-config (default 8 pages = 64 KiB) and clamped to a
            // sane floor so the first transaction can always commit.
            next_page: 1 + config.wal_pages_initial.max(MIN_WAL_PAGES),
            catalog_start: 0,
            catalog_pages: 0,
            catalog_len: 0,
            vector_count: 0,
            m_cost: config.kdf.m_cost,
            t_cost: config.kdf.t_cost,
            p_cost: config.kdf.p_cost,
            salt,
            dek_nonce,
            wrapped_dek,
            index_start: 0,
            index_pages: 0,
            index_len: 0,
            wal_start: 1,
            wal_pages: config.wal_pages_initial.max(MIN_WAL_PAGES),
            wal_seq: 0,
            manifest_start: 0,
            manifest_pages: 0,
            manifest_len: 0,
            // Regenerated per write by `apply_seal`; `None` until we seal.
            seal_nonce: None,
            seal_tag: None,
            // v8+ result-cache pointer — zero-initialized on create,
            // populated only when `cache_put` runs at least once and
            // the next `flush` seals a directory run.
            cache_start: 0,
            cache_pages: 0,
            cache_len: 0,
            // FMT-9 v9 fields; all zero on a fresh file (and unused on v8).
            catalog_gen: 0,
            index_gen: 0,
            manifest_gen: 0,
            cache_gen: 0,
            free: RunPtr::default(),
            keys: RunPtr::default(),
            bm25: RunPtr::default(),
            index_delta: RunPtr::default(),
            catalog_delta: RunPtr::default(),
        };

        let mut pager = Pager::new(file, dek, 0, version);
        let mut page = header.to_page();
        header.apply_seal(&mut page, pager.dek())?;
        pager.write_raw(0, &page)?;
        pager.sync()?;

        Ok(Mnemo {
            pager,
            header,
            catalog: Vec::new(),
            index: IdIndex::default(),
            path: absolute(path),
            dimensions: config.dimensions,
            kdf: config.kdf,
            dirty_catalog: false,
            ann: None,
            dirty_index: false,
            index_persist: IndexPersist::default(),
            ann_unloaded: false,
            manifest: Vec::new(),
            // FMT-9c: v9 keeps fewer snapshots by default: with page reuse,
            // retention is what pins pages.
            max_snapshots: if version >= VERSION_V9 && config.max_snapshots == DEFAULT_MAX_SNAPSHOTS
            {
                V9_DEFAULT_MAX_SNAPSHOTS
            } else {
                config.max_snapshots
            },
            read_only: false,
            cache_dir: Vec::new(),
            cache_idx: DirectoryIndex::new(),
            dirty_cache: false,
            cache_budgets: std::collections::HashMap::new(),
            cache_flush_policy: CacheFlushPolicy::default(),
            cache_batch: BatchState::default(),
            cache_counters: CounterState::default(),
            embedder: None,
            fsyncs_last_flush: 0,
            agent_set: None,
            key_index: None,
            superseded: None,
            layers: None,
            rekey_cleanup: None,
            access_delta: None,
            commit_point: None,
            lock_note: None,
            notices: Vec::new(),
            window_committed: false,
            shared: false,
            turn_compress_after: DEFAULT_TURN_COMPRESS_AFTER_SECS,
            free: FreeList::default(),
            pending_free: Vec::new(),
            free_dirty: false,
            key_slots: KeySlotRun::default(),
            dirty_keys: false,
            gen_leased: false,
            col: VecColumn::default(),
            recs: RecPages::default(),
            bm25: Bm25State::default(),
            kw: None,
            index_layout_override: None,
            dirty_rows: std::collections::BTreeSet::new(),
            delta_chain: Vec::new(),
            delta_rows: 0,
            catalog_rewrite: false,
            index_chain: Vec::new(),
            index_chain_ops: 0,
            index_rebase: false,
            index_delta_ops: None,
            snapshot_cap_explicit: config.max_snapshots != DEFAULT_MAX_SNAPSHOTS,
            direct_writes: false,
            vector_cache_budget: DEFAULT_VECTOR_CACHE_BUDGET,
        })
    }

    /// Open an existing database. A wrong passphrase fails cleanly with
    /// [`MnemoError::WrongPassphrase`]. Takes an OS exclusive lock for
    /// the lifetime of the returned handle; a second concurrent open
    /// (from any process) returns [`MnemoError::Locked`].
    ///
    /// For a caller that only needs to read (and can tolerate the
    /// pending-recovery / pending-migration cases documented on
    /// [`Mnemo::open_read_only`]), prefer that entry point — it takes a
    /// shared lock and coexists with other readers.
    pub fn open(path: impl Into<PathBuf>, passphrase: &str) -> Result<Mnemo> {
        Self::open_secret(path, passphrase.as_bytes()).map(passphrase_opened)
    }

    /// PERF-1 step 7: open a database created by [`Mnemo::create_with_key`]
    /// (or rekeyed with [`Mnemo::rekey_with_key`]) with its raw key.
    pub fn open_with_key(path: impl Into<PathBuf>, key: &[u8; crypto::KEY_LEN]) -> Result<Mnemo> {
        Self::open_secret(path, key)
    }

    pub(super) fn open_secret(path: impl Into<PathBuf>, secret: &[u8]) -> Result<Mnemo> {
        Self::open_secret_with(path, secret, false)
    }

    /// The read-write open. `allow_old` lets a file older than v9 open (and
    /// migrate in memory to v8): only compaction uses it, to read a v4–v8
    /// file and write the v9 copy. Every public read-write open refuses
    /// those with [`MnemoError::FormatTooOld`] (the v8 sunset, v0.13).
    pub(super) fn open_secret_with(
        path: impl Into<PathBuf>,
        secret: &[u8],
        allow_old: bool,
    ) -> Result<Mnemo> {
        let path: PathBuf = path.into();
        let file = open_with_lock(&path, /*write=*/ true, Create::No)?;
        Self::load_locked(path, file, secret, allow_old)
    }

    /// Open read-write through 12.4's lock layer, for a long operation that
    /// must not run alongside shared handles' calls: an import, a rekey, a
    /// compaction ([`Mnemo::compact_file`] does this itself).
    ///
    /// Unlike [`Mnemo::open`], which tries the lock once and fails with
    /// [`MnemoError::Locked`], this waits up to `opts.deadline` with writer
    /// preference (the intent lock), so busy shared readers can't keep it
    /// out. Once it holds the lock it writes a `lock-info` note naming `op`,
    /// so a shared caller that times out learns what holds the file. The
    /// handle keeps the exclusive lock until it's dropped or closed, like
    /// any read-write handle.
    ///
    /// Where shared mode isn't available (a network or synced filesystem
    /// without `opts.allow_network_fs`, or a Windows build made with Rust
    /// before 1.85) it opens as [`Mnemo::open`] does: one try, no note.
    pub fn open_exclusive(
        path: impl AsRef<Path>,
        passphrase: &str,
        op: &str,
        opts: crate::SharedOptions,
    ) -> Result<Mnemo> {
        Self::open_exclusive_secret(path.as_ref(), passphrase.as_bytes(), op, &opts, false)
            .map(passphrase_opened)
    }

    /// [`Mnemo::open_exclusive`] for a raw-key file.
    pub fn open_exclusive_with_key(
        path: impl AsRef<Path>,
        key: &[u8; crypto::KEY_LEN],
        op: &str,
        opts: crate::SharedOptions,
    ) -> Result<Mnemo> {
        Self::open_exclusive_secret(path.as_ref(), key, op, &opts, false)
    }

    pub(super) fn open_exclusive_secret(
        path: &Path,
        secret: &[u8],
        op: &str,
        opts: &crate::SharedOptions,
        allow_old: bool,
    ) -> Result<Mnemo> {
        let path = absolute(path.to_path_buf());
        let (file, note) = lock_exclusive(&path, op, opts)?;
        let mut db = Self::load_locked(path, file, secret, allow_old)?;
        db.lock_note = note;
        Ok(db)
    }

    /// The read-write open's work on a file its caller already holds
    /// exclusively: heal a torn header, replay a pending WAL, derive the key
    /// from `secret`, and build the handle. A shared-mode handle (12.4)
    /// reloads through it with the call's locked file.
    pub(super) fn load_locked(
        path: PathBuf,
        file: LockedFile,
        secret: &[u8],
        allow_old: bool,
    ) -> Result<Mnemo> {
        Self::load_locked_with(path, file, secret, None, allow_old)
    }

    /// [`Mnemo::load_locked`], with a data key the caller already holds (a
    /// shared handle's, RB-007.2). The torn-header scan accepts a candidate
    /// whose seal verifies under it, and a pending WAL is replayed with it
    /// when the header the replay writes verifies under it, without running
    /// the KDF: a rekey changes the secret, never the data key. Anything
    /// that doesn't verify under it is checked with `secret`, as an open
    /// does.
    pub(super) fn load_locked_with(
        path: PathBuf,
        mut file: LockedFile,
        secret: &[u8],
        held_dek: Option<&[u8; crypto::KEY_LEN]>,
        allow_old: bool,
    ) -> Result<Mnemo> {
        let mut hbuf = [0u8; PAGE_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hbuf)?;
        let mut header = match Header::from_page(&hbuf) {
            Ok(h) => h,
            Err(e) => {
                // DUR-1 (R-002.1 part 2+3): page 0 did not parse — any
                // error is treated as a torn-header candidate, not just
                // `HeaderChecksum`. The whole 294-byte header lives in
                // the first 512-byte sector, so the realistic failure is
                // the sector zeroed or garbled, which returns `BadMagic`
                // or `UnsupportedVersion` *before* the CRC gate.
                //
                // Scan the file for valid committed header-frame WAL
                // regions, pick the highest `txn_id`, and replay it. If
                // nothing valid is found, return the *original* error so
                // non-mnemo files still report `BadMagic`.
                let scan = scan_header_recovery_candidates(&mut file, secret, held_dek)?;
                let frames = scan.ok_or(e)?;
                // RB-003.3: the scan has checked the key; refuse an old
                // format before healing anything.
                if !allow_old {
                    if let Some(h) = page_zero_header(&frames) {
                        if h.version < VERSION_V9 {
                            return Err(MnemoError::FormatTooOld { version: h.version });
                        }
                    }
                }
                for (page_no, bytes) in &frames {
                    if bytes.len() != PAGE_SIZE {
                        return Err(MnemoError::Invalid("WAL frame is not page-sized".into()));
                    }
                    file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
                    file.write_all(bytes)?;
                }
                file.sync_all()?;
                crate::bump_fsync();
                file.seek(SeekFrom::Start(0))?;
                file.read_exact(&mut hbuf)?;
                Header::from_page(&hbuf)?
            }
        };

        // Crash recovery: a committed-but-uncheckpointed transaction is
        // replayed below. Each frame is a finished page image; the header
        // frame, replayed to page 0, supersedes the header just read.
        let pending = wal::recover(
            &mut file,
            header.wal_start,
            header.wal_pages,
            header.wal_seq,
        )?;

        // RB-003.3: settle the key and the format before writing anything,
        // so a wrong secret or a refused old format leaves the file
        // byte-identical. Both are checked against the header the file will
        // have once the WAL is replayed (its page-0 frame, else the current
        // header), since that header is the one the secret must open. No write
        // the engine makes leaves a pending page-0 frame whose key wrap differs
        // from page 0's (a rekey logs its new page 0 into a fresh WAL region,
        // so a cut-off rekey leaves the old secret or the new one, never a
        // mix), but checking against the replay's header is the right
        // defence if one ever does.
        let target = match pending.as_deref().and_then(page_zero_header) {
            Some(h) => h,
            None => header.clone(),
        };
        let kdf = KdfParams {
            m_cost: target.m_cost,
            t_cost: target.t_cost,
            p_cost: target.p_cost,
        };
        let dek = match held_dek {
            Some(held) if sealed_under(&target, held) => zeroize::Zeroizing::new(*held),
            _ => {
                let kek = crypto::derive_kek(secret, &target.salt, kdf)?;
                crypto::unwrap_dek(&kek, &target.dek_nonce, &target.wrapped_dek)?
            }
        };
        if !allow_old && target.version < VERSION_V9 {
            return Err(MnemoError::FormatTooOld {
                version: target.version,
            });
        }

        if let Some(frames) = pending {
            for (page_no, bytes) in &frames {
                if bytes.len() != PAGE_SIZE {
                    return Err(MnemoError::Invalid("WAL frame is not page-sized".into()));
                }
                file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
                file.write_all(bytes)?;
            }
            file.sync_all()?;
            crate::bump_fsync();
            // Re-read the now-current header from the replayed page 0.
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut hbuf)?;
            header = Header::from_page(&hbuf)?;
        }

        Self::from_locked(path, file, header, dek, kdf, allow_old)
    }

    /// Build a read-write handle from a locked file whose page 0 is
    /// `header`, with the data key already unwrapped. The shared-mode reload
    /// (12.4) comes here directly when the seal verifies under the key it
    /// holds, so it pays no KDF.
    pub(super) fn from_locked(
        path: PathBuf,
        file: LockedFile,
        mut header: Header,
        dek: zeroize::Zeroizing<[u8; crypto::KEY_LEN]>,
        kdf: KdfParams,
        allow_old: bool,
    ) -> Result<Mnemo> {
        // Validate the v7 header seal now that we have the DEK. Returns
        // `MnemoError::HeaderTampered` if any of the sealed mutable fields
        // were rewritten after the last legitimate flush. No-op for v6 and
        // older — pre-v7 files don't carry a seal.
        //
        // DUR-1 (Planner decision 2026-11-01, Option B): a seal failure is
        // **always** reported as tampering, never auto-healed. R-002.1
        // part 3's seal-mismatch WAL fallback is withdrawn: the seal
        // exists for tamper-evidence; silently loading a stale committed
        // header from the WAL would erase that evidence. A real crash
        // cannot produce "CRC valid + seal invalid" either, because the
        // whole 294-byte header lies inside sector 0 which storage
        // writes atomically — a torn sector breaks magic, version or
        // CRC first, and *those* are healed by the scan above.
        header.validate_seal(&dek)?;

        // Build the pager at the file's *current* version so initial reads
        // use the matching AAD scheme — `Pager::page_aad` returns empty AAD
        // for v4/v5 (no page binding) and `page_no.to_le_bytes()` for v6+.
        let on_disk_version = header.version;
        // The v8 sunset (v0.13): a read-write handle is v9. Older files open
        // read-only (v8) or through `compact_file`, which converts them.
        if !allow_old && on_disk_version < VERSION_V9 {
            return Err(MnemoError::FormatTooOld {
                version: on_disk_version,
            });
        }
        let mut pager = Pager::new(file, dek, header.write_counter, on_disk_version);
        let dimensions = header.dimensions as usize;

        // === v4→v5 catalog migration =====================================
        // v6 reads directly; v5 reads directly; v4 replays the catalog into
        // the v5 shape by reading each memory's body to populate the new
        // `accessed_at` / `access_count` fields. All page reads here still
        // use the v4/v5 AAD scheme (none).
        let (catalog, migrated_from_v4) = if on_disk_version >= 5 {
            (
                load_catalog(
                    &mut pager,
                    header.catalog_start,
                    header.catalog_pages,
                    header.catalog_len,
                    header.catalog_gen,
                )?,
                false,
            )
        } else if on_disk_version == 4 {
            let v4 = load_catalog_v4(
                &mut pager,
                header.catalog_start,
                header.catalog_pages,
                header.catalog_len,
            )?;
            (migrate_v4_catalog(&mut pager, v4)?, true)
        } else {
            return Err(MnemoError::UnsupportedVersion(on_disk_version));
        };
        // FMT-9c.2: replay the v9 delta chain onto the base catalog.
        let (catalog, delta_chain, delta_rows) =
            apply_delta_chain(&mut pager, catalog, header.catalog_delta)?;
        let index = build_id_index(&catalog);
        // PERF-1 step 3: the ANN index loads on first use, not at open. A
        // v5→v6 page migration is the exception: it must read the index
        // now, under the old page AAD, to rewrite it.
        let ann = if on_disk_version < 6 {
            load_index(
                &mut pager,
                header.index_start,
                header.index_pages,
                header.index_len,
                header.index_gen,
                dimensions,
            )?
        } else {
            None
        };
        let ann_unloaded = ann.is_none() && header.index_pages > 0;
        let mut manifest = load_manifest(
            &mut pager,
            header.manifest_start,
            header.manifest_pages,
            header.manifest_len,
            header.manifest_gen,
        )?;

        // === v5→v6 page-encryption migration =============================
        // Pre-v6 data pages were encrypted under no AAD; v6 binds the home
        // `page_no` as AAD so a page-transplant attack fails authentication
        // on read. Re-stage every live record page through `write_page` so
        // the next flush re-encrypts it under v6 AAD at the same home slot.
        // Tombstoned entries skip — they're reclaimed by `compact_file`.
        // Old catalog / ANN / manifest pages stay as orphan ciphertext;
        // the next flush writes the new runs at fresh slots.
        let migrated_pages_to_v6 = on_disk_version < 6;
        if migrated_pages_to_v6 {
            for e in &catalog {
                if e.deleted {
                    continue;
                }
                for i in 0..e.page_count as u64 {
                    let payload = pager.read_page(e.start_page + i, 0)?;
                    pager.write_page(e.start_page + i, 0, &payload)?;
                }
            }
            pager.set_version(6);
        }

        // === v6→v7 header-seal migration =================================
        // v7 adds an AES-GCM seal at the tail of the header page that
        // authenticates every mutable header field. Pre-v7 files have no
        // seal — we just bump the version in memory and mark `dirty_catalog`
        // so the next flush writes a sealed v7 header. Pages are untouched
        // (page crypto is unchanged from v6).
        let migrated_header_to_v7 = on_disk_version < 7;
        if migrated_header_to_v7 {
            pager.set_version(7);
        }

        // === v7→v8 cache-directory migration =============================
        // v8 adds three u64 header fields (cache_start/pages/len) for the
        // result-cache directory (Phase 10.1). Pre-v8 files have those
        // three bytes as zero (the on-disk region beyond the v7 seal is
        // zero-initialized), which is the legal "empty cache directory"
        // state — no data migration required. We bump the version in
        // memory and mark dirty_catalog so the next flush writes a
        // v8-sealed header with the new AAD shape covering the cache
        // pointer. Pages are untouched.
        let migrated_header_to_v8 = on_disk_version < 8;
        if migrated_header_to_v8 {
            pager.set_version(VERSION);
        }

        let migrated = migrated_from_v4
            || migrated_pages_to_v6
            || migrated_header_to_v7
            || migrated_header_to_v8;
        if migrated {
            // Stamp the version forward in memory; the next flush persists
            // it via the WAL-committed header frame (and the v7 seal).
            header.version = VERSION;
            // Snapshots written under an older format reference page runs
            // encrypted under the old AAD scheme; this build can't decrypt
            // them under the new pager. Drop the manifest so the next
            // flush records a fresh snapshot of the migrated state.
            // Point-in-time recovery into the pre-migration past is
            // sacrificed for migration simplicity. Old pages remain on
            // disk until `compact_file`. (For v6→v7 alone — which doesn't
            // change page crypto — the old snapshots would technically
            // still be readable, but dropping them uniformly keeps the
            // migration policy simple.)
            manifest.clear();
        }

        // The ANN index pages on disk were sealed under the *old* AAD scheme
        // if we migrated pages to v6; force a rewrite at fresh v6-encrypted
        // slots on the next flush so a future reopen doesn't try to read
        // them under the new AAD and fail. (Catalog and manifest already
        // get rewritten via dirty_catalog and the manifest.clear() above.)
        let dirty_index = migrated_pages_to_v6 && ann.is_some();

        // Load the cache directory (v8+ only). Pre-v8 files have
        // cache_pages == 0 by header layout, so this trivially yields
        // an empty directory without touching the disk.
        let cache_dir = load_cache_directory(
            &mut pager,
            header.cache_start,
            header.cache_pages,
            header.cache_len,
            header.cache_gen,
        )?;
        let cache_idx = DirectoryIndex::build(&cache_dir);
        // FMT-9 (a)/(b): the free list and key-slot run (empty pointers on
        // v8, so these don't touch the disk there).
        let free = load_free_list(&mut pager, header.free)?;
        let key_slots = load_key_slots(&mut pager, header.keys)?;
        let col = VecColumn::from_catalog(&catalog);
        let recs = RecPages::from_catalog(&catalog);
        let bm25_manifest = load_bm25_manifest(&mut pager, header.bm25)?;
        let max_snapshots = if header.version >= VERSION_V9 {
            V9_DEFAULT_MAX_SNAPSHOTS
        } else {
            DEFAULT_MAX_SNAPSHOTS
        };

        Ok(Mnemo {
            pager,
            header,
            catalog,
            index,
            path: absolute(path),
            dimensions,
            kdf,
            dirty_catalog: migrated,
            ann,
            dirty_index,
            // A v5→v6 page migration leaves the index sealed under the old
            // page AAD, so it must be rewritten on the next flush.
            index_persist: IndexPersist {
                forced: dirty_index,
                ..IndexPersist::default()
            },
            ann_unloaded,
            manifest,
            // `Mnemo::open` has no [`MnemoConfig`] to read; default the cap
            // to [`DEFAULT_MAX_SNAPSHOTS`]. Override with
            // [`Mnemo::set_max_snapshots`] right after open if you need
            // unlimited retention or a different cap for this handle.
            max_snapshots,
            read_only: false,
            cache_dir,
            cache_idx,
            dirty_cache: false,
            cache_budgets: std::collections::HashMap::new(),
            cache_flush_policy: CacheFlushPolicy::default(),
            cache_batch: BatchState::default(),
            cache_counters: CounterState::default(),
            embedder: None,
            fsyncs_last_flush: 0,
            agent_set: None,
            key_index: None,
            superseded: None,
            layers: None,
            rekey_cleanup: None,
            access_delta: None,
            commit_point: None,
            lock_note: None,
            notices: Vec::new(),
            window_committed: false,
            shared: false,
            turn_compress_after: DEFAULT_TURN_COMPRESS_AFTER_SECS,
            free,
            pending_free: Vec::new(),
            free_dirty: false,
            key_slots,
            dirty_keys: false,
            gen_leased: false,
            col,
            recs,
            bm25: Bm25State {
                manifest: bm25_manifest,
                ..Bm25State::default()
            },
            kw: None,
            index_layout_override: None,
            dirty_rows: std::collections::BTreeSet::new(),
            delta_chain,
            delta_rows,
            catalog_rewrite: false,
            index_chain: Vec::new(),
            index_chain_ops: 0,
            index_rebase: false,
            index_delta_ops: None,
            snapshot_cap_explicit: false,
            direct_writes: false,
            vector_cache_budget: DEFAULT_VECTOR_CACHE_BUDGET,
        })
    }

    /// Open an existing database in **read-only** mode. Takes an OS shared
    /// lock; multiple read-only handles can coexist, but any attempt to
    /// open the same file read-write elsewhere returns
    /// [`MnemoError::Locked`]. All mutating methods on the returned handle
    /// return [`MnemoError::ReadOnly`], including `recall` with
    /// `track_access(true)`.
    ///
    /// Two conditions block a read-only open and return
    /// [`MnemoError::NeedsWriteOpen`]:
    ///
    /// 1. The file has a committed-but-uncheckpointed WAL transaction that
    ///    would ordinarily be replayed on open. A read-only handle
    ///    cannot write the recovered pages back.
    /// 2. The on-disk format version is older than the current
    ///    `format::VERSION`, requiring an on-disk migration.
    ///
    /// In either case, open the file read-write once (which recovers /
    /// migrates in place), close the handle cleanly, and reopen
    /// read-only. This is documented in the README's Durability section.
    pub fn open_read_only(path: impl Into<PathBuf>, passphrase: &str) -> Result<Mnemo> {
        Self::open_read_only_secret(path, passphrase.as_bytes()).map(passphrase_opened)
    }

    /// PERF-1 step 7: [`Mnemo::open_read_only`] for a raw-key database.
    pub fn open_read_only_with_key(
        path: impl Into<PathBuf>,
        key: &[u8; crypto::KEY_LEN],
    ) -> Result<Mnemo> {
        Self::open_read_only_secret(path, key)
    }

    /// The embedding width the file at `path` records, read from its header
    /// (page 0 is sealed, not encrypted): no key, no lock, no KDF. A caller
    /// that embeds before it takes the lock uses it to build its embedder's
    /// identity (12.4: slow work first, unlocked). A torn header is an
    /// error here (a read-write open heals it).
    pub fn peek_dimensions(path: impl AsRef<std::path::Path>) -> Result<usize> {
        let mut file = File::open(path.as_ref())?;
        let mut hbuf = [0u8; PAGE_SIZE];
        file.read_exact(&mut hbuf)?;
        Ok(Header::from_page(&hbuf)?.dimensions as usize)
    }

    /// Whether `key` opens the raw-key file at `path`, decided from its
    /// header alone: no catalog load, no lock, microseconds. `Ok(false)` is
    /// "not this key". Unlike a read-only open, it answers for a file that
    /// has a WAL transaction to recover (the state a kill between a commit
    /// and its checkpoint leaves), so a caller trying each of its keys in
    /// turn isn't stopped by [`MnemoError::NeedsWriteOpen`]. A torn header is
    /// judged from the WAL frame that would heal it. Other failures (not a
    /// `.mnemo` file, a tampered header, an I/O error) are errors. Added in
    /// v0.12.2 (RB-001.23).
    pub fn key_opens(
        path: impl AsRef<std::path::Path>,
        key: &[u8; crypto::KEY_LEN],
    ) -> Result<bool> {
        let mut file = File::open(path.as_ref())?;
        let mut hbuf = [0u8; PAGE_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hbuf)?;
        let header = match Header::from_page(&hbuf) {
            Ok(h) => h,
            // A candidate frame only counts when it validates with `key`.
            Err(parse_err) => {
                return match scan_header_recovery_candidates(&mut file, key, None)? {
                    Some(_) => Ok(true),
                    None => Err(parse_err),
                }
            }
        };
        let kdf = KdfParams {
            m_cost: header.m_cost,
            t_cost: header.t_cost,
            p_cost: header.p_cost,
        };
        let kek = crypto::derive_kek(key, &header.salt, kdf)?;
        let dek = match crypto::unwrap_dek(&kek, &header.dek_nonce, &header.wrapped_dek) {
            Ok(dek) => dek,
            Err(MnemoError::WrongPassphrase) => return Ok(false),
            Err(e) => return Err(e),
        };
        header.validate_seal(&dek)?;
        Ok(true)
    }

    pub(super) fn open_read_only_secret(path: impl Into<PathBuf>, secret: &[u8]) -> Result<Mnemo> {
        let path: PathBuf = path.into();
        let file = open_with_lock(&path, /*write=*/ false, Create::No)?;
        Self::read_only_from_locked(path, file, secret)
    }

    /// [`Mnemo::open_read_only`] through 12.4's lock layer: it waits up to
    /// `opts.deadline` for a writer (a plain handle, a shared write, a
    /// compaction) instead of failing at once with `Locked`, and runs
    /// alongside other readers. The handle then holds the shared lock for
    /// its life, as any read-only handle does; close it promptly. A `Locked`
    /// after the wait names what holds the file when it left a note.
    ///
    /// Like `open_read_only`, it never recovers anything: a pending WAL or
    /// a torn header gives [`MnemoError::NeedsWriteOpen`]. Where shared
    /// mode isn't available it tries once, as `open_read_only` does.
    pub fn open_read_only_waiting(
        path: impl AsRef<Path>,
        passphrase: &str,
        opts: crate::SharedOptions,
    ) -> Result<Mnemo> {
        Self::open_read_only_waiting_secret(path.as_ref(), passphrase.as_bytes(), &opts)
            .map(passphrase_opened)
    }

    /// [`Mnemo::open_read_only_waiting`] for a raw-key file.
    pub fn open_read_only_waiting_with_key(
        path: impl AsRef<Path>,
        key: &[u8; crypto::KEY_LEN],
        opts: crate::SharedOptions,
    ) -> Result<Mnemo> {
        Self::open_read_only_waiting_secret(path.as_ref(), key, &opts)
    }

    fn open_read_only_waiting_secret(
        path: &Path,
        secret: &[u8],
        opts: &crate::SharedOptions,
    ) -> Result<Mnemo> {
        let path = absolute(path.to_path_buf());
        let file = lock_shared(&path, opts)?;
        Self::read_only_from_locked(path, file, secret)
    }

    /// The read-only open's work on a file its caller holds the shared lock
    /// on.
    fn read_only_from_locked(path: PathBuf, mut file: LockedFile, secret: &[u8]) -> Result<Mnemo> {
        let mut hbuf = [0u8; PAGE_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hbuf)?;
        // A torn header on a read-only open cannot be healed (that requires
        // writing recovered frames back). Surface as `NeedsWriteOpen` so the
        // caller knows to reopen read-write once. DUR-1 (R-002.1 part 4):
        // any `Header::from_page` failure (`BadMagic`,
        // `UnsupportedVersion`, `HeaderChecksum`) qualifies — but only
        // when the file-wide scan actually finds a valid committed
        // header frame to recover from, so a non-mnemo file still
        // reports `BadMagic` cleanly. A `validate_seal` failure is NOT
        // routed here (Option B: tampering is never auto-healed).
        let header = match Header::from_page(&hbuf) {
            Ok(h) => h,
            Err(parse_err) => {
                let recoverable = scan_header_recovery_candidates(&mut file, secret, None)?;
                return Err(match (parse_err, recoverable) {
                    // RB-004.5 item 2: as for a pending WAL (RB-003.3), a
                    // pre-v9 file can't be opened read-write, so the advice
                    // names the step that does heal it.
                    (_, Some(frames))
                        if page_zero_header(&frames).is_some_and(|h| h.version < VERSION_V9) =>
                    {
                        MnemoError::NeedsWriteOpen {
                            reason: "torn header page in a pre-v9 file; run `mnemo compact \
                                     <file>` (with `--key-file <key>` for a raw-key file), \
                                     which recovers it and converts the file to v9",
                        }
                    }
                    (_, Some(_)) => MnemoError::NeedsWriteOpen {
                        reason: "torn header page needs recovery from WAL",
                    },
                    (e, None) => e,
                });
            }
        };

        // Refuse to migrate on read-only opens. v6→v7 is header-only
        // (no page writes required), but even that bumps `version` on
        // disk and re-seals the header — which needs write access. Keep
        // the policy uniform across all sub-version bumps.
        // A v4–v7 file needs migrating, which a read-only handle can't do,
        // and since v0.13 no read-write handle opens it either: compaction
        // converts it to v9.
        if header.version < VERSION {
            return Err(MnemoError::FormatTooOld {
                version: header.version,
            });
        }

        // Refuse if there is a committed-but-uncheckpointed transaction
        // waiting for `wal::recover` to fold it into home pages.
        if wal::recover(
            &mut file,
            header.wal_start,
            header.wal_pages,
            header.wal_seq,
        )?
        .is_some()
        {
            // RB-003.3: a pre-v9 file can't be opened read-write any more,
            // so the usual advice would send the user in a circle.
            // Compaction recovers the WAL and converts the file.
            return Err(MnemoError::NeedsWriteOpen {
                reason: if header.version < VERSION_V9 {
                    "pending WAL recovery in a pre-v9 file; run `mnemo compact <file>` \
                     (with `--key-file <key>` for a raw-key file), which recovers it and \
                     converts the file to v9"
                } else {
                    "pending WAL recovery"
                },
            });
        }

        let kdf = KdfParams {
            m_cost: header.m_cost,
            t_cost: header.t_cost,
            p_cost: header.p_cost,
        };
        let kek = crypto::derive_kek(secret, &header.salt, kdf)?;
        let dek = crypto::unwrap_dek(&kek, &header.dek_nonce, &header.wrapped_dek)?;
        header.validate_seal(&dek)?;

        let mut pager = Pager::new(file, dek, header.write_counter, header.version);
        let dimensions = header.dimensions as usize;

        let catalog = load_catalog(
            &mut pager,
            header.catalog_start,
            header.catalog_pages,
            header.catalog_len,
            header.catalog_gen,
        )?;
        // FMT-9c.2: replay the v9 delta chain onto the base catalog.
        let (catalog, delta_chain, delta_rows) =
            apply_delta_chain(&mut pager, catalog, header.catalog_delta)?;
        let index = build_id_index(&catalog);
        // PERF-1 step 3: the ANN index loads on first use, not at open.
        let ann: Option<IvfPqIndex> = None;
        let ann_unloaded = header.index_pages > 0;
        let manifest = load_manifest(
            &mut pager,
            header.manifest_start,
            header.manifest_pages,
            header.manifest_len,
            header.manifest_gen,
        )?;
        let cache_dir = load_cache_directory(
            &mut pager,
            header.cache_start,
            header.cache_pages,
            header.cache_len,
            header.cache_gen,
        )?;
        let cache_idx = DirectoryIndex::build(&cache_dir);
        let free = load_free_list(&mut pager, header.free)?;
        let key_slots = load_key_slots(&mut pager, header.keys)?;
        let col = VecColumn::from_catalog(&catalog);
        let recs = RecPages::from_catalog(&catalog);
        let bm25_manifest = load_bm25_manifest(&mut pager, header.bm25)?;
        let max_snapshots = if header.version >= VERSION_V9 {
            V9_DEFAULT_MAX_SNAPSHOTS
        } else {
            DEFAULT_MAX_SNAPSHOTS
        };

        Ok(Mnemo {
            pager,
            header,
            catalog,
            index,
            path: absolute(path),
            dimensions,
            kdf,
            // No mutation is possible; leave both dirty flags false so
            // `close()` is a genuine no-op even if we later add an
            // auto-flush on drop.
            dirty_catalog: false,
            ann,
            dirty_index: false,
            index_persist: IndexPersist::default(),
            ann_unloaded,
            manifest,
            max_snapshots,
            read_only: true,
            cache_dir,
            cache_idx,
            dirty_cache: false,
            cache_budgets: std::collections::HashMap::new(),
            cache_flush_policy: CacheFlushPolicy::default(),
            cache_batch: BatchState::default(),
            cache_counters: CounterState::default(),
            embedder: None,
            fsyncs_last_flush: 0,
            agent_set: None,
            key_index: None,
            superseded: None,
            layers: None,
            rekey_cleanup: None,
            access_delta: None,
            commit_point: None,
            lock_note: None,
            notices: Vec::new(),
            window_committed: false,
            shared: false,
            turn_compress_after: DEFAULT_TURN_COMPRESS_AFTER_SECS,
            free,
            pending_free: Vec::new(),
            free_dirty: false,
            key_slots,
            dirty_keys: false,
            gen_leased: false,
            col,
            recs,
            bm25: Bm25State {
                manifest: bm25_manifest,
                ..Bm25State::default()
            },
            kw: None,
            index_layout_override: None,
            dirty_rows: std::collections::BTreeSet::new(),
            delta_chain,
            delta_rows,
            catalog_rewrite: false,
            index_chain: Vec::new(),
            index_chain_ops: 0,
            index_rebase: false,
            index_delta_ops: None,
            snapshot_cap_explicit: false,
            direct_writes: false,
            vector_cache_budget: DEFAULT_VECTOR_CACHE_BUDGET,
        })
    }

    /// FMT-9e: compress v9 thread-turn records once they're older than
    /// `secs` (by their `created_at`). The default,
    /// [`DEFAULT_TURN_COMPRESS_AFTER_SECS`], is 7 days: recent turns stay
    /// plain, so a live session reads them at full speed, and each commit
    /// compresses up to 256 turns that have aged past the cutoff. `0`
    /// compresses every turn as it's written. Compaction always compresses
    /// every turn, and so does a vacuum that moves one. Applies to this
    /// handle only (not persisted); a no-op on v8 files, which never
    /// compress. Reading never depends on it.
    pub fn set_turn_compress_after(&mut self, secs: u64) {
        self.turn_compress_after = secs.min(i64::MAX as u64) as i64;
    }

    /// Override the manifest snapshot cap on this open handle. `0` disables
    /// the cap; any positive value keeps the most-recent `max` snapshots and
    /// drops the rest on the next flush. See [`MnemoConfig::max_snapshots`].
    pub fn set_max_snapshots(&mut self, max: usize) {
        self.max_snapshots = max;
        // FMT-9c: an explicit count is a hard cap on v9 (see
        // `apply_retention`).
        self.snapshot_cap_explicit = true;
    }

    /// 12.3: the salt in this handle's header, which names its keychain
    /// entry (the keys note, section 4).
    pub(crate) fn header_salt(&self) -> [u8; crypto::SALT_LEN] {
        self.header.salt
    }

    /// 12.3: the salt in page 0 as it is on disk now, read through this
    /// handle's own file (a second open would fail against Windows' lock).
    /// After a failed rekey it tells which salt is in force.
    pub(crate) fn disk_salt(&mut self) -> Result<[u8; crypto::SALT_LEN]> {
        let page = self.pager.read_raw(0)?;
        Ok(Header::from_page(&page)?.salt)
    }

    /// 12.3: what this handle has to tell its owner, once each: today, that
    /// a passphrase opened a file whose KDF parameters are the raw-key kind
    /// (the keys note, section 1, rule 7), with the fix. Empty otherwise.
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    /// Embedding dimensionality this database expects.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Number of live (non-deleted) memories.
    pub fn len(&self) -> usize {
        self.catalog.iter().filter(|e| !e.deleted).count()
    }

    /// True if the database holds no live memories.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // --- internal page/record helpers -------------------------------------
}

/// 12.3 (the keys note, section 1, rule 7): the notice for a file a
/// passphrase opened whose KDF parameters are the raw-key kind, if it is one.
pub(crate) fn passphrase_floor_notice(path: &Path, kdf: KdfParams) -> Option<String> {
    kdf.below_passphrase_floor().then(|| {
        format!(
            "{} is protected by a passphrase on raw-key KDF parameters ({} KiB, {} pass), \
             which makes each guess of it cheap; `mnemo rekey` or `mnemo compact` raises \
             the parameters",
            path.display(),
            kdf.m_cost,
            kdf.t_cost
        )
    })
}

/// A handle a passphrase just opened, with [`passphrase_floor_notice`] added.
fn passphrase_opened(mut db: Mnemo) -> Mnemo {
    if let Some(n) = passphrase_floor_notice(&db.path, db.kdf) {
        db.notices.push(n);
    }
    db
}
