//! The pager: encrypted, page-granular I/O over the database file.
//!
//! The pager owns the file handle and the data-encryption-key (DEK). Callers
//! deal in *plaintext payloads*; the pager transparently encrypts on write and
//! decrypts on read. Decrypted pages are kept in a **bounded LRU cache**
//! ([`crate::cache::PageCache`]) so repeated reads skip a decrypt and writes
//! are batched until [`Pager::write_dirty`]. The cache caps retained *clean* pages;
//! dirty (un-flushed) pages are pinned until a flush makes them clean.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

/// Test-only crash injection hooks (Phase 1.3). Compiled in under
/// `cfg(test)` and the `failpoints` feature; otherwise absent — the
/// production build carries no runtime cost. The counter is
/// thread-local so parallel tests do not stomp on each other.
#[cfg(any(test, feature = "failpoints"))]
pub(crate) mod failpoints {
    use std::cell::Cell;

    thread_local! {
        // -1  => disabled (default).
        // N>0 => Nth subsequent `check()` returns Ok, then check
        //        `N+1` fails and disarms.
        // 0   => the *next* `check()` fails, then disarms.
        //
        // `const { ... }` in `thread_local!` was stabilized in Rust
        // 1.59, safely under our 1.75 MSRV floor. Const-init avoids
        // the first-touch branch and satisfies clippy's
        // `missing_const_for_thread_local` on stable.
        static WRITES_UNTIL_FAIL: Cell<i64> = const { Cell::new(-1) };
        // How many writes in a row fail once the counter fires (12.4's
        // MaybeStored test fails the checkpoint and then the recovery).
        static FAILS_LEFT: Cell<u32> = const { Cell::new(1) };
        // REKEY-2 (RB-005.4 item 5): writes counted from the last
        // `set_writes_failing` (or from the thread's start), and the
        // write numbers in that count that fail.
        static WRITES_SEEN: Cell<u64> = const { Cell::new(0) };
        static FAILING: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    // v0.19.1 (task v0191-1b): reads of one page fail, `count` times, in
    // any thread (the batch reads decrypt and stream on helper threads).
    // Process-wide, so a test that arms it keeps a binary of its own.
    static READ_FAIL_PAGE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(u64::MAX);
    static READ_FAILS_LEFT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// Make the next `count` reads that cover page `page` fail with a
    /// synthetic `io::Error`, in any thread. `count = 0` disarms it.
    pub fn set_page_read_fails(page: u64, count: u32) {
        use std::sync::atomic::Ordering::SeqCst;
        READ_FAILS_LEFT.store(count, SeqCst);
        READ_FAIL_PAGE.store(if count == 0 { u64::MAX } else { page }, SeqCst);
    }

    /// Called before every read of pages `first..first + n`.
    pub(crate) fn check_read(first: u64, n: u64) -> std::io::Result<()> {
        use std::sync::atomic::Ordering::SeqCst;
        let page = READ_FAIL_PAGE.load(SeqCst);
        if page == u64::MAX || page < first || page >= first + n {
            return Ok(());
        }
        // A compare-exchange loop: `fetch_update` is deprecated on newer
        // toolchains and its rename `try_update` is newer than the MSRV.
        let mut left = READ_FAILS_LEFT.load(SeqCst);
        loop {
            if left == 0 {
                return Ok(());
            }
            match READ_FAILS_LEFT.compare_exchange(left, left - 1, SeqCst, SeqCst) {
                Ok(_) => break,
                Err(now) => left = now,
            }
        }
        if left == 1 {
            READ_FAIL_PAGE.store(u64::MAX, SeqCst);
        }
        Err(std::io::Error::other("injected read failure"))
    }

    /// An event in the I/O trace ([`set_io_trace`]): a page written, the
    /// WAL's commit written, or a sync.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum IoEvent {
        /// A page written at its home location (data, a direct run, a
        /// checkpointed frame, the header).
        Page(u64),
        /// A WAL transaction (its frames and commit frame) written.
        WalCommit,
        /// A `sync_all`.
        Sync,
    }

    thread_local! {
        static TRACE: std::cell::RefCell<Option<Vec<IoEvent>>> =
            const { std::cell::RefCell::new(None) };
        // -1 => disabled; N >= 0 => the Nth sync from now fails, once.
        static SYNCS_UNTIL_FAIL: Cell<i64> = const { Cell::new(-1) };
    }

    /// PERF-2 3f: start (`true`, emptying it) or stop recording this
    /// thread's writes and syncs.
    pub fn set_io_trace(on: bool) {
        TRACE.with(|t| *t.borrow_mut() = on.then(Vec::new));
    }

    /// The events recorded since [`set_io_trace`]`(true)`.
    pub fn io_trace() -> Vec<IoEvent> {
        TRACE.with(|t| t.borrow().clone().unwrap_or_default())
    }

    pub(crate) fn trace(ev: IoEvent) {
        TRACE.with(|t| {
            if let Some(v) = t.borrow_mut().as_mut() {
                v.push(ev);
            }
        });
    }

    /// PERF-2 3f: the Nth `sync_all` from now (`0` is the next) fails
    /// without syncing, once; `-1` disarms it. Covers the pager's syncs
    /// and the WAL commit's.
    pub fn set_syncs_until_fail(n: i64) {
        SYNCS_UNTIL_FAIL.with(|c| c.set(n));
    }

    pub(crate) fn check_sync() -> std::io::Result<()> {
        SYNCS_UNTIL_FAIL.with(|c| match c.get() {
            0 => {
                c.set(-1);
                Err(std::io::Error::other("injected sync failure"))
            }
            v if v > 0 => {
                c.set(v - 1);
                Ok(())
            }
            _ => Ok(()),
        })
    }

    /// Fail exactly the writes numbered in `ns`, counting from 0 at this
    /// call (each write the pager or the WAL makes is one number). Unlike
    /// [`set_writes_until_fail_n`], the failing writes needn't be
    /// consecutive. An empty set disarms it. Independent of
    /// [`set_writes_until_fail`].
    pub fn set_writes_failing(ns: &[u64]) {
        WRITES_SEEN.with(|c| c.set(0));
        FAILING.with(|f| *f.borrow_mut() = ns.to_vec());
    }

