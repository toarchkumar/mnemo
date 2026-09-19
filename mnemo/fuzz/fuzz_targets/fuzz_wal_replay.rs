//! Fuzz target: WAL replay scan (`wal::recover_bytes`).
//!
//! On open, `Mnemo::open` calls `wal::recover` to replay any
//! committed-but-uncheckpointed transaction; a hostile file can put
//! anything in the WAL region and the scan MUST NOT panic. The
//! byte-slice variant `recover_bytes` is what fuzzes cleanly (no
//! `File` required); production `recover` reads the file into a Vec
//! then delegates to `recover_bytes`, so any panic here is a panic
//! there too.
//!
//! Input encoding: first 8 bytes = `wal_seq` (u64 LE, the "last
//! checkpointed" sequence — recovery only fires for txn ids `> wal_seq`),
//! rest = the WAL region bytes. Short inputs are early-returned so
//! libFuzzer doesn't waste cycles on trivial buffers.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let wal_seq = u64::from_le_bytes(data[..8].try_into().unwrap());
    let region = &data[8..];
    let _ = mnemo::__fuzz::wal_recover_bytes(region, wal_seq);
});
