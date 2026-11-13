//! The pager: encrypted, page-granular I/O over the database file.
//!
//! The pager owns the file handle and the data-encryption-key (DEK). Callers
//! deal in *plaintext payloads*; the pager transparently encrypts on write and
//! decrypts on read. Decrypted pages are kept in a **bounded LRU cache**
//! ([`crate::cache::PageCache`]) so repeated reads skip a decrypt and writes
//! are batched until [`Pager::flush`]. The cache caps retained *clean* pages;
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

use zeroize::Zeroizing;

use crate::cache::PageCache;
use crate::crypto::{self, KEY_LEN, NONCE_LEN};
use crate::error::{MnemoError, Result};
use crate::format::{PAGE_SIZE, PAYLOAD};

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
    /// Bounded LRU cache of decrypted page payloads.
    cache: PageCache,
    /// FMT-9: the generation each dirty page is to be sealed under (v9
    /// binds it into the page AAD). Drained by [`Pager::flush`].
    dirty_gens: std::collections::HashMap<u64, u64>,
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
            dirty_gens: std::collections::HashMap::new(),
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
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.f()?.write_all(data)?;
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
        // because the WAL/checkpoint — not Pager::flush — will write it.
        self.cache.insert(page_no, padded, false);
        Ok(disk)
    }

    /// Write an already-finished page image straight to `page_no`. The bytes
    /// are written verbatim — used to checkpoint WAL frames (encrypted pages,
    /// or the plaintext header) to their home locations.
    pub fn write_sealed(&mut self, page_no: u64, bytes: &[u8; PAGE_SIZE]) -> Result<()> {
        __failpoint_check()?;
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.f()?.write_all(bytes)?;
        Ok(())
    }

    /// Flush OS buffers to stable storage.
    pub fn sync(&mut self) -> Result<()> {
        self.f()?.sync_all()?;
        crate::bump_fsync();
        Ok(())
    }

    /// Read and decrypt the plaintext payload of an encrypted page. `gen` is
    /// the generation the referencing pointer expects (v9; ignored before).
    pub fn read_page(&mut self, page_no: u64, gen: u64) -> Result<Vec<u8>> {
        if let Some(p) = self.cache.get(page_no) {
            return Ok(p);
        }
        let mut disk = [0u8; PAGE_SIZE];
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.f()?.read_exact(&mut disk)?;

        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&disk[0..NONCE_LEN]);
        let ciphertext = &disk[NONCE_LEN..];

        let aad = self.page_aad(page_no, gen);
        let payload = crypto::aead_decrypt(&self.dek, &nonce, ciphertext, &aad)
            .map_err(|_| MnemoError::PageAuthFailed(page_no))?;

        self.cache.insert(page_no, payload.clone(), false);
        Ok(payload)
    }

    /// FMT-9b: [`Pager::read_page`] without caching the result: for bulk
    /// reads (loading the vector column) that would otherwise push every
    /// cached record page out. A cached copy (possibly dirty) still wins.
    pub fn read_page_uncached(&mut self, page_no: u64, gen: u64) -> Result<Vec<u8>> {
        if let Some(p) = self.cache.peek(page_no) {
            return Ok(p.to_vec());
        }
        let mut disk = [0u8; PAGE_SIZE];
        self.f()?
            .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.f()?.read_exact(&mut disk)?;
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&disk[0..NONCE_LEN]);
        let aad = self.page_aad(page_no, gen);
        crypto::aead_decrypt(&self.dek, &nonce, &disk[NONCE_LEN..], &aad)
            .map_err(|_| MnemoError::PageAuthFailed(page_no))
    }

    /// Stage a plaintext payload for an encrypted page. The payload is padded
    /// (or rejected if too large) to exactly [`PAYLOAD`] bytes. The page is
    /// cached dirty — pinned against eviction — until [`Pager::flush`].
    ///
    /// [`Pager::flush`] seals it under generation `gen` (v9; ignored before).
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
        self.cache.insert(page_no, padded, true);
        self.dirty_gens.insert(page_no, gen);
        Ok(())
    }

    /// Encrypt and write every dirty page, then fsync.
    ///
    /// Each page gets a fresh nonce from the per-session random prefix +
    /// per-session counter (`Pager::next_nonce` → `crypto::session_page_nonce`
    /// since v0.5.2 / SEC-1). The counter advances per write; the random
    /// prefix is drawn once per writable open and regenerated on counter
    /// wrap, so no nonce ever repeats across writes in this session or
    /// across copies of the file. Once written, pages become clean and
    /// the cache is trimmed back to its cap.
    pub fn flush(&mut self) -> Result<()> {
        let pages = self.cache.dirty_pages();
        if pages.is_empty() {
            return Ok(());
        }
        for page_no in pages {
            let payload = self
                .cache
                .peek(page_no)
                .expect("dirty page must be cached")
                .to_vec();
            self.write_counter += 1;
            let nonce = self.next_nonce();
            let gen = self.dirty_gens.get(&page_no).copied().unwrap_or(0);
            let aad = self.page_aad(page_no, gen);
            let ciphertext = crypto::aead_encrypt(&self.dek, &nonce, &payload, &aad)?;

            let mut disk = Vec::with_capacity(PAGE_SIZE);
            disk.extend_from_slice(&nonce);
            disk.extend_from_slice(&ciphertext);
            debug_assert_eq!(disk.len(), PAGE_SIZE);

            __failpoint_check()?;
            self.f()?
                .seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
            self.f()?.write_all(&disk)?;
        }
        self.f()?.sync_all()?;
        crate::bump_fsync();
        // The pages just written are now clean and so evictable; settle the
        // cache back within its bound.
        self.cache.mark_all_clean();
        self.dirty_gens.clear();
        self.cache.trim();
        Ok(())
    }
}