    /// How many writes this thread has made since the last
    /// [`set_writes_failing`].
    pub fn writes_seen() -> u64 {
        WRITES_SEEN.with(|c| c.get())
    }

    /// Arm the pager (and the WAL commit writer) to fail its Nth
    /// subsequent write with a synthetic `io::Error`. `n = 0` fails
    /// on the very next write; `n = -1` disables. One-shot: the
    /// counter disarms itself the moment it fires.
    pub fn set_writes_until_fail(n: i64) {
        WRITES_UNTIL_FAIL.with(|c| c.set(n));
        FAILS_LEFT.with(|c| c.set(1));
    }

    /// Like [`set_writes_until_fail`], but `count` writes in a row fail
    /// once the counter fires, then it disarms.
    pub fn set_writes_until_fail_n(n: i64, count: u32) {
        WRITES_UNTIL_FAIL.with(|c| c.set(n));
        FAILS_LEFT.with(|c| c.set(count.max(1)));
    }

    /// Called by every file-writing code path in the pager and the
    /// WAL commit. Returns `Err` when the counter reaches zero,
    /// simulating a torn write mid-flush.
    pub(crate) fn check() -> std::io::Result<()> {
        let n = WRITES_SEEN.with(|c| {
            let n = c.get();
            c.set(n + 1);
            n
        });
        if FAILING.with(|f| f.borrow().contains(&n)) {
            return Err(std::io::Error::other(
                "injected crash (REKEY-2 failpoint set)",
            ));
        }
        WRITES_UNTIL_FAIL.with(|c| {
            let v = c.get();
            if v == 0 {
                let left = FAILS_LEFT.with(|f| f.get());
                if left > 1 {
                    FAILS_LEFT.with(|f| f.set(left - 1));
                } else {
                    c.set(-1);
                }
                Err(std::io::Error::other(
                    "injected crash (Phase 1.3 failpoint)",
                ))
            } else if v > 0 {
                c.set(v - 1);
                Ok(())
            } else {
                Ok(())
            }
        })
    }
}

/// Zero-cost stub used everywhere the failpoint is compiled out.
/// Kept as an `#[inline(always)] fn` so the call site reads the
/// same in both configurations; the branch is optimized away.
///
/// Also re-exported to sibling modules (`wal`) that need the same
/// hook on their own `write_all` sites — the double-underscore
/// makes it obvious this is not a stable API.
#[cfg(not(any(test, feature = "failpoints")))]
#[inline(always)]
pub(crate) fn __failpoint_check() -> std::io::Result<()> {
    Ok(())
}

#[cfg(any(test, feature = "failpoints"))]
#[inline(always)]
pub(crate) fn __failpoint_check() -> std::io::Result<()> {
    failpoints::check()
}

/// PERF-2 3f: the sync failpoint ([`failpoints::set_syncs_until_fail`]),
/// checked before every `sync_all` of a commit.
#[cfg(not(any(test, feature = "failpoints")))]
#[inline(always)]
pub(crate) fn __sync_failpoint() -> std::io::Result<()> {
    Ok(())
}

#[cfg(any(test, feature = "failpoints"))]
#[inline(always)]
pub(crate) fn __sync_failpoint() -> std::io::Result<()> {
    failpoints::check_sync()
}

/// PERF-2 3f: the I/O trace ([`failpoints::set_io_trace`]).
#[cfg(not(any(test, feature = "failpoints")))]
#[inline(always)]
pub(crate) fn __io_trace(_ev: IoEvent) {}

#[cfg(any(test, feature = "failpoints"))]
#[inline(always)]
pub(crate) fn __io_trace(ev: IoEvent) {
    failpoints::trace(ev)
}

#[cfg(any(test, feature = "failpoints"))]
pub(crate) use failpoints::IoEvent;

/// What the trace records; without the failpoints it's never built.
#[cfg(not(any(test, feature = "failpoints")))]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum IoEvent {
    Page(u64),
    WalCommit,
    Sync,
}

/// v0.19.1: the read-side failpoint ([`failpoints::set_page_read_fails`]),
/// checked before every read of pages `first..first + n`.
#[cfg(not(any(test, feature = "failpoints")))]
#[inline(always)]
fn read_failpoint(_first: u64, _n: u64) -> std::io::Result<()> {
    Ok(())
}

#[cfg(any(test, feature = "failpoints"))]
#[inline(always)]
fn read_failpoint(first: u64, n: u64) -> std::io::Result<()> {
    failpoints::check_read(first, n)
}

use zeroize::Zeroizing;

use crate::cache::PageCache;
use crate::crypto::{self, KEY_LEN, NONCE_LEN};
use crate::error::{MnemoError, Result};
use crate::format::{PAGE_SIZE, PAYLOAD};

/// PERF-2 (v0.19.0): below this many pages, [`Pager::read_pages_uncached`]
/// and [`Pager::for_each_page`] decrypt on the calling thread; at or above
/// it, on scoped threads.
pub const PARALLEL_DECRYPT_MIN_PAGES: usize = 64;

/// PERF-2 (v0.19.0): the pages one [`Pager::for_each_page`] thread reads and
/// decrypts per buffer: 15 pages is 120 KiB, under the size at which the
/// macOS allocator starts keeping freed blocks for reuse, so a handle's
/// memory doesn't grow with a large fill.
const STREAM_PAGES: usize = 15;

/// PERF-2 (v0.19.0): the plaintext of a batch of pages
/// ([`Pager::read_pages_uncached`]): the pages read from disk, decrypted in
/// place in one buffer, and the cached ones as they were.
pub(crate) struct PageBatch {
    disk: Vec<u8>,
    slots: Vec<Slot>,
}

enum Slot {
    /// The page's index in the decrypted disk buffer.
    Disk(usize),
    /// A cached (possibly dirty) copy.
    Cached(Vec<u8>),
}

impl PageBatch {
    /// The `i`th requested page's plaintext ([`PAYLOAD`] bytes).
    pub(crate) fn payload(&self, i: usize) -> &[u8] {
        match &self.slots[i] {
            Slot::Disk(j) => {
                let at = j * PAGE_SIZE + NONCE_LEN;
                &self.disk[at..at + PAYLOAD]
            }
            Slot::Cached(v) => v,
        }
    }

