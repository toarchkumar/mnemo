//! The `.mnemo` on-disk file format (Phase 1 of the build plan).
//!
//! A `.mnemo` file is a sequence of fixed-size pages.
//!
//! * **Page 0** is the [`Header`] — the *only* unencrypted region. It carries
//!   the magic bytes, format version, and the key-derivation parameters needed
//!   to unlock everything else. It contains no user data.
//! * **Pages 1..** are encrypted with AES-256-GCM. Each on-disk page is laid
//!   out as `nonce (12) || ciphertext || tag (16)`, so the usable plaintext
//!   payload is [`PAYLOAD`] bytes.
//!
//! Records (memories) and the catalog are stored as runs of consecutive
//! encrypted pages. The header points at the current catalog run; the catalog
//! maps memory IDs to their page runs.

use crate::crypto::{NONCE_LEN, SALT_LEN, TAG_LEN};
use crate::error::{MnemoError, Result};

/// Magic bytes at the start of every `.mnemo` file.
pub const MAGIC: &[u8; 4] = b"MNEM";
/// Current format version.
///
/// History:
/// - v4 added the snapshot-manifest region.
/// - **v5** widens [`CatalogEntry`] with `accessed_at` and `access_count`,
///   so `recall` can update access stats without rewriting the full record
///   (Phase 2.1 of the improvement plan). Migration is automatic on
///   first [`crate::Mnemo::open`] of a v4 file: the catalog is replayed
///   into the v5 shape (populating the new fields from each record's
///   serialized body) and rewritten on the next flush.
///
/// [`CatalogEntry`]: crate::store
pub const VERSION: u16 = 7;
/// Lowest version this build can auto-migrate on open. Files older than
/// this are rejected with [`MnemoError::UnsupportedVersion`]; files in
/// `[MIGRATABLE_FROM, VERSION)` are upgraded in place by [`crate::Mnemo::open`]
/// and rewritten under the current `VERSION` on the next flush.
///
/// History:
/// - **v7** adds a small AES-GCM seal at the tail of the header page that
///   authenticates every mutable header field (write_counter, next_page,
///   catalog/index/manifest pointers, etc.) under the DEK. An attacker
///   can no longer silently rewrite, e.g., `catalog_start` to point at a
///   stale catalog run — the seal's GCM tag fails. Migration is just a
///   single flush; pages are untouched (page crypto is unchanged from v6).
/// - **v6** binds each encrypted page's home `page_no` as AES-GCM AAD so
///   page-transplant attacks (move a valid encrypted page to a different
///   slot) become tamper-evident. Migration re-encrypts every live page.
/// - **v5** widened `CatalogEntry` with `accessed_at` and `access_count`
///   so `recall` no longer rewrites the full record. Migration replays
///   the v4 catalog into the v5 shape.
/// - v4 added the snapshot-manifest region.
pub const MIGRATABLE_FROM: u16 = 4;
/// Size of a page in bytes (on disk).
pub const PAGE_SIZE: usize = 8192;
/// Usable plaintext bytes per encrypted page (`PAGE_SIZE` minus crypto overhead).
pub const PAYLOAD: usize = PAGE_SIZE - NONCE_LEN - TAG_LEN; // 8164
/// Length of the wrapped DEK blob: 32-byte key + 16-byte tag.
pub const WRAPPED_DEK_LEN: usize = 48;

/// Bit flag: the file's pages are encrypted (always set in v1).
pub const FLAG_ENCRYPTED: u32 = 1;

/// Byte offset of the header CRC-32 (everything before it is covered).
pub const HEADER_CRC_OFF: usize = 238;

// --- v7 header AEAD seal layout -----------------------------------------
//
// The v7 seal is a small AES-GCM authentication tag appended to the header
// page that proves the mutable header fields have not been silently
// rewritten by an attacker with file-write access. The seal uses:
//
//   nonce      = fresh random 12 bytes per header write (stored on disk)
//   plaintext  = empty
//   aad        = SEAL_AAD_PREFIX || version_le || all mutable u64 fields
//   key        = DEK
//   output     = 16-byte GCM tag (the seal)
//
// So a tampered field in the plaintext header produces a different AAD on
// the next open, and the tag fails authentication. The CRC at byte 238
// stays as a pre-passphrase torn-write check; the seal is the keyed
// integrity layer that runs after the DEK is unwrapped.

