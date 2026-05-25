//! Write-ahead log (Phase 3 of the build plan).
//!
//! # What the WAL does here
//!
//! Mnemo's home pages are append-only: a memory record, a catalog snapshot,
//! and the ANN index are each written to *fresh* pages every time, so the only
//! page ever overwritten in place is page 0, the header. The WAL turns a
//! `flush` into a single **atomic transaction**:
//!
//! 1. Record (vector) data pages are written copy-on-write straight to fresh
//!    home pages and fsynced — unreferenced, hence crash-safe by construction.
//! 2. The transaction's *control plane* — the new catalog pages, the new ANN
//!    index pages, and the new header — is written into the WAL region as a
//!    run of `(txn_id, page_no, page_image)` frames, terminated by a
//!    checksummed `COMMIT` frame.
//! 3. **One fsync of the WAL region is the commit point.** Before it, the
//!    transaction does not exist; after it, the transaction is durable even
//!    though no home page has been touched.
//! 4. *Checkpoint* copies each frame's payload to its home page and rewrites
//!    page 0, then fsyncs. The WAL is now spent.
//!
//! On open, [`recover`] replays a committed-but-uncheckpointed transaction.
//! A torn or partial transaction (no valid `COMMIT` frame) is discarded — its
//! pages were never referenced, so the database simply opens at the previous
//! consistent state. This is what the copy-on-write scheme could not give on
//! its own: a single-fsync commit and an explicit, replayable transaction
//! boundary.
//!
//! # On-disk shape of a frame
//!
//! ```text
//!   MAGIC(4) | kind(1) | txn_id(8) | page_no(8) | len(4) | crc32(4) | payload(len)
//! ```
//!
//! `kind` is `1` (DATA — `payload` is a full page image) or `2` (COMMIT —
//! `payload` is empty, `crc32` is the running CRC over every DATA frame in the
//! transaction). DATA payloads are the *same* AES-256-GCM page images written
//! to home pages, so user data in the WAL is encrypted exactly as it is
//! everywhere else; the small plaintext frame metadata (a txn id, a page
//! number) is structural, on a par with the always-plaintext file header.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::error::{MnemoError, Result};
use crate::format::PAGE_SIZE;

/// Frame magic — "MWAL".
const FRAME_MAGIC: &[u8; 4] = b"MWAL";
/// Fixed bytes preceding a frame payload: magic+kind+txn+page+len+crc.
const FRAME_HEADER: usize = 4 + 1 + 8 + 8 + 4 + 4; // 29
/// Frame kind: a home-page image.
const KIND_DATA: u8 = 1;
/// Frame kind: end-of-transaction marker.
const KIND_COMMIT: u8 = 2;

/// One page image bound for a specific home page number.
pub type Frame = (u64, Vec<u8>);

/// Bytes a transaction of `n` DATA frames occupies in the WAL region.
pub fn txn_byte_len(n: usize, page_size: usize) -> u64 {
    // n DATA frames (header + page payload) + one empty COMMIT frame.
    ((n * (FRAME_HEADER + page_size)) + FRAME_HEADER) as u64
}