    /// How many pages the batch holds.
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// PERF-2: the batch as one buffer of page images, page `i` at `i *
    /// PAGE_SIZE` with its plaintext at `NONCE_LEN..NONCE_LEN + PAYLOAD`:
    /// the decrypted read buffer itself when every page came from disk in
    /// the order asked, a copy laid out the same way otherwise (cached
    /// pages, or pages asked out of order).
    pub(crate) fn into_images(self) -> Vec<u8> {
        let in_place = self
            .slots
            .iter()
            .enumerate()
            .all(|(i, s)| matches!(s, Slot::Disk(j) if *j == i));
        if in_place {
            return self.disk;
        }
        let mut out = vec![0u8; self.slots.len() * PAGE_SIZE];
        for i in 0..self.slots.len() {
            let at = i * PAGE_SIZE + NONCE_LEN;
            out[at..at + PAYLOAD].copy_from_slice(self.payload(i));
        }
        out
    }
}

/// A positioned read of exactly `buf.len()` bytes at `offset`. It doesn't
/// use the file's cursor, which every other read and write here sets
/// itself first.
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    read_failpoint(
        offset / PAGE_SIZE as u64,
        (buf.len() as u64).div_ceil(PAGE_SIZE as u64),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            match file.seek_read(&mut buf[done..], offset + done as u64) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "failed to fill whole buffer",
                    ))
                }
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// The database file with the advisory lock its opener took, released
/// explicitly when this is dropped.
///
/// `flock` locks belong to the open file description, not to one file
/// descriptor. While another thread of the process spawns a child, the
/// child holds a copy of the parent's file table until it `exec`s (the
/// descriptor is close-on-exec, but that only takes effect at the exec).
/// Without the explicit unlock, a handle dropped inside that window left
/// the lock held by the child's copy, and the next open of the file failed
/// with `Locked` (FLAKE-1). Unlocking releases it whatever copies exist.
/// On Windows the unlock is equally harmless: handles aren't inherited.
pub(crate) struct LockedFile(File);

impl LockedFile {
    /// Wraps a file whose lock the caller has just taken.
    pub(crate) fn new(file: File) -> Self {
        Self(file)
    }
}

impl std::ops::Deref for LockedFile {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}

impl std::ops::DerefMut for LockedFile {
    fn deref_mut(&mut self) -> &mut File {
        &mut self.0
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        // Nothing to do on failure: closing the file releases the lock once
        // the last copy of the description goes.
        let _ = fs4::fs_std::FileExt::unlock(&self.0);
    }
}

/// Default page-cache capacity, in pages (~8 MiB of decrypted payloads).
/// PERF-1 step 4 lowered it from 8192 (~64 MiB): at 10K and 100K memories
/// that cache was most of an open handle's RSS, for little hit rate on
/// recall's scattered record reads. `Mnemo::set_cache_capacity` raises it.
pub const DEFAULT_CACHE_PAGES: usize = 1024;

/// FMT-9b: the default page cache for a v9 file, 256 pages (~2 MiB). v9
/// recall scores from the catalog rows and the resident vector column and
/// decrypts only the `top_k` hits, so a large cache of record pages buys
/// little; `Mnemo::set_cache_capacity` still raises it.
pub const V9_DEFAULT_CACHE_PAGES: usize = 256;

/// What a shared-mode handle's pager reports if anything reaches the file
/// between calls (it holds none then).
const DETACHED: &str = "the brain's file isn't open outside a shared-mode call";

/// Encrypted page reader/writer with a bounded write-back cache.
pub struct Pager {
    /// The brain's file. A shared-mode handle (12.4) holds it only inside a
    /// call: [`Pager::attach_file`] at the start, [`Pager::detach_file`] at
    /// the end, so no descriptor stays open between calls.
    file: Option<LockedFile>,
    dek: Zeroizing<[u8; KEY_LEN]>,
    /// Monotonic write counter persisted in the header (lease / replay
    /// machinery; see `Mnemo::prepare_for_flush`). **No longer used in
    /// the page nonce** — SEC-1 (v0.5.2) moved page nonces to a
    /// per-session random prefix + per-session counter so copies of a
    /// file can't collide on `(page_no, write_counter)` under the same
    /// DEK. Still bumped per write for the existing lease machinery.
    pub write_counter: u64,
    /// Random 64-bit "fixed field" (NIST SP 800-38D §8.2.1) stamped
    /// into bytes 0..8 of every page nonce this pager produces. Drawn
    /// fresh on every writable open and regenerated on counter wrap —
    /// so cross-session nonce collisions require two independent
    /// sessions to draw the same 64-bit random value (birthday bound
    /// ~2^32). Unused on read-only handles.
    session_random: u64,
    /// Per-session monotonic counter stamped into bytes 8..12 of every
    /// page nonce. Starts at 0, increments once per encrypted page
    /// write via [`Pager::next_nonce`]. If about to overflow `u32::MAX`,
    /// [`Pager::next_nonce`] regenerates `session_random` and resets to 0
    /// so the (random, counter) pair stays unique within the session.
    session_counter: u32,
    /// Format version this pager is currently encrypting *for*. `Pager` is
    /// version-aware because the format-bump in v6 changed page encryption
    /// to bind each page's home `page_no` as AAD on AES-GCM, making
    /// page-swap attacks tamper-evident. The pager picks the right AAD
    /// scheme per version on every encrypt/decrypt. During a v5→v6
    /// migration the pager flips from 5 to 6 mid-flight via
    /// [`Pager::set_version`].
    version: u16,
    /// Bounded LRU cache of decrypted page payloads, keyed by page and
    /// generation (v0200-3). A dirty page is sealed under the generation it
    /// was written with (v9 binds it into the page AAD).
    cache: PageCache,
    /// PERF-2 3f (v0.20.0): pages were written since the last
    /// [`Pager::sync`]. Set by every page write, cleared only by a sync,
    /// so a commit that failed after writing pages still syncs on its
    /// retry. The data pages it wrote are written again too (RB-016.1).
    unsynced: bool,
}

