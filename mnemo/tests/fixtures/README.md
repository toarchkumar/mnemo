# Test fixtures

Every file here is **synthetic test data**. None holds real memories, and
every key and passphrase that opens one is a throwaway test value, written
in the tests in plain sight on purpose. They're committed so the tests can
read files written by other builds (the published v0.12.3 engine, the
mobile binding), which the current build can't write any more.

`.gitignore` lets `mnemo/tests/fixtures/**/*.mnemo` through (AGENTS.md,
"Never commit", names this as the one exception). Nothing outside this
folder is ever committed as a `.mnemo` file, and no real secret goes in
here.

None of it ships: `mnemo/Cargo.toml` excludes `tests/` from the published
crate, and the PyPI sdist follows it.

| Path | What it is | Written by | Unlocks with |
|---|---|---|---|
| `v8/v8_passphrase.mnemo` | a v8 file with every record kind: 17 memories, a keyed history, a thread and its essence, project essence, a manifest, a cache entry, an index and 7 snapshots (`v8/expected.json` lists them) | the published `mnemo-engine 0.12.3`, through `v8/generator` | the passphrase `v8-fixture-passphrase` |
| `v8/v8_key.mnemo` | a small raw-key v8 file (3 memories) | `v8/generator` | the key in `v8/v8_key.key` (64 hex digits) |
| `v8/v8_zero_vectors.mnemo` | a v8 file with a legacy all-zero-vector manifest and memory | `examples/gen_v8_zero_vector_fixture.rs` (v0.13 step 2a, the last engine that wrote v8) | `v8-fixture-passphrase` |
| `v8/v8_pending_wal.mnemo` | a v8 file left mid-commit: 10 memories checkpointed, 5 more committed in the WAL but not checkpointed | `v8/generator` (`pending-wal`) | `v8-fixture-passphrase` |
| `inspect/ios_export.mnemo` | a passphrase export built through the mobile binding's calls in the iOS sample's order: notes, a keyed fact, a thread with essence | the uniffi test `write_the_inspect_fixture` | the passphrase `inspect-fixture-passphrase` |

To regenerate the v8 files, run the generator crate (it has its own
workspace and pins `mnemo-engine = "=0.12.3"` from crates.io):

```sh
cd mnemo/tests/fixtures/v8/generator
cargo run --locked                 # v8_passphrase.mnemo, v8_key.mnemo, expected.json
cargo run --locked -- pending-wal  # v8_pending_wal.mnemo
```

The inspector fixture: `cargo test -p mnemo-uniffi -- --ignored write_the_inspect_fixture`.
