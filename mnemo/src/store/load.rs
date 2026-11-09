//! Loading the control plane: file locking, sealed-run readers, the catalog and index delta chains, the snapshot manifest, cache directory, ANN index, free list and key slots, and the v4 catalog migration.

use super::*;

/// Open a file and take an OS advisory lock, returning `Locked { path }`
/// if the requested lock cannot be acquired. On drop of the returned
/// [`File`], the OS releases the lock. `flock`/`LockFileEx` semantics
/// (per file-descriptor on Unix, per-open-file-handle on Windows) mean
/// the lock is scoped to this handle: cloning the file (or the returned
/// `File`) shares the lock, but a fresh `open()` from any process gets a
/// distinct handle and will collide.
pub(super) fn open_with_lock(path: &Path, write: bool, create: bool) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    if write {
        opts.write(true);
    }
    if create {
        opts.create(true).truncate(true);
    }
    let file = opts.open(path)?;
    // On success the returned `Result<()>` is `Ok(())`. On conflict:
    //   * Unix `flock(2)` → `ErrorKind::WouldBlock` (EWOULDBLOCK / EAGAIN)
    //   * Windows `LockFileEx` → raw OS code 33 (ERROR_LOCK_VIOLATION)
    //     or 32 (ERROR_SHARING_VIOLATION), both surfaced by Rust as
    //     `ErrorKind::Uncategorized` because `std::io::ErrorKind`
    //     doesn't carry a portable "locked" variant.
    // Anything else is a real I/O failure (permissions, invalid handle,
    // ...) and gets bubbled up as `MnemoError::Io`.
    let result = if write {
        FileExt::try_lock_exclusive(&file)
    } else {
        FileExt::try_lock_shared(&file)
    };
    match result {
        Ok(()) => Ok(file),
        Err(e) if is_lock_conflict(&e) => Err(MnemoError::Locked {
            path: path.to_path_buf(),
        }),
        Err(e) => Err(MnemoError::Io(e)),
    }
}

/// Do two paths resolve to the same on-disk file? Used by
/// [`Mnemo::export_encrypted`] to refuse overwriting the live
/// database. On Unix we compare `(dev, ino)` via `MetadataExt`, which
/// catches hard links and symlinks. On Windows we compare canonicalized
/// paths, since there is no stable device+inode equivalent in `std`.
/// Returns `None` if either side can't be stat'd / canonicalized —
/// the caller treats that as "not the same file" so a missing `dest`
/// (the normal export case) proceeds.
pub(super) fn paths_refer_to_same_file(a: &Path, b: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let ma = std::fs::metadata(a).ok()?;
        let mb = std::fs::metadata(b).ok()?;
        Some(ma.dev() == mb.dev() && ma.ino() == mb.ino())
    }
    #[cfg(not(unix))]
    {
        let ca = std::fs::canonicalize(a).ok()?;
        let cb = std::fs::canonicalize(b).ok()?;
        Some(ca == cb)
    }
}

/// A sibling temp-file path next to `dest`, suffixed with
/// `.export-tmp`. Lives in `dest`'s own parent directory so the final
/// [`std::fs::rename`] is same-filesystem (and therefore atomic on
/// every platform mnemo supports).
pub(super) fn temp_sibling(dest: &Path) -> PathBuf {
    let mut buf = dest.as_os_str().to_os_string();
    buf.push(".export-tmp");
    PathBuf::from(buf)
}

/// Was this I/O error the OS reporting a conflicting file lock, or a
/// genuine I/O failure? Platform-conditional because
/// [`std::io::ErrorKind`] carries no portable "resource-locked" variant
/// as of Rust 1.75 — Windows surfaces `ERROR_LOCK_VIOLATION` /
/// `ERROR_SHARING_VIOLATION` as `Uncategorized`.
pub(super) fn is_lock_conflict(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    if let Some(code) = e.raw_os_error() {
        // 33 = ERROR_LOCK_VIOLATION, 32 = ERROR_SHARING_VIOLATION —
        // both mean "someone else holds an incompatible lock on this
        // region of the file". See
        // https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--0-499-
        if code == 33 || code == 32 {
            return true;
        }
    }
    false
}

/// Read a run of consecutive encrypted pages and concatenate their plaintext.
/// `gen` is the generation the run's pointer expects (v9; `0` before).
pub(super) fn read_run_bytes(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(len as usize);
    for i in 0..pages {
        buf.extend_from_slice(&pager.read_page(start + i, gen)?);
    }
    buf.truncate(len as usize);
    Ok(buf)
}