impl Pager {
    /// Wrap an open file with a pager. `write_counter` comes from the header.
    pub(crate) fn new(
        file: LockedFile,
        dek: Zeroizing<[u8; KEY_LEN]>,
        write_counter: u64,
        version: u16,
    ) -> Self {
        Self {
            file: Some(file),
            dek,
            write_counter,
            // Fresh 64-bit random per Pager construction = per writable
            // open. See SEC-1 doc on `crypto::session_page_nonce`.
            session_random: crypto::random_u64(),
            session_counter: 0,
            version,
            cache: PageCache::new(if version >= crate::format::VERSION_V9 {
                V9_DEFAULT_CACHE_PAGES
            } else {
                DEFAULT_CACHE_PAGES
            }),
            unsynced: false,
        }
    }

    /// Mint the next page nonce for this session.
    ///
    /// Increments `session_counter` and returns
    /// `session_random || session_counter`. If the counter would overflow
    /// `u32::MAX`, regenerates `session_random` and resets the counter
    /// before incrementing — so no `(session_random, session_counter)`
    /// pair ever repeats within this session.
    fn next_nonce(&mut self) -> [u8; NONCE_LEN] {
        if self.session_counter == u32::MAX {
            // Wrap guard: pick a new 64-bit prefix and reset the counter
            // so the next write uses `(fresh_random, 1)` instead of
            // reusing `(old_random, 0)`.
            self.session_random = crypto::random_u64();
            self.session_counter = 0;
        }
        self.session_counter += 1;
        crypto::session_page_nonce(self.session_random, self.session_counter)
    }

    /// Switch the pager to a new format version. Used by [`crate::Mnemo::open`]
    /// during a migration that needs to read with the old version's AAD
    /// scheme and write with the new one.
    pub(crate) fn set_version(&mut self, version: u16) {
        self.version = version;
    }

    /// AAD bound to an encrypted page under this pager's version. Returns a
    /// fresh `Vec` each call to keep the `Aead::Payload` lifetime simple at
    /// every call site. Empty for v4/v5 (matching the historical scheme
    /// without page binding); `page_no.to_le_bytes()` from v6 onwards.
    ///
    /// **v9 (FMT-9 (a)):** `page_no || generation`, where the generation is
    /// the transaction id that wrote the page. Pages are reused in v9, so a
    /// page number alone no longer pins a ciphertext: an attacker holding
    /// an older copy of the file could put back the page's previous
    /// ciphertext, whose tag is valid for the same `page_no`. Every pointer
    /// to a page (header run pointers, catalog rows, cache-directory
    /// entries, snapshots) records the generation it expects, and a read
    /// under any other generation fails authentication.
    #[inline]
    fn page_aad(&self, page_no: u64, gen: u64) -> Vec<u8> {
        if self.version >= crate::format::VERSION_V9 {
            let mut aad = Vec::with_capacity(16);
            aad.extend_from_slice(&page_no.to_le_bytes());
            aad.extend_from_slice(&gen.to_le_bytes());
            aad
        } else if self.version >= 6 {
            page_no.to_le_bytes().to_vec()
        } else {
            Vec::new()
        }
    }

    /// v0200-3: the generation a page is cached under: the pointer's on v9,
    /// 0 before (whose page AAD has no generation, and whose pointers may
    /// not carry one).
    #[inline]
    fn cache_gen(&self, gen: u64) -> u64 {
        if self.version >= crate::format::VERSION_V9 {
            gen
        } else {
            0
        }
    }

    /// v0200-3: whether a page write is waiting for [`Pager::write_dirty`],
    /// or for a sync to cover it.
    pub(crate) fn has_dirty_pages(&self) -> bool {
        self.cache.has_dirty()
    }

    /// The format version the pager encrypts for.
    pub(crate) fn version(&self) -> u16 {
        self.version
    }

    /// Borrow the data-encryption-key (used by `rekey`, which re-wraps it).
    pub fn dek(&self) -> &[u8; KEY_LEN] {
        &self.dek
    }

    /// The underlying file (its identity is checked before each commit).
    ///
    /// Panics outside a shared-mode call, where the pager holds no file:
    /// nothing reaches the file there.
    pub fn file(&self) -> &File {
        self.file.as_deref().expect(DETACHED)
    }

    /// Mutable access to the underlying file. Used by the WAL, which writes
    /// its log frames directly rather than through the page cache. Panics
    /// like [`Pager::file`].
    pub fn file_mut(&mut self) -> &mut File {
        self.file.as_deref_mut().expect(DETACHED)
    }

    /// The file for this pager's own I/O; an error, not a panic, if a
    /// shared-mode handle reaches it between calls.
    fn f(&mut self) -> std::io::Result<&mut File> {
        self.file
            .as_deref_mut()
            .ok_or_else(|| std::io::Error::other(DETACHED))
    }

    /// 12.4: give a shared-mode handle's pager the call's locked file.
    pub(crate) fn attach_file(&mut self, file: LockedFile) {
        self.file = Some(file);
    }

    /// 12.4: take the call's locked file back at the end of the call. The
    /// caller drops it, which unlocks and closes it.
    pub(crate) fn detach_file(&mut self) -> Option<LockedFile> {
        self.file.take()
    }

    /// Resize the page cache, evicting clean pages if it shrank.
    pub fn set_cache_capacity(&mut self, pages: usize) {
        self.cache.set_capacity(pages);
    }

    /// Current page-cache occupancy and configured cap.
    pub fn cache_stats(&self) -> (usize, usize) {
        (self.cache.len(), self.cache.capacity())
    }

    /// Write a raw, *unencrypted* page-sized buffer at `page_no`.
    /// Used only for the header (page 0).
    pub fn write_raw(&mut self, page_no: u64, data: &[u8; PAGE_SIZE]) -> Result<()> {
        __failpoint_check()?;
        self.unsynced = true;
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.f()?.write_all(data)?;
        __io_trace(IoEvent::Page(page_no));
        Ok(())
    }