/// Byte offset of the v7 header seal nonce.
pub const HEADER_SEAL_NONCE_OFF: usize = 242;
/// Byte offset of the v7 header seal tag.
pub const HEADER_SEAL_TAG_OFF: usize = HEADER_SEAL_NONCE_OFF + NONCE_LEN;
/// Domain-separation prefix for the v7 header seal AAD. Distinguishes the
/// header seal from page AAD and any future seals.
pub const SEAL_AAD_PREFIX: &[u8] = b"mnemo-header-seal-v7";

// Compile-time check: the seal region must fit inside one page after the
// existing header fields and CRC.
const _: () = assert!(HEADER_SEAL_TAG_OFF + TAG_LEN <= PAGE_SIZE);

/// The header page. Fixed-size, unencrypted, lives at page 0.
#[derive(Clone, Debug)]
pub struct Header {
    pub version: u16,
    pub page_size: u32,
    pub flags: u32,
    /// Embedding dimensionality every vector in this file must match.
    pub dimensions: u32,
    /// Unix seconds when the file was created.
    pub created_at: i64,
    /// Monotonic counter feeding page nonces. Never decreases.
    pub write_counter: u64,
    /// Next unallocated page number (append-only allocator).
    pub next_page: u64,
    /// First page of the current catalog run (0 if the catalog is empty).
    pub catalog_start: u64,
    /// Number of pages in the catalog run.
    pub catalog_pages: u64,
    /// Exact byte length of the serialized catalog.
    pub catalog_len: u64,
    /// Number of live (non-deleted) memories.
    pub vector_count: u64,
    /// Argon2id memory cost (KiB).
    pub m_cost: u32,
    /// Argon2id time cost.
    pub t_cost: u32,
    /// Argon2id parallelism.
    pub p_cost: u32,
    /// Argon2 salt for KEK derivation.
    pub salt: [u8; SALT_LEN],
    /// Nonce used to wrap the DEK.
    pub dek_nonce: [u8; NONCE_LEN],
    /// The DEK encrypted under the KEK (`ciphertext || tag`).
    pub wrapped_dek: [u8; WRAPPED_DEK_LEN],
    /// First page of the ANN index run (0 if no index has been built).
    pub index_start: u64,
    /// Number of pages in the ANN index run.
    pub index_pages: u64,
    /// Exact byte length of the serialized ANN index.
    pub index_len: u64,
    /// First page of the write-ahead log region.
    pub wal_start: u64,
    /// Number of pages in the WAL region.
    pub wal_pages: u64,
    /// Id of the most recent transaction already checkpointed into this
    /// header. A WAL transaction with a higher id is replayed on open.
    pub wal_seq: u64,
    /// First page of the snapshot-manifest run (0 = none yet).
    pub manifest_start: u64,
    /// Page count of the snapshot-manifest run.
    pub manifest_pages: u64,
    /// Exact serialized byte length of the snapshot manifest.
    pub manifest_len: u64,
    /// v7 header AEAD seal nonce, parsed from disk for v7+ files. `None`
    /// for older formats and for freshly created in-memory headers — the
    /// seal nonce is regenerated per write by [`Header::apply_seal`].
    pub seal_nonce: Option<[u8; NONCE_LEN]>,
    /// v7 header AEAD seal tag, parsed from disk for v7+ files. `None`
    /// for older formats and freshly created headers. Validated by
    /// [`Header::validate_seal`] after the DEK is unwrapped.
    pub seal_tag: Option<[u8; TAG_LEN]>,
}