/// FMT-9b: a page run read as a byte stream, one page at a time and
/// without caching it, so loading a large run (a 100K catalog is several
/// MB, its index about 17 MB) never holds the whole serialized run in
/// memory next to the structure being decoded from it.
pub(super) struct RunReader<'a> {
    pub(super) pager: &'a mut Pager,
    /// The first page error, kept so the caller reports it as itself
    /// (e.g. `PageAuthFailed`) rather than as a decode error.
    pub(super) err: &'a std::cell::RefCell<Option<MnemoError>>,
    pub(super) next_page: u64,
    pub(super) end_page: u64,
    pub(super) gen: u64,
    pub(super) remaining: u64,
    pub(super) buf: Vec<u8>,
    pub(super) pos: usize,
}

impl<'a> RunReader<'a> {
    pub(super) fn new(
        pager: &'a mut Pager,
        err: &'a std::cell::RefCell<Option<MnemoError>>,
        start: u64,
        pages: u64,
        len: u64,
        gen: u64,
    ) -> Self {
        RunReader {
            pager,
            err,
            next_page: start,
            end_page: start + pages,
            gen,
            remaining: len,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for RunReader<'_> {
    /// The decoder reads a byte or a few at a time; serve those straight
    /// from the current page.
    #[inline]
    fn read_exact(&mut self, out: &mut [u8]) -> std::io::Result<()> {
        if self.buf.len() - self.pos >= out.len() {
            out.copy_from_slice(&self.buf[self.pos..self.pos + out.len()]);
            self.pos += out.len();
            return Ok(());
        }
        let mut done = 0;
        while done < out.len() {
            match self.read(&mut out[done..])? {
                0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                n => done += n,
            }
        }
        Ok(())
    }

    #[inline]
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos == self.buf.len() {
            if self.remaining == 0 || self.next_page >= self.end_page {
                return Ok(0);
            }
            let mut page = match self.pager.read_page_uncached(self.next_page, self.gen) {
                Ok(p) => p,
                Err(e) => {
                    let msg = e.to_string();
                    *self.err.borrow_mut() = Some(e);
                    return Err(std::io::Error::other(msg));
                }
            };
            self.next_page += 1;
            page.truncate((self.remaining as usize).min(page.len()));
            self.remaining -= page.len() as u64;
            self.buf = page;
            self.pos = 0;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// FMT-9b: deserialize a sequence into a `Vec` sized from the encoded
/// length up front (bounded by `cap`, so a hostile length can't force a
/// huge allocation). serde's own `Vec` impl pre-sizes only up to 1 MiB and
/// then doubles, which for a 100K catalog leaves a quarter of the capacity
/// unused and a freed half-size copy behind in the allocator.
pub(super) struct ExactVec<T> {
    pub(super) cap: usize,
    pub(super) _t: std::marker::PhantomData<T>,
}

impl<'de, T: Deserialize<'de>> serde::de::DeserializeSeed<'de> for ExactVec<T> {
    type Value = Vec<T>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Vec<T>, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for ExactVec<T> {
    type Value = Vec<T>;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a sequence")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Vec<T>, A::Error> {
        let mut v = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(self.cap));
        while let Some(x) = seq.next_element()? {
            v.push(x);
        }
        Ok(v)
    }
}

/// Load a v5+ catalog run (an empty `pages` yields an empty catalog).
pub(super) fn load_catalog(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
) -> Result<Vec<CatalogEntry>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    // Sized exactly (every row encodes to at least 8 bytes, which bounds
    // the up-front allocation by the run's length). v8 decodes from one
    // buffer, as before; v9 streams the run (it's larger, and it's paired
    // with a resident vector column, so the transient buffer costs RSS).
    let cap = (len / 8) as usize;
    if pager.version() < VERSION_V9 {
        let buf = read_run_bytes(pager, start, pages, len, gen)?;
        let mut de = rmp_serde::Deserializer::from_read_ref(&buf);
        return serde::de::DeserializeSeed::deserialize(
            ExactVec::<CatalogEntry> {
                cap,
                _t: std::marker::PhantomData,
            },
            &mut de,
        )
        .map_err(|e| MnemoError::Serialize(e.to_string()));
    }
    let err = std::cell::RefCell::new(None);
    let mut de = rmp_serde::Deserializer::new(RunReader::new(pager, &err, start, pages, len, gen));
    let res = serde::de::DeserializeSeed::deserialize(
        ExactVec::<CatalogEntry> {
            cap,
            _t: std::marker::PhantomData,
        },
        &mut de,
    );
    drop(de);
    match (res, err.into_inner()) {
        (Ok(v), _) => Ok(v),
        (Err(_), Some(page_err)) => Err(page_err),
        (Err(e), None) => Err(MnemoError::Serialize(e.to_string())),
    }
}

/// FMT-9c.2: one delta run of a v9 catalog: the rows a commit changed or
/// added, by position, and the previous delta in the chain.
#[derive(Serialize, Deserialize)]
pub(super) struct CatalogDelta {
    pub(super) prev: (u64, u64, u64, u64),
    pub(super) rows: Vec<(u32, CatalogEntry)>,
}

pub(super) fn ptr_tuple(p: RunPtr) -> (u64, u64, u64, u64) {
    (p.start, p.pages, p.len, p.gen)
}

pub(super) fn tuple_ptr(t: (u64, u64, u64, u64)) -> RunPtr {
    RunPtr {
        start: t.0,
        pages: t.1,
        len: t.2,
        gen: t.3,
    }
}

/// FMT-9c.2: walk the delta chain from `head` back to the base and replay
/// it onto `catalog`, oldest first. Returns the catalog, the chain (oldest
/// first) and its total row count. An empty `head` is a no-op (and v8).
pub(super) fn apply_delta_chain(
    pager: &mut Pager,
    mut catalog: Vec<CatalogEntry>,
    head: RunPtr,
) -> Result<(Vec<CatalogEntry>, Vec<RunPtr>, usize)> {
    let mut deltas: Vec<(RunPtr, Vec<(u32, CatalogEntry)>)> = Vec::new();
    let mut at = head;
    while !at.is_empty() {
        if deltas.len() > 4096 {
            return Err(MnemoError::Invalid("catalog delta chain too long".into()));
        }
        let buf = read_run_bytes(pager, at.start, at.pages, at.len, at.gen)?;
        let d: CatalogDelta =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        deltas.push((at, d.rows));
        at = tuple_ptr(d.prev);
    }
    deltas.reverse();
    let mut rows = 0usize;
    let mut chain = Vec::with_capacity(deltas.len());
    for (ptr, d) in deltas {
        rows += d.len();
        chain.push(ptr);
        for (i, row) in d {
            let i = i as usize;
            match i.cmp(&catalog.len()) {
                std::cmp::Ordering::Less => catalog[i] = row,
                std::cmp::Ordering::Equal => catalog.push(row),
                std::cmp::Ordering::Greater => {
                    return Err(MnemoError::Invalid("catalog delta skips a row".into()))
                }
            }
        }
    }
    Ok((catalog, chain, rows))
}

/// FMT-9c.3: one delta run of a v9 ANN index: the changes a commit made,
/// and the previous delta in the chain.
#[derive(Serialize)]
pub(super) struct IndexDeltaRef<'a> {
    pub(super) prev: (u64, u64, u64, u64),
    pub(super) ops: &'a [IndexOp],
}