    /// Read a raw, *unencrypted* page-sized buffer from `page_no`.
    ///
    /// Retained as the symmetric counterpart to [`Pager::write_raw`]; the
    /// header page is currently read directly in `Mnemo::open`.
    #[allow(dead_code)]
    pub fn read_raw(&mut self, page_no: u64) -> Result<[u8; PAGE_SIZE]> {
        let mut buf = [0u8; PAGE_SIZE];
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.f()?.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// v0200-3r: drop every clean cached page sealed under a generation
    /// above `gen` (v9; a v8 pager caches everything under 0).
    pub(crate) fn drop_cached_newer_than(&mut self, gen: u64) {
        let g = self.cache_gen(gen);
        self.cache.drop_newer_than(g);
    }

    /// v0200-3r: the nonce `page_no` has on disk, read past the cache. SEC-1
    /// gives every session a random nonce prefix, so a page written again,
    /// by any process, in any copy of the file, never has the same one.
    pub(crate) fn page_nonce(&mut self, page_no: u64) -> Result<[u8; NONCE_LEN]> {
        let mut nonce = [0u8; NONCE_LEN];
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        read_failpoint(page_no, 1)?;
        self.f()?.read_exact(&mut nonce)?;
        Ok(nonce)
    }

    /// Encrypt `payload` into a finished on-disk page image **without writing
    /// it**. Advances the nonce counter and refreshes the page cache (as a
    /// clean entry) so later reads see the new content. Used by the WAL path:
    /// the returned bytes are logged to the WAL and only later copied to
    /// `page_no` by a checkpoint.
    ///
    /// `gen` is the page's generation (v9; ignored before).
    pub fn seal_page(&mut self, page_no: u64, gen: u64, payload: &[u8]) -> Result<[u8; PAGE_SIZE]> {
        if payload.len() > PAYLOAD {
            return Err(MnemoError::Invalid(format!(
                "payload {} exceeds page capacity {}",
                payload.len(),
                PAYLOAD
            )));
        }
        let mut padded = vec![0u8; PAYLOAD];
        padded[..payload.len()].copy_from_slice(payload);

        // Bump the header's lease counter (unchanged — still consumed by
        // `prepare_for_flush`); mint the actual nonce from the per-session
        // random + counter pair (SEC-1).
        self.write_counter += 1;
        let nonce = self.next_nonce();
        let aad = self.page_aad(page_no, gen);
        let ciphertext = crypto::aead_encrypt(&self.dek, &nonce, &padded, &aad)?;

        let mut disk = [0u8; PAGE_SIZE];
        disk[..NONCE_LEN].copy_from_slice(&nonce);
        disk[NONCE_LEN..].copy_from_slice(&ciphertext);

        // The plaintext is now the live content of this page; it is clean
        // because the WAL/checkpoint — not Pager::write_dirty — will write it.
        let cg = self.cache_gen(gen);
        self.cache.insert(page_no, cg, padded, false);
        Ok(disk)
    }

    /// Write an already-finished page image straight to `page_no`. The bytes
    /// are written verbatim — used to checkpoint WAL frames (encrypted pages,
    /// or the plaintext header) to their home locations.
    pub fn write_sealed(&mut self, page_no: u64, bytes: &[u8; PAGE_SIZE]) -> Result<()> {
        __failpoint_check()?;
        self.unsynced = true;
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.f()?.write_all(bytes)?;
        __io_trace(IoEvent::Page(page_no));
        Ok(())
    }

    /// Flush OS buffers to stable storage.
    ///
    /// RB-016.1 (v0.20.0): the pages [`Pager::write_dirty`] wrote are clean
    /// only once this succeeds. If it fails they're dirty again: after a
    /// failed `fsync` the OS may have dropped those writes, and a later
    /// `fsync` can still succeed, so the next flush must write them again.
    pub fn sync(&mut self) -> Result<()> {
        if let Err(e) = self.sync_file() {
            self.cache.mark_written_dirty();
            return Err(e);
        }
        crate::bump_fsync();
        self.unsynced = false;
        self.cache.mark_written_clean();
        self.cache.trim();
        Ok(())
    }

    fn sync_file(&mut self) -> Result<()> {
        __sync_failpoint()?;
        self.f()?.sync_all()?;
        Ok(())
    }

    /// RB-016.1 (v0.20.0): a commit failed before its commit point, so the
    /// pages [`Pager::write_dirty`] wrote for it go back on the next flush's
    /// list. Whatever failed, no sync has covered them.
    pub(crate) fn rewrite_unsynced(&mut self) {
        self.cache.mark_written_dirty();
    }

    /// PERF-2 3f (v0.20.0): whether pages were written since the last
    /// [`Pager::sync`], so a commit's barrier knows it has work to do.
    pub fn unsynced(&self) -> bool {
        self.unsynced
    }

    /// Read and decrypt the plaintext payload of an encrypted page. `gen` is
    /// the generation the referencing pointer expects (v9; ignored before).
    pub fn read_page(&mut self, page_no: u64, gen: u64) -> Result<Vec<u8>> {
        let cg = self.cache_gen(gen);
        if let Some(p) = self.cache.get(page_no, cg) {
            return Ok(p);
        }
        let mut disk = [0u8; PAGE_SIZE];
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        read_failpoint(page_no, 1)?;
        self.f()?.read_exact(&mut disk)?;

        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&disk[0..NONCE_LEN]);
        let ciphertext = &disk[NONCE_LEN..];

        let aad = self.page_aad(page_no, gen);
        let payload = crypto::aead_decrypt(&self.dek, &nonce, ciphertext, &aad)
            .map_err(|_| MnemoError::PageAuthFailed(page_no))?;

        // Never over this transaction's own write to the page (an older
        // pointer read it from disk, under its own generation).
        if !self.cache.is_dirty(page_no) {
            self.cache.insert(page_no, cg, payload.clone(), false);
        }
        Ok(payload)
    }