/// CRC-32 (IEEE 802.3, reflected). Small and dependency-free.
fn crc32(seed: u32, data: &[u8]) -> u32 {
    let mut c = !seed;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            let mask = (c & 1).wrapping_neg();
            c = (c >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !c
}

/// CRC-32 over a standalone buffer.
pub fn checksum(data: &[u8]) -> u32 {
    crc32(0, data)
}

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Write `frames` as one transaction into the WAL region and fsync.
///
/// This is the transaction's **commit point**: when it returns `Ok`, the
/// transaction is durable. The WAL region spans `wal_pages` pages starting at
/// page `wal_start`; an over-large transaction is rejected (the caller is
/// expected to have grown the region first).
pub fn commit(
    file: &mut File,
    wal_start: u64,
    wal_pages: u64,
    txn_id: u64,
    frames: &[Frame],
) -> Result<()> {
    let capacity = wal_pages * PAGE_SIZE as u64;
    let need = txn_byte_len(frames.len(), PAGE_SIZE);
    if need > capacity {
        return Err(MnemoError::Invalid(format!(
            "transaction needs {need} WAL bytes but the region holds {capacity}"
        )));
    }

    let mut buf: Vec<u8> = Vec::with_capacity(need as usize);
    let mut running = 0u32; // CRC chained across every DATA frame.
    for (page_no, payload) in frames {
        buf.extend_from_slice(FRAME_MAGIC);
        buf.push(KIND_DATA);
        put_u64(&mut buf, txn_id);
        put_u64(&mut buf, *page_no);
        put_u32(&mut buf, payload.len() as u32);
        let fc = checksum(payload);
        put_u32(&mut buf, fc);
        buf.extend_from_slice(payload);
        running = crc32(running, payload);
    }
    // COMMIT frame: empty payload, crc field carries the running checksum.
    buf.extend_from_slice(FRAME_MAGIC);
    buf.push(KIND_COMMIT);
    put_u64(&mut buf, txn_id);
    put_u64(&mut buf, 0);
    put_u32(&mut buf, 0);
    put_u32(&mut buf, running);

    file.seek(SeekFrom::Start(wal_start * PAGE_SIZE as u64))?;
    file.write_all(&buf)?;
    file.sync_all()?;
    Ok(())
}

/// Scan the WAL region for a committed transaction newer than `wal_seq`.
///
/// Returns the frames of that transaction (to be replayed to home pages) or
/// `None` when the WAL holds nothing newer — whether because it is empty, was
/// already checkpointed, or contains only a torn, never-committed tail.
pub fn recover(
    file: &mut File,
    wal_start: u64,
    wal_pages: u64,
    wal_seq: u64,
) -> Result<Option<Vec<Frame>>> {
    if wal_pages == 0 {
        return Ok(None);
    }
    let capacity = (wal_pages * PAGE_SIZE as u64) as usize;
    let mut region = vec![0u8; capacity];
    file.seek(SeekFrom::Start(wal_start * PAGE_SIZE as u64))?;
    // The region may run past EOF on a freshly grown file; a short read just
    // leaves trailing zeros, which fail the magic check and end the scan.
    let mut filled = 0usize;
    while filled < capacity {
        match file.read(&mut region[filled..])? {
            0 => break,
            n => filled += n,
        }
    }

    let mut off = 0usize;
    let mut frames: Vec<Frame> = Vec::new();
    let mut running = 0u32;
    let mut txn: Option<u64> = None;

    while off + FRAME_HEADER <= filled {
        if &region[off..off + 4] != FRAME_MAGIC {
            break; // End of valid log (zeros, or stale bytes).
        }
        let kind = region[off + 4];
        let id = rd_u64(&region, off + 5);
        let page_no = rd_u64(&region, off + 13);
        let len = rd_u32(&region, off + 21) as usize;
        let crc = rd_u32(&region, off + 25);
        let body = off + FRAME_HEADER;

        // A transaction must be internally consistent: one txn id throughout.
        match txn {
            None => txn = Some(id),
            Some(t) if t == id => {}
            Some(_) => break, // Different txn id mid-stream — stop.
        }

        match kind {
            KIND_DATA => {
                if body + len > filled {
                    break; // Truncated payload — torn write.
                }
                let payload = &region[body..body + len];
                if checksum(payload) != crc {
                    break; // Corrupt frame — torn write.
                }
                running = crc32(running, payload);
                frames.push((page_no, payload.to_vec()));
                off = body + len;
            }
            KIND_COMMIT => {
                // Valid commit iff the chained checksum matches.
                if crc != running {
                    break;
                }
                let id = txn.unwrap_or(0);
                if id > wal_seq && !frames.is_empty() {
                    return Ok(Some(frames));
                }
                return Ok(None); // Committed, but already checkpointed.
            }
            _ => break,
        }
    }
    // Reached here with no COMMIT frame: the transaction never committed.
    Ok(None)
}
