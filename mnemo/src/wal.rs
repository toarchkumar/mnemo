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
//! # DUR-1: header changes commit through the WAL too
//!
//! Beyond the per-flush transaction above, every page-0 change after
//! [`crate::Mnemo::create`] — WAL relocation (`ensure_wal_capacity`), rekey
//! (`Mnemo::rekey`) — goes through this same commit path as a one-frame
//! transaction `(0, new_header_page)`. The relocation path writes the frame
//! into the *new* region first, then checkpoints page 0, then
//! [`invalidate_region`]s the old region's first frame header. A crash
//! anywhere in that sequence either preserves the old state (page 0 wasn't
//! overwritten) or lets `Mnemo::open`'s torn-header scan recover the new
//! state from the new region's committed header frame. The initial
//! `Mnemo::create` is the only unlogged page-0 write that remains.
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

/// Slice-by-8 lookup tables for CRC-32 (IEEE 802.3, reflected,
/// polynomial `0xEDB8_8320`), built at compile time. `CRC_TABLES[0]` is the
/// classic byte table; `CRC_TABLES[k]` advances a byte through `k` more
/// zero bytes.
const CRC_TABLES: [[u32; 256]; 8] = {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut k = 1;
    while k < 8 {
        let mut i = 0;
        while i < 256 {
            let prev = t[k - 1][i];
            t[k][i] = (prev >> 8) ^ t[0][(prev & 0xFF) as usize];
            i += 1;
        }
        k += 1;
    }
    t
};