    /// FMT-9b: [`Pager::read_page`] without caching the result: for bulk
    /// reads (loading the vector column) that would otherwise push every
    /// cached record page out. A cached copy (possibly dirty) still wins.
    pub fn read_page_uncached(&mut self, page_no: u64, gen: u64) -> Result<Vec<u8>> {
        if let Some(p) = self.cache.peek(page_no, self.cache_gen(gen)) {
            return Ok(p.to_vec());
        }
        let mut disk = [0u8; PAGE_SIZE];
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        read_failpoint(page_no, 1)?;
        self.f()?.read_exact(&mut disk)?;
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&disk[0..NONCE_LEN]);
        let aad = self.page_aad(page_no, gen);
        crypto::aead_decrypt(&self.dek, &nonce, &disk[NONCE_LEN..], &aad)
            .map_err(|_| MnemoError::PageAuthFailed(page_no))
    }

    /// PERF-2 (v0.19.0): [`Pager::read_page_uncached`] for many pages at
    /// once. `pages` is a list of `(page_no, gen)`; [`PageBatch::payload`]
    /// gives each one's plaintext, in the same order. A cached copy
    /// (possibly dirty) still wins. The rest are read with one positioned
    /// read per run of adjacent pages and decrypted in place in that one
    /// buffer: on up to `available_parallelism()` scoped threads when there
    /// are at least [`PARALLEL_DECRYPT_MIN_PAGES`] of them, on this thread
    /// otherwise. The first page (by page number) that fails authentication
    /// is the error, as it would be one page at a time.
    pub(crate) fn read_pages_uncached(&mut self, pages: &[(u64, u64)]) -> Result<PageBatch> {
        let threads = if pages.len() >= PARALLEL_DECRYPT_MIN_PAGES {
            std::thread::available_parallelism().map_or(1, |n| n.get())
        } else {
            1
        };
        // `MNEMO_DEV_DECRYPT_THREADS` caps the threads, so a benchmark can
        // record the single-thread number (a phone without spare cores).
        // Unsupported and compiled only with `dev-hooks` (HOOKS-1).
        #[cfg(feature = "dev-hooks")]
        let threads = std::env::var("MNEMO_DEV_DECRYPT_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map_or(threads, |cap| threads.min(cap.max(1)));
        self.read_pages_with(pages, threads)
    }

    /// PERF-2 (v0.19.0): read and decrypt `pages` (`(page_no, gen)`), calling
    /// `sink(i, plaintext)` once for each, in no particular order (`i` is
    /// the page's position in `pages`). A cached copy (possibly dirty) still
    /// wins. The rest are split among up to `available_parallelism()`
    /// scoped threads, started once for the call when there are at least
    /// [`PARALLEL_DECRYPT_MIN_PAGES`] pages; each reads its share of the
    /// pages (sorted, adjacent ones in one positioned read) [`STREAM_PAGES`]
    /// at a time into a small buffer, decrypts them in place and hands the
    /// buffer to `sink` on this thread through a bounded channel, so memory
    /// stays a few small buffers whatever the number of pages. A page that
    /// fails authentication, or a failing `sink`, stops the call with that
    /// error.
    pub(crate) fn for_each_page(
        &mut self,
        pages: &[(u64, u64)],
        sink: impl FnMut(usize, &[u8]) -> Result<()>,
    ) -> Result<()> {
        self.for_each_page_with(pages, None, sink)
    }

    /// [`Pager::for_each_page`], with `threads` fixing the thread count
    /// (tests; `None` chooses as `for_each_page` does).
    pub(crate) fn for_each_page_with(
        &mut self,
        pages: &[(u64, u64)],
        threads: Option<usize>,
        mut sink: impl FnMut(usize, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let mut todo: Vec<usize> = Vec::with_capacity(pages.len());
        for (i, &(p, g)) in pages.iter().enumerate() {
            match self.cache.peek(p, self.cache_gen(g)) {
                Some(c) => {
                    let c = c.to_vec();
                    sink(i, &c)?;
                }
                None => todo.push(i),
            }
        }
        if todo.is_empty() {
            return Ok(());
        }
        todo.sort_by_key(|&i| pages[i].0);
        let auto = if todo.len() >= PARALLEL_DECRYPT_MIN_PAGES {
            std::thread::available_parallelism().map_or(1, |n| n.get())
        } else {
            1
        };
        // `MNEMO_DEV_DECRYPT_THREADS`, as for `read_pages_uncached`.
        #[cfg(feature = "dev-hooks")]
        let auto = std::env::var("MNEMO_DEV_DECRYPT_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map_or(auto, |cap| auto.min(cap.max(1)));
        let threads = threads
            .unwrap_or(auto)
            .clamp(1, todo.len().div_ceil(STREAM_PAGES));
        let aads: Vec<Vec<u8>> = todo
            .iter()
            .map(|&i| self.page_aad(pages[i].0, pages[i].1))
            .collect();
        let cipher = crypto::page_cipher(&self.dek);
        let file: &File = self.f()?;
        // Read and decrypt `todo[lo..hi]` (at most STREAM_PAGES) into a new
        // buffer, page after page.
        let read = |lo: usize, hi: usize| -> Result<Vec<u8>> {
            let mut buf = vec![0u8; (hi - lo) * PAGE_SIZE];
            let mut k = lo;
            while k < hi {
                let first = pages[todo[k]].0;
                let mut run = 1;
                while k + run < hi && pages[todo[k + run]].0 == first + run as u64 {
                    run += 1;
                }
                let at = (k - lo) * PAGE_SIZE;
                read_exact_at(
                    file,
                    &mut buf[at..at + run * PAGE_SIZE],
                    first * PAGE_SIZE as u64,
                )?;
                k += run;
            }
            for (n, img) in buf.chunks_exact_mut(PAGE_SIZE).enumerate() {
                let j = lo + n;
                let mut nonce = [0u8; NONCE_LEN];
                nonce.copy_from_slice(&img[..NONCE_LEN]);
                let (body, tag) = img[NONCE_LEN..].split_at_mut(PAYLOAD);
                crypto::aead_decrypt_in_place(&cipher, &nonce, &aads[j], body, tag)
                    .map_err(|_| MnemoError::PageAuthFailed(pages[todo[j]].0))?;
            }
            Ok(buf)
        };
        let mut deliver = |lo: usize, buf: &[u8]| -> Result<()> {
            for (n, img) in buf.chunks_exact(PAGE_SIZE).enumerate() {
                sink(todo[lo + n], &img[NONCE_LEN..NONCE_LEN + PAYLOAD])?;
            }
            Ok(())
        };
        if threads <= 1 {
            let mut lo = 0;
            while lo < todo.len() {
                let hi = (lo + STREAM_PAGES).min(todo.len());
                deliver(lo, &read(lo, hi)?)?;
                lo = hi;
            }
            return Ok(());
        }
        let per = todo.len().div_ceil(threads);
        std::thread::scope(|s| {
            let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(usize, Vec<u8>)>>(2 * threads);
            let read = &read;
            for t in 0..threads {
                let tx = tx.clone();
                let (start, end) = (t * per, ((t + 1) * per).min(todo.len()));
                s.spawn(move || {
                    let mut lo = start;
                    while lo < end {
                        let hi = (lo + STREAM_PAGES).min(end);
                        let msg = read(lo, hi).map(|b| (lo, b));
                        let stop = msg.is_err();
                        if tx.send(msg).is_err() || stop {
                            return;
                        }
                        lo = hi;
                    }
                });
            }
            drop(tx);
            // Dropping `rx` on an error makes every thread's next send fail,
            // so they stop.
            for msg in rx {
                let (lo, buf) = msg?;
                deliver(lo, &buf)?;
            }
            Ok(())
        })
    }

    /// [`Pager::read_pages_uncached`] on at most `threads` threads (1: this
    /// thread only).
    pub(crate) fn read_pages_with(
        &mut self,
        pages: &[(u64, u64)],
        threads: usize,
    ) -> Result<PageBatch> {
        let mut slots: Vec<Slot> = pages
            .iter()
            .map(|&(p, g)| match self.cache.peek(p, self.cache_gen(g)) {
                Some(c) => Slot::Cached(c.to_vec()),
                None => Slot::Disk(usize::MAX),
            })
            .collect();
        // The pages to read, by position in `pages`, sorted by page number so
        // adjacent pages share one read.
        let mut todo: Vec<usize> = (0..pages.len())
            .filter(|&i| matches!(slots[i], Slot::Disk(_)))
            .collect();
        todo.sort_by_key(|&i| pages[i].0);
        for (j, &i) in todo.iter().enumerate() {
            slots[i] = Slot::Disk(j);
        }
        let mut disk = vec![0u8; todo.len() * PAGE_SIZE];
        let mut k = 0;
        while k < todo.len() {
            let first = pages[todo[k]].0;
            let mut run = 1;
            while k + run < todo.len() && pages[todo[k + run]].0 == first + run as u64 {
                run += 1;
            }
            let buf = &mut disk[k * PAGE_SIZE..(k + run) * PAGE_SIZE];
            read_exact_at(self.f()?, buf, first * PAGE_SIZE as u64)?;
            k += run;
        }
        let cipher = crypto::page_cipher(&self.dek);
        let aads: Vec<Vec<u8>> = todo
            .iter()
            .map(|&i| self.page_aad(pages[i].0, pages[i].1))
            .collect();
        // Decrypt pages `first..` of `chunk` in place; the first failure is
        // that page's index in `todo`.
        let open = |first: usize, chunk: &mut [u8]| -> std::result::Result<(), usize> {
            for (n, img) in chunk.chunks_exact_mut(PAGE_SIZE).enumerate() {
                let j = first + n;
                let mut nonce = [0u8; NONCE_LEN];
                nonce.copy_from_slice(&img[..NONCE_LEN]);
                let (body, tag) = img[NONCE_LEN..].split_at_mut(PAYLOAD);
                crypto::aead_decrypt_in_place(&cipher, &nonce, &aads[j], body, tag)
                    .map_err(|_| j)?;
            }
            Ok(())
        };
        let threads = threads.clamp(1, todo.len().max(1));
        let failed: Option<usize> = if threads <= 1 {
            open(0, &mut disk).err()
        } else {
            let per = todo.len().div_ceil(threads);
            std::thread::scope(|s| {
                let open = &open;
                let parts: Vec<_> = disk
                    .chunks_mut(per * PAGE_SIZE)
                    .enumerate()
                    .map(|(t, chunk)| s.spawn(move || open(t * per, chunk).err()))
                    .collect();
                parts
                    .into_iter()
                    .filter_map(|h| h.join().expect("a decrypt thread panicked"))
                    .min()
            })
        };
        if let Some(j) = failed {
            return Err(MnemoError::PageAuthFailed(pages[todo[j]].0));
        }
        Ok(PageBatch { disk, slots })
    }

    /// Stage a plaintext payload for an encrypted page. The payload is padded
    /// (or rejected if too large) to exactly [`PAYLOAD`] bytes. The page is
    /// cached dirty — pinned against eviction — until [`Pager::write_dirty`].
    ///
    /// [`Pager::write_dirty`] seals it under generation `gen` (v9; ignored before).
    pub fn write_page(&mut self, page_no: u64, gen: u64, payload: &[u8]) -> Result<()> {
        if payload.len() > PAYLOAD {
            return Err(MnemoError::Invalid(format!(
                "payload {} exceeds page capacity {}",
                payload.len(),
                PAYLOAD
            )));
        }
        let mut padded = vec![0u8; PAYLOAD];
        padded[..payload.len()].copy_from_slice(payload);
        let cg = self.cache_gen(gen);
        self.cache.insert(page_no, cg, padded, true);
        Ok(())
    }

    /// Encrypt and write every dirty page, without the fsync (PERF-2 3f):
    /// the caller syncs ([`Pager::sync`]) before anything that references
    /// the pages can be durable. [`Pager::unsynced`] says it's due.
    ///
    /// Each page gets a fresh nonce from the per-session random prefix +
    /// per-session counter (`Pager::next_nonce` → `crypto::session_page_nonce`
    /// since v0.5.2 / SEC-1). The counter advances per write; the random
    /// prefix is drawn once per writable open and regenerated on counter
    /// wrap, so no nonce ever repeats across writes in this session or
    /// across copies of the file. Once written, pages are *written*: still
    /// pinned, and not written again, until [`Pager::sync`] makes them clean
    /// (or, failing, dirty again: RB-016.1).
    pub fn write_dirty(&mut self) -> Result<()> {
        let pages = self.cache.dirty_pages();
        if pages.is_empty() {
            return Ok(());
        }
        for (page_no, gen) in pages {
            let payload = self
                .cache
                .peek(page_no, gen)
                .expect("dirty page must be cached")
                .to_vec();
            self.write_counter += 1;
            let nonce = self.next_nonce();
            let aad = self.page_aad(page_no, gen);
            let ciphertext = crypto::aead_encrypt(&self.dek, &nonce, &payload, &aad)?;

            let mut disk = Vec::with_capacity(PAGE_SIZE);
            disk.extend_from_slice(&nonce);
            disk.extend_from_slice(&ciphertext);
            debug_assert_eq!(disk.len(), PAGE_SIZE);

            __failpoint_check()?;
            self.unsynced = true;
            self.f()?
                .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
            self.f()?.write_all(&disk)?;
            __io_trace(IoEvent::Page(page_no));
        }
        // The pages just written stay pinned until a sync covers them
        // (RB-016.1): if that sync fails, the next flush writes them again,
        // as v0.19.1 did. `unsynced` keeps the sync due; the sync settles the
        // cache back within its bound.
        self.cache.mark_dirty_written();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEN: u64 = 7;

    fn pager(path: &std::path::Path) -> Pager {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap();
        Pager::new(
            LockedFile::new(file),
            Zeroizing::new([7u8; KEY_LEN]),
            0,
            crate::format::VERSION_V9,
        )
    }

    /// Pages 1..=n, each holding its own number, sealed under `GEN`.
    fn write_pages(path: &std::path::Path, n: u64) {
        let mut p = pager(path);
        for page in 1..=n {
            let body = page.to_le_bytes();
            let img = p.seal_page(page, GEN, &body).unwrap();
            p.write_sealed(page, &img).unwrap();
        }
        p.sync().unwrap();
    }

    /// PERF-2: a batch read gives what page-at-a-time reads give, in the
    /// order asked, with gaps and on one thread or several; the first page
    /// that fails authentication is the error on both paths; a read past
    /// the end fails; a cached (dirty) copy wins.
    #[test]
    fn batched_reads_match_single_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.bin");
        let n = (PARALLEL_DECRYPT_MIN_PAGES as u64) * 2;
        write_pages(&path, n);
        let mut want: Vec<(u64, u64)> = (1..=n).filter(|p| p % 5 != 0).map(|p| (p, GEN)).collect();
        want.reverse();
        want.swap(3, 40);
        let mut single = pager(&path);
        let one: Vec<Vec<u8>> = want
            .iter()
            .map(|&(p, g)| single.read_page_uncached(p, g).unwrap())
            .collect();
        let all = |b: PageBatch| {
            (0..b.len())
                .map(|i| b.payload(i).to_vec())
                .collect::<Vec<_>>()
        };
        for threads in [1, 4] {
            let mut p = pager(&path);
            assert_eq!(
                all(p.read_pages_with(&want, threads).unwrap()),
                one,
                "{threads} threads"
            );
        }
        assert_eq!(all(pager(&path).read_pages_uncached(&want).unwrap()), one);
        for (got, &(page, _)) in one.iter().zip(&want) {
            assert_eq!(&got[..8], &page.to_le_bytes());
        }

        // A damaged page (one the batch asks for): its own number in the
        // error, both paths.
        let bad = 51u64;
        assert!(want.iter().any(|&(p, _)| p == bad));
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.seek(SeekFrom::Start(bad * PAGE_SIZE as u64 + 100))
                .unwrap();
            f.write_all(&[0xA5]).unwrap();
        }
        for threads in [1, 4] {
            let res = pager(&path).read_pages_with(&want, threads);
            assert!(
                matches!(res, Err(MnemoError::PageAuthFailed(p)) if p == bad),
                "{threads} threads: {:?}",
                res.map(|b| b.len())
            );
        }
        // The wrong generation fails too.
        let res = pager(&path).read_pages_with(&[(1, GEN + 1)], 1);
        assert!(
            matches!(res, Err(MnemoError::PageAuthFailed(1))),
            "{:?}",
            res.map(|b| b.len())
        );
        // Past the end of the file.
        assert!(pager(&path).read_pages_with(&[(n + 5, GEN)], 1).is_err());
        // A dirty cached copy wins over the disk.
        let mut p = pager(&path);
        p.write_page(2, GEN, b"in the cache").unwrap();
        let got = p.read_pages_with(&[(2, GEN), (3, GEN)], 1).unwrap();
        assert_eq!(&got.payload(0)[..12], b"in the cache");
        assert_eq!(&got.payload(1)[..8], &3u64.to_le_bytes());
    }

    /// PERF-2: `for_each_page` hands every page to the sink once, with its
    /// own plaintext, on one thread or several; a damaged page or a failing
    /// sink stops it with that error; a cached (dirty) copy wins.
    #[test]
    fn streamed_reads_reach_the_sink_once_each() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.bin");
        let n = (PARALLEL_DECRYPT_MIN_PAGES as u64) * 3;
        write_pages(&path, n);
        let want: Vec<(u64, u64)> = (1..=n)
            .rev()
            .filter(|p| p % 7 != 0)
            .map(|p| (p, GEN))
            .collect();
        for threads in [Some(1), Some(4), None] {
            let mut seen = vec![0u32; want.len()];
            pager(&path)
                .for_each_page_with(&want, threads, |i, bytes| {
                    seen[i] += 1;
                    assert_eq!(&bytes[..8], &want[i].0.to_le_bytes());
                    assert_eq!(bytes.len(), PAYLOAD);
                    Ok(())
                })
                .unwrap();
            assert!(seen.iter().all(|&c| c == 1), "{threads:?}: {seen:?}");
        }
        // A failing sink stops the call with its error.
        for threads in [Some(1), Some(4)] {
            let mut calls = 0;
            let res = pager(&path).for_each_page_with(&want, threads, |_, _| {
                calls += 1;
                if calls == 10 {
                    Err(MnemoError::Invalid("stop".into()))
                } else {
                    Ok(())
                }
            });
            assert!(matches!(res, Err(MnemoError::Invalid(_))), "{threads:?}");
            assert_eq!(calls, 10);
        }
        // A cached dirty copy wins.
        let mut p = pager(&path);
        p.write_page(2, GEN, b"cached").unwrap();
        let mut got = Vec::new();
        p.for_each_page_with(&[(2, GEN), (3, GEN)], Some(1), |i, b| {
            got.push((i, b[..6].to_vec()));
            Ok(())
        })
        .unwrap();
        got.sort();
        assert_eq!(got[0], (0, b"cached".to_vec()));
        assert_eq!(&got[1].1[..6], &3u64.to_le_bytes()[..6]);
        // A damaged page: its own number in the error.
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.seek(SeekFrom::Start(100 * PAGE_SIZE as u64 + 64))
                .unwrap();
            f.write_all(&[0x5A]).unwrap();
        }
        for threads in [Some(1), Some(4)] {
            let res = pager(&path).for_each_page_with(&want, threads, |_, _| Ok(()));
            assert!(
                matches!(res, Err(MnemoError::PageAuthFailed(100))),
                "{threads:?}: {:?}",
                res.err()
            );
        }
    }
}
