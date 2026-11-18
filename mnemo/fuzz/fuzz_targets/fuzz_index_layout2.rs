//! Fuzz target: the ANN index's layout-2 decoder (PERF-2, v0.19.0;
//! RB-015.2). A v9 file's index run is a layout byte and then, in layout 2,
//! flat little-endian arrays whose lengths `IvfPqIndex::read_layout2` checks
//! before allocating. A damaged or hostile run must return `Err`, never
//! panic; one that decodes is queried, changed and written again.
//!
//! The first input byte picks the mode: even decodes the rest as a run body,
//! odd applies the rest as edits to a valid body (so the fuzzer starts from
//! a well-formed run, not only from noise).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let _ = if mode & 1 == 0 {
        mnemo_engine::__fuzz::decode_index_layout2(rest)
    } else {
        mnemo_engine::__fuzz::mutate_index_layout2(rest)
    };
});