/// CRC-32 (IEEE 802.3, reflected), continuing from `seed`. Dependency-free
/// slice-by-8 (PERF-1: the bit-at-a-time loop it replaces was a large part
/// of every commit and open); the output is bit-identical.
fn crc32(seed: u32, data: &[u8]) -> u32 {
    let t = &CRC_TABLES;
    let mut c = !seed;
    let mut chunks = data.chunks_exact(8);
    for b in &mut chunks {
        let one = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) ^ c;
        let two = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        c = t[7][(one & 0xFF) as usize]
            ^ t[6][((one >> 8) & 0xFF) as usize]
            ^ t[5][((one >> 16) & 0xFF) as usize]
            ^ t[4][(one >> 24) as usize]
            ^ t[3][(two & 0xFF) as usize]
            ^ t[2][((two >> 8) & 0xFF) as usize]
            ^ t[1][((two >> 16) & 0xFF) as usize]
            ^ t[0][(two >> 24) as usize];
    }
    for &b in chunks.remainder() {
        c = t[0][((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
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

    // Phase 1.3 failpoint: crash-before-commit. Runs before the
    // buffer hits the WAL region, so a triggered failure guarantees
    // no commit frame exists on disk — reopen must recover to the
    // pre-flush state. Zero-cost stub in production.
    crate::pager::__failpoint_check()?;
    file.seek(SeekFrom::Start(wal_start * PAGE_SIZE as u64))?;
    file.write_all(&buf)?;
    file.sync_all()?;
    crate::bump_fsync();
    Ok(())
}

/// Zero the first frame header at `region_start` so a scan beginning
/// there finds no `MWAL` magic. Used by [`crate::store::Mnemo`] after a
/// WAL relocation has been checkpointed into page 0 — the old region is
/// now stale, and leaving its magic in place would feed garbage frames
/// to the torn-header scan on reopen.
///
/// Writes a zero-filled `FRAME_HEADER` (29 bytes) and fsyncs. Only the
/// first 4 bytes (the magic) *need* to go to make the region invalid,
/// but zeroing the whole frame header is cheap and sidesteps any
/// debate about what a "half-zeroed" magic means.
pub fn invalidate_region(file: &mut File, region_start: u64) -> Result<()> {
    crate::pager::__failpoint_check()?;
    let zeros = [0u8; FRAME_HEADER];
    file.seek(SeekFrom::Start(region_start * PAGE_SIZE as u64))?;
    file.write_all(&zeros)?;
    file.sync_all()?;
    crate::bump_fsync();
    Ok(())
}

/// Magic prefix of a frame on disk. Four ASCII bytes `MWAL`. Exposed
/// so the torn-header scan in `Mnemo::open` can test for `MWAL` at
/// every 8 KiB-aligned page start without reaching into this module.
pub const FRAME_MAGIC_BYTES: &[u8; 4] = FRAME_MAGIC;

/// Streaming scan of a candidate WAL region, starting at `region_start`
/// pages, with **no size cap** on the transaction. Reads one frame
/// header at a time (29 bytes), then the payload declared by `len`,
/// chaining `KIND_DATA` payloads into the running CRC until a
/// `KIND_COMMIT` frame closes the transaction.
///
/// Returns `Some((txn_id, frames))` only when a valid committed
/// transaction is found: all frames carry the same `txn_id`, every
/// payload's per-frame CRC verifies, and the `KIND_COMMIT` frame's
/// chained CRC matches the running CRC. Returns `Ok(None)` for any
/// other outcome — bad magic, truncated payload, CRC mismatch,
/// different `txn_id` mid-stream, EOF before `KIND_COMMIT`.
///
/// Used by the torn-header scan in `Mnemo::open`, which must handle a
/// relocated 1M-vector IVF+PQ transaction whose control plane can run
/// well past 16 MiB — the pre-DUR-1 "read a 16 MiB window and call
/// [`recover_bytes`]" approach would silently truncate such a
/// transaction before its commit frame.
pub fn scan_committed_txn(file: &mut File, region_start: u64) -> Result<Option<(u64, Vec<Frame>)>> {
    let region_byte_offset = region_start * PAGE_SIZE as u64;
    file.seek(SeekFrom::Start(region_byte_offset))?;

    let mut frames: Vec<Frame> = Vec::new();
    let mut running = 0u32;
    let mut txn: Option<u64> = None;
    let mut header_buf = [0u8; FRAME_HEADER];

    loop {
        // Read one frame header exactly. A short read here means EOF
        // or a truncated final frame — either way, no commit.
        if let Err(e) = file.read_exact(&mut header_buf) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e.into());
        }
        if &header_buf[0..4] != FRAME_MAGIC {
            return Ok(None);
        }
        let kind = header_buf[4];
        let id = rd_u64(&header_buf, 5);
        let page_no = rd_u64(&header_buf, 13);
        let len = rd_u32(&header_buf, 21) as usize;
        let crc = rd_u32(&header_buf, 25);

        // Enforce single-transaction scan: every frame in a committed
        // transaction carries the same id.
        match txn {
            None => txn = Some(id),
            Some(t) if t == id => {}
            Some(_) => return Ok(None),
        }

        match kind {
            KIND_DATA => {
                // Streamed payload read; a short read is a torn frame.
                let mut payload = vec![0u8; len];
                if let Err(e) = file.read_exact(&mut payload) {
                    if e.kind() == std::io::ErrorKind::UnexpectedEof {
                        return Ok(None);
                    }
                    return Err(e.into());
                }
                if checksum(&payload) != crc {
                    return Ok(None);
                }
                running = crc32(running, &payload);
                frames.push((page_no, payload));
            }
            KIND_COMMIT => {
                // Chained CRC decides whether the transaction committed.
                if crc != running {
                    return Ok(None);
                }
                let id = txn.unwrap_or(0);
                if frames.is_empty() {
                    return Ok(None);
                }
                return Ok(Some((id, frames)));
            }
            _ => return Ok(None),
        }
    }
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
    // PERF-1: peek at the first frame header before reading anything else.
    // A transaction always starts at the region's first byte and every one
    // of its frames carries the same txn id, so a first frame whose id is
    // `<= wal_seq` means the region holds an already-checkpointed (or an
    // older, never-committed) transaction. The full scan below would reach
    // the same `None`, after reading, checksumming and copying the whole
    // transaction. An empty or zeroed region fails the magic check here,
    // as it would below.
    file.seek(SeekFrom::Start(wal_start * PAGE_SIZE as u64))?;
    let mut head = [0u8; FRAME_HEADER];
    let mut got = 0usize;
    while got < FRAME_HEADER {
        match file.read(&mut head[got..])? {
            0 => break,
            n => got += n,
        }
    }
    if got < FRAME_HEADER || &head[0..4] != FRAME_MAGIC || rd_u64(&head, 5) <= wal_seq {
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
    region.truncate(filled);
    recover_bytes(&region, wal_seq)
}

/// Byte-slice variant of [`recover`] that skips the `File::read` step.
/// Same frame-scan semantics; extracted so fuzz targets can drive the
/// scan without an on-disk file. The buffer is treated as the WAL
/// region starting at frame 0 — trailing zeros or garbage past the
/// last committed frame end the scan cleanly, same as `recover`'s
/// short-read behavior.
///
/// Marked `#[doc(hidden)]` because it's a fuzz/testability affordance,
/// not a stable public API — callers who have a `File` should use
/// [`recover`] instead.
#[doc(hidden)]
pub fn recover_bytes(region: &[u8], wal_seq: u64) -> Result<Option<Vec<Frame>>> {
    let filled = region.len();
    let mut off = 0usize;
    let mut frames: Vec<Frame> = Vec::new();
    let mut running = 0u32;
    let mut txn: Option<u64> = None;

    while off + FRAME_HEADER <= filled {
        if &region[off..off + 4] != FRAME_MAGIC {
            break; // End of valid log (zeros, or stale bytes).
        }
        let kind = region[off + 4];
        let id = rd_u64(region, off + 5);
        let page_no = rd_u64(region, off + 13);
        let len = rd_u32(region, off + 21) as usize;
        let crc = rd_u32(region, off + 25);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bit-at-a-time CRC the slice-by-8 version replaced.
    fn crc32_bitwise(seed: u32, data: &[u8]) -> u32 {
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

    #[test]
    fn crc32_matches_the_bitwise_reference() {
        assert_eq!(checksum(b"123456789"), 0xCBF4_3926);
        let data: Vec<u8> = (0..20_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        for len in (0..64).chain([255, 256, 1_000, 8_192, 20_000]) {
            for seed in [0u32, 0xDEAD_BEEF] {
                assert_eq!(
                    crc32(seed, &data[..len]),
                    crc32_bitwise(seed, &data[..len]),
                    "len {len}"
                );
            }
        }
        // Chaining across a split equals one pass.
        let whole = crc32(0, &data);
        assert_eq!(crc32(crc32(0, &data[..777]), &data[777..]), whole);
    }
}
