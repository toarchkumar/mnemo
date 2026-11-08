//! PERF-1 step 1: a checkpointed WAL region costs no allocation on open.
//!
//! Lives in its own test binary because it installs a counting global
//! allocator, which the `forbid(unsafe_code)` library can't. The counter
//! is thread-local, so only the measured call is counted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use mnemo_engine::__wal;

thread_local! {
    static BYTES: Cell<Option<usize>> = const { Cell::new(None) };
}

struct Counting;

// SAFETY: every call is delegated to `System`; the counter only reads the
// requested size.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = BYTES.try_with(|b| {
            if let Some(n) = b.get() {
                b.set(Some(n + layout.size()));
            }
        });
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn measure<R>(f: impl FnOnce() -> R) -> (R, usize) {
    BYTES.with(|b| b.set(Some(0)));
    let r = f();
    let n = BYTES.with(|b| b.replace(None)).unwrap_or(0);
    (r, n)
}

#[test]
fn recover_skips_a_checkpointed_26mb_region_without_allocating() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(dir.path().join("wal.bin"))
        .unwrap();
    let frames: Vec<(u64, Vec<u8>)> = (0..3_200u64)
        .map(|p| (p + 10, vec![(p % 251) as u8; __wal::PAGE_SIZE]))
        .collect();
    let page = __wal::PAGE_SIZE as u64;
    let pages = __wal::txn_byte_len(frames.len()).div_ceil(page);
    assert!(pages * page > 26_000_000, "region should be ~26 MB");
    __wal::commit(&mut file, pages, 7, &frames).unwrap();

    // Already checkpointed (wal_seq == txn id): nothing to replay, and the
    // region is never read into memory.
    let (r, bytes) = measure(|| __wal::recover(&mut file, pages, 7));
    assert!(r.unwrap().is_none());
    assert!(
        bytes < 64 * 1024,
        "recover allocated {bytes} bytes for a spent region"
    );

    // Newer than wal_seq: replayed in full, exactly as before.
    let replay = __wal::recover(&mut file, pages, 6)
        .unwrap()
        .expect("a newer transaction is replayed");
    assert_eq!(replay.len(), frames.len());
    assert_eq!(replay[17], frames[17]);
}