/// Compute the AEAD AAD bytes used by the v7 header seal. The AAD covers
/// every header field whose value carries security-relevant meaning at
/// open time (catalog/index/manifest pointers, write counter, version
/// itself), so flipping any of them invalidates the seal tag.
fn header_seal_aad(h: &Header) -> Vec<u8> {
    let mut aad = Vec::with_capacity(SEAL_AAD_PREFIX.len() + 2 + 15 * 8);
    aad.extend_from_slice(SEAL_AAD_PREFIX);
    aad.extend_from_slice(&h.version.to_le_bytes());
    aad.extend_from_slice(&h.write_counter.to_le_bytes());
    aad.extend_from_slice(&h.next_page.to_le_bytes());
    aad.extend_from_slice(&h.catalog_start.to_le_bytes());
    aad.extend_from_slice(&h.catalog_pages.to_le_bytes());
    aad.extend_from_slice(&h.catalog_len.to_le_bytes());
    aad.extend_from_slice(&h.vector_count.to_le_bytes());
    aad.extend_from_slice(&h.index_start.to_le_bytes());
    aad.extend_from_slice(&h.index_pages.to_le_bytes());
    aad.extend_from_slice(&h.index_len.to_le_bytes());
    aad.extend_from_slice(&h.wal_start.to_le_bytes());
    aad.extend_from_slice(&h.wal_pages.to_le_bytes());
    aad.extend_from_slice(&h.wal_seq.to_le_bytes());
    aad.extend_from_slice(&h.manifest_start.to_le_bytes());
    aad.extend_from_slice(&h.manifest_pages.to_le_bytes());
    aad.extend_from_slice(&h.manifest_len.to_le_bytes());
    aad
}

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn rd_i64(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl Header {
    /// Serialize the header into a full page-sized buffer.
    pub fn to_page(&self) -> [u8; PAGE_SIZE] {
        let mut b = [0u8; PAGE_SIZE];
        b[0..4].copy_from_slice(MAGIC);
        b[4..6].copy_from_slice(&self.version.to_le_bytes());
        b[6..10].copy_from_slice(&self.page_size.to_le_bytes());
        b[10..14].copy_from_slice(&self.flags.to_le_bytes());
        b[14..18].copy_from_slice(&self.dimensions.to_le_bytes());
        b[18..26].copy_from_slice(&self.created_at.to_le_bytes());
        b[26..34].copy_from_slice(&self.write_counter.to_le_bytes());
        b[34..42].copy_from_slice(&self.next_page.to_le_bytes());
        b[42..50].copy_from_slice(&self.catalog_start.to_le_bytes());
        b[50..58].copy_from_slice(&self.catalog_pages.to_le_bytes());
        b[58..66].copy_from_slice(&self.catalog_len.to_le_bytes());
        b[66..74].copy_from_slice(&self.vector_count.to_le_bytes());
        b[74..78].copy_from_slice(&self.m_cost.to_le_bytes());
        b[78..82].copy_from_slice(&self.t_cost.to_le_bytes());
        b[82..86].copy_from_slice(&self.p_cost.to_le_bytes());
        b[86..86 + SALT_LEN].copy_from_slice(&self.salt);
        b[102..102 + NONCE_LEN].copy_from_slice(&self.dek_nonce);
        b[114..114 + WRAPPED_DEK_LEN].copy_from_slice(&self.wrapped_dek);
        b[162..170].copy_from_slice(&self.index_start.to_le_bytes());
        b[170..178].copy_from_slice(&self.index_pages.to_le_bytes());
        b[178..186].copy_from_slice(&self.index_len.to_le_bytes());
        b[186..194].copy_from_slice(&self.wal_start.to_le_bytes());
        b[194..202].copy_from_slice(&self.wal_pages.to_le_bytes());
        b[202..210].copy_from_slice(&self.wal_seq.to_le_bytes());
        b[214..222].copy_from_slice(&self.manifest_start.to_le_bytes());
        b[222..230].copy_from_slice(&self.manifest_pages.to_le_bytes());
        b[230..238].copy_from_slice(&self.manifest_len.to_le_bytes());
        // Header CRC over every byte before it — detects a torn page-0 write.
        let crc = crate::wal::checksum(&b[0..HEADER_CRC_OFF]);
        b[HEADER_CRC_OFF..HEADER_CRC_OFF + 4].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Parse a header from a page-sized buffer, validating magic and version.
    pub fn from_page(b: &[u8]) -> Result<Header> {
        if b.len() < PAGE_SIZE {
            return Err(MnemoError::BadMagic);
        }
        if &b[0..4] != MAGIC {
            return Err(MnemoError::BadMagic);
        }
        let version = rd_u16(b, 4);
        // Accept the current VERSION and any migratable predecessor; older
        // and forward versions are rejected. `Mnemo::open` is responsible
        // for upgrading migratable in-memory state and rewriting the on-disk
        // format on the next flush.
        if version != VERSION && !(MIGRATABLE_FROM..VERSION).contains(&version) {
            return Err(MnemoError::UnsupportedVersion(version));
        }
        let stored_crc = rd_u32(b, HEADER_CRC_OFF);
        if crate::wal::checksum(&b[0..HEADER_CRC_OFF]) != stored_crc {
            // A torn or corrupt header page — fail cleanly rather than
            // derive a key from garbage.
            return Err(MnemoError::HeaderChecksum);
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&b[86..86 + SALT_LEN]);
        let mut dek_nonce = [0u8; NONCE_LEN];
        dek_nonce.copy_from_slice(&b[102..102 + NONCE_LEN]);
        let mut wrapped_dek = [0u8; WRAPPED_DEK_LEN];
        wrapped_dek.copy_from_slice(&b[114..114 + WRAPPED_DEK_LEN]);
        Ok(Header {
            version,
            page_size: rd_u32(b, 6),
            flags: rd_u32(b, 10),
            dimensions: rd_u32(b, 14),
            created_at: rd_i64(b, 18),
            write_counter: rd_u64(b, 26),
            next_page: rd_u64(b, 34),
            catalog_start: rd_u64(b, 42),
            catalog_pages: rd_u64(b, 50),
            catalog_len: rd_u64(b, 58),
            vector_count: rd_u64(b, 66),
            m_cost: rd_u32(b, 74),
            t_cost: rd_u32(b, 78),
            p_cost: rd_u32(b, 82),
            salt,
            dek_nonce,
            wrapped_dek,
            index_start: rd_u64(b, 162),
            index_pages: rd_u64(b, 170),
            index_len: rd_u64(b, 178),
            wal_start: rd_u64(b, 186),
            wal_pages: rd_u64(b, 194),
            wal_seq: rd_u64(b, 202),
            manifest_start: rd_u64(b, 214),
            manifest_pages: rd_u64(b, 222),
            manifest_len: rd_u64(b, 230),
            // v7+ headers store an AEAD seal nonce and tag after the CRC.
            // For older formats the bytes are zero / unused; the seal is
            // only validated when `version >= 7`.
            seal_nonce: if version >= 7 {
                let mut n = [0u8; NONCE_LEN];
                n.copy_from_slice(&b[HEADER_SEAL_NONCE_OFF..HEADER_SEAL_NONCE_OFF + NONCE_LEN]);
                Some(n)
            } else {
                None
            },
            seal_tag: if version >= 7 {
                let mut t = [0u8; TAG_LEN];
                t.copy_from_slice(&b[HEADER_SEAL_TAG_OFF..HEADER_SEAL_TAG_OFF + TAG_LEN]);
                Some(t)
            } else {
                None
            },
        })
    }

    /// Validate the v7 header seal under the given DEK. Recomputes the AAD
    /// from this header's current field values and verifies the GCM tag
    /// against the stored seal nonce. No-op for v6 and below, where no
    /// seal exists. Returns [`MnemoError::HeaderTampered`] on failure.
    pub fn validate_seal(&self, dek: &[u8; crate::crypto::KEY_LEN]) -> Result<()> {
        if self.version < 7 {
            return Ok(());
        }
        let nonce = self
            .seal_nonce
            .ok_or(MnemoError::HeaderTampered)?;
        let tag = self
            .seal_tag
            .ok_or(MnemoError::HeaderTampered)?;
        let aad = header_seal_aad(self);
        // AES-GCM over empty plaintext yields a 16-byte tag; decryption
        // takes the tag as the ciphertext input and returns empty bytes
        // on success.
        crate::crypto::aead_decrypt(dek, &nonce, &tag, &aad)
            .map(|_| ())
            .map_err(|_| MnemoError::HeaderTampered)
    }

    /// Write the v7 header seal into `page` at the fixed seal offsets.
    /// Generates a fresh random nonce on every call and computes the tag
    /// over a recomputed AAD, so every header write produces a new seal.
    /// No-op for v6 and below.
    pub fn apply_seal(
        &mut self,
        page: &mut [u8; PAGE_SIZE],
        dek: &[u8; crate::crypto::KEY_LEN],
    ) -> Result<()> {
        if self.version < 7 {
            return Ok(());
        }
        let nonce = crate::crypto::random_nonce();
        let aad = header_seal_aad(self);
        let tag_buf = crate::crypto::aead_encrypt(dek, &nonce, &[], &aad)?;
        if tag_buf.len() != TAG_LEN {
            return Err(MnemoError::Crypto(
                "header seal: unexpected tag length".into(),
            ));
        }
        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&tag_buf);
        page[HEADER_SEAL_NONCE_OFF..HEADER_SEAL_NONCE_OFF + NONCE_LEN]
            .copy_from_slice(&nonce);
        page[HEADER_SEAL_TAG_OFF..HEADER_SEAL_TAG_OFF + TAG_LEN].copy_from_slice(&tag);
        self.seal_nonce = Some(nonce);
        self.seal_tag = Some(tag);
        Ok(())
    }
}

// Compile-time sanity: the header layout must fit inside one page.
const _: () = assert!(HEADER_CRC_OFF + 4 <= PAGE_SIZE);
const _: () = assert!(TAG_LEN == 16);