#[derive(Deserialize)]
pub(super) struct IndexDeltaOwned {
    pub(super) prev: (u64, u64, u64, u64),
    pub(super) ops: Vec<IndexOp>,
}

/// FMT-9c.3: walk the index delta chain from `head` and replay it onto a
/// just-loaded base index, oldest first. Returns the chain and its change
/// count.
pub(super) fn apply_index_chain(
    pager: &mut Pager,
    ann: &mut IvfPqIndex,
    head: RunPtr,
) -> Result<(Vec<RunPtr>, usize)> {
    let mut deltas: Vec<(RunPtr, Vec<IndexOp>)> = Vec::new();
    let mut at = head;
    while !at.is_empty() {
        if deltas.len() > 4096 {
            return Err(MnemoError::Invalid("index delta chain too long".into()));
        }
        let buf = read_run_bytes(pager, at.start, at.pages, at.len, at.gen)?;
        let d: IndexDeltaOwned =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        deltas.push((at, d.ops));
        at = tuple_ptr(d.prev);
    }
    deltas.reverse();
    let mut chain = Vec::with_capacity(deltas.len());
    let mut n = 0usize;
    for (ptr, ops) in deltas {
        n += ops.len();
        chain.push(ptr);
        ann.apply_ops(ops)?;
    }
    Ok((chain, n))
}

/// FMT-9f: the runs of an index delta chain whose base didn't load (a
/// layout this build doesn't know), oldest first, without replaying them,
/// so the next `build_index` or `drop_index` frees them with the base.
/// Best effort: a delta this build can't decode ends the walk, and the
/// runs before it stay unreferenced until the next compaction.
pub(super) fn index_chain_runs(pager: &mut Pager, head: RunPtr) -> Vec<RunPtr> {
    let mut runs = Vec::new();
    let mut at = head;
    while !at.is_empty() && runs.len() <= 4096 {
        let Ok(buf) = read_run_bytes(pager, at.start, at.pages, at.len, at.gen) else {
            break;
        };
        let Ok(d) = rmp_serde::from_slice::<IndexDeltaOwned>(&buf) else {
            break;
        };
        runs.push(at);
        at = tuple_ptr(d.prev);
    }
    runs.reverse();
    runs
}

