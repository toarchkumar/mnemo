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

use zeroize::Zeroizing;

use crate::cache::PageCache;
use crate::crypto::{self, KEY_LEN, NONCE_LEN};
use crate::error::{MnemoError, Result};
use crate::format::{PAGE_SIZE, PAYLOAD};

/// Default page-cache capacity, in pages (~64 MiB of decrypted payloads).
pub const DEFAULT_CACHE_PAGES: usize = 8192;

/// Encrypted page reader/writer with a bounded write-back cache.
pub struct Pager {
    file: File,
    dek: Zeroizing<[u8; KEY_LEN]>,
    /// Monotonic nonce counter. Incremented for every encrypted page write.
    pub write_counter: u64,
    /// Bounded LRU cache of decrypted page payloads.
    cache: PageCache,
}

impl Pager {
    /// Wrap an open file with a pager. `write_counter` comes from the header.
    pub fn new(file: File, dek: Zeroizing<[u8; KEY_LEN]>, write_counter: u64) -> Self {
        Self {
            file,
            dek,
            write_counter,
            cache: PageCache::new(DEFAULT_CACHE_PAGES),
        }
    }

    /// Borrow the data-encryption-key (used by `rekey`, which re-wraps it).
    pub fn dek(&self) -> &[u8; KEY_LEN] {
        &self.dek
    }

    /// Mutable access to the underlying file. Used by the WAL, which writes
    /// its log frames directly rather than through the page cache.
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
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
        self.file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.file.write_all(data)?;
        Ok(())
    }

    /// Read a raw, *unencrypted* page-sized buffer from `page_no`.
    ///
    /// Retained as the symmetric counterpart to [`Pager::write_raw`]; the
    /// header page is currently read directly in `Mnemo::open`.
    #[allow(dead_code)]
    pub fn read_raw(&mut self, page_no: u64) -> Result<[u8; PAGE_SIZE]> {
        let mut buf = [0u8; PAGE_SIZE];
        self.file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Encrypt `payload` into a finished on-disk page image **without writing
    /// it**. Advances the nonce counter and refreshes the page cache (as a
    /// clean entry) so later reads see the new content. Used by the WAL path:
    /// the returned bytes are logged to the WAL and only later copied to
    /// `page_no` by a checkpoint.
    pub fn seal_page(&mut self, page_no: u64, payload: &[u8]) -> Result<[u8; PAGE_SIZE]> {
        if payload.len() > PAYLOAD {
            return Err(MnemoError::Invalid(format!(
                "payload {} exceeds page capacity {}",
                payload.len(),
                PAYLOAD
            )));
        }
        let mut padded = vec![0u8; PAYLOAD];
        padded[..payload.len()].copy_from_slice(payload);

        self.write_counter += 1;
        let nonce = crypto::page_nonce(page_no, self.write_counter);
        let ciphertext = crypto::aead_encrypt(&self.dek, &nonce, &padded)?;

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
        self.file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.file.write_all(bytes)?;
        Ok(())
    }

    /// Flush OS buffers to stable storage.
    pub fn sync(&mut self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }

    /// Read and decrypt the plaintext payload of an encrypted page.
    pub fn read_page(&mut self, page_no: u64) -> Result<Vec<u8>> {
        if let Some(p) = self.cache.get(page_no) {
            return Ok(p);
        }
        let mut disk = [0u8; PAGE_SIZE];
        self.file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
        self.file.read_exact(&mut disk)?;

        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&disk[0..NONCE_LEN]);
        let ciphertext = &disk[NONCE_LEN..];

        let payload = crypto::aead_decrypt(&self.dek, &nonce, ciphertext)
            .map_err(|_| MnemoError::PageAuthFailed(page_no))?;

        self.cache.insert(page_no, payload.clone(), false);
        Ok(payload)
    }

    /// Stage a plaintext payload for an encrypted page. The payload is padded
    /// (or rejected if too large) to exactly [`PAYLOAD`] bytes. The page is
    /// cached dirty — pinned against eviction — until [`Pager::flush`].
    pub fn write_page(&mut self, page_no: u64, payload: &[u8]) -> Result<()> {
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
        Ok(())
    }

    /// Number of pages currently dirty in the cache.
    ///
    /// Used by [`crate::Mnemo::flush`] to pre-compute how many `write_counter`
    /// values the upcoming [`Pager::flush`] will consume, so the store can
    /// stamp a "leased" header to disk *before* any encrypted page hits the
    /// disk. See the leasing discussion on `Mnemo::flush`.
    pub fn dirty_page_count(&self) -> usize {
        self.cache.dirty_pages().len()
    }

    /// Encrypt and write every dirty page, then fsync.
    ///
    /// Each page gets a fresh nonce from `(page_no, write_counter)`; the
    /// counter advances per write so a nonce is never reused. Once written,
    /// pages become clean and the cache is trimmed back to its cap.
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
            let nonce = crypto::page_nonce(page_no, self.write_counter);
            let ciphertext = crypto::aead_encrypt(&self.dek, &nonce, &payload)?;

            let mut disk = Vec::with_capacity(PAGE_SIZE);
            disk.extend_from_slice(&nonce);
            disk.extend_from_slice(&ciphertext);
            debug_assert_eq!(disk.len(), PAGE_SIZE);

            self.file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
            self.file.write_all(&disk)?;
        }
        self.file.sync_all()?;
        // The pages just written are now clean and so evictable; settle the
        // cache back within its bound.
        self.cache.mark_all_clean();
        self.cache.trim();
        Ok(())
    }
}
