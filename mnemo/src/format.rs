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
/// Current format version. v4 adds the snapshot-manifest region.
pub const VERSION: u16 = 4;
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
        if version != VERSION {
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
        })
    }
}

// Compile-time sanity: the header layout must fit inside one page.
const _: () = assert!(HEADER_CRC_OFF + 4 <= PAGE_SIZE);
const _: () = assert!(TAG_LEN == 16);