/// Load a **v4** catalog run using the frozen [`CatalogEntryV4`] shape.
/// Used only by the v4→v5 migration path in [`Mnemo::open`].
pub(super) fn load_catalog_v4(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
) -> Result<Vec<CatalogEntryV4>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len, 0)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Walk a v4 catalog and synthesize the v5 shape. For each live entry,
/// reads the memory's serialized body and copies its `accessed_at` and
/// `access_count` into the catalog row (the v4 record body still carries
/// them — they're moving *from* the body *to* the catalog). Deleted
/// entries skip the read and zero the fields.
pub(super) fn migrate_v4_catalog(
    pager: &mut Pager,
    v4: Vec<CatalogEntryV4>,
) -> Result<Vec<CatalogEntry>> {
    let mut out = Vec::with_capacity(v4.len());
    for e in &v4 {
        let (accessed_at, access_count) = if e.deleted {
            (0i64, 0u32)
        } else {
            let mut buf = Vec::with_capacity(e.len as usize);
            for i in 0..e.page_count as u64 {
                buf.extend_from_slice(&pager.read_page(e.start_page + i, 0)?);
            }
            buf.truncate(e.len as usize);
            let mem: Memory = rmp_serde::from_slice(&buf)
                .map_err(|err| MnemoError::Serialize(err.to_string()))?;
            (mem.accessed_at, mem.access_count)
        };
        out.push(CatalogEntry {
            id: e.id,
            start_page: e.start_page,
            page_count: e.page_count,
            len: e.len,
            deleted: e.deleted,
            accessed_at,
            access_count,
            gen: 0,
            v9: None,
        });
    }
    Ok(out)
}

/// Load the snapshot manifest (an empty `pages` yields an empty manifest).
pub(super) fn load_manifest(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
) -> Result<Vec<Snapshot>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len, gen)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Load the result-cache directory (v8+). An empty `pages` yields an
/// empty directory, which is the natural state for v7 files migrated
/// to v8 (no cache entries yet) and for freshly created v8 files.
pub(super) fn load_cache_directory(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
) -> Result<Vec<CacheDirectoryEntry>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len, gen)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Load an ANN index run, validating its dimensionality.
pub(super) fn load_index(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
    dims: usize,
) -> Result<Option<IvfPqIndex>> {
    if pages == 0 {
        return Ok(None);
    }
    // v8 decodes from one buffer, as before. v9 streams the run (a 100K
    // index is ~17 MB), trading decode speed for not holding it twice.
    if pager.version() < VERSION_V9 {
        let buf = read_run_bytes(pager, start, pages, len, gen)?;
        let mut idx: IvfPqIndex =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        if idx.dims() != dims {
            return Err(MnemoError::Invalid(
                "ANN index dimensionality does not match the database".into(),
            ));
        }
        idx.rebuild_assignment();
        return Ok(Some(idx));
    }
    // v9 index runs start with a layout byte. An index is derived data, so
    // a layout this build doesn't know means "no index" (recall scans
    // exactly; `rebuild_index` writes a current one), never an error.
    let err = std::cell::RefCell::new(None);
    let mut rd = RunReader::new(pager, &err, start, pages, len, gen);
    let mut layout = [0u8; 1];
    if rd.read_exact(&mut layout).is_err() {
        drop(rd);
        return match err.into_inner() {
            Some(page_err) => Err(page_err),
            None => Ok(None),
        };
    }
    if layout[0] != V9_INDEX_LAYOUT {
        return Ok(None);
    }
    let res: std::result::Result<IvfPqIndex, _> = rmp_serde::from_read(rd);
    let mut idx = match (res, err.into_inner()) {
        (Ok(i), _) => i,
        (Err(_), Some(page_err)) => return Err(page_err),
        (Err(e), None) => return Err(MnemoError::Serialize(e.to_string())),
    };
    if idx.dims() != dims {
        return Err(MnemoError::Invalid(
            "ANN index dimensionality does not match the database".into(),
        ));
    }
    idx.rebuild_assignment();
    Ok(Some(idx))
}

/// FMT-9 (a): load the v9 free list (an empty pointer yields an empty list).
pub(super) fn load_free_list(pager: &mut Pager, ptr: RunPtr) -> Result<FreeList> {
    if ptr.is_empty() {
        return Ok(FreeList::default());
    }
    let buf = read_run_bytes(pager, ptr.start, ptr.pages, ptr.len, ptr.gen)?;
    let runs: Vec<crate::freelist::FreeRun> =
        rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
    FreeList::from_runs(runs)
}

/// FMT-9 (b): load the v9 key-slot run (an empty pointer yields no slots).
pub(super) fn load_key_slots(pager: &mut Pager, ptr: RunPtr) -> Result<KeySlotRun> {
    if ptr.is_empty() {
        return Ok(KeySlotRun::default());
    }
    let buf = read_run_bytes(pager, ptr.start, ptr.pages, ptr.len, ptr.gen)?;
    let run: KeySlotRun =
        rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
    run.validate()?;
    Ok(run)
}
