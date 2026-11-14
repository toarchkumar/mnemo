# mnemo-uniffi

The [UniFFI](https://github.com/mozilla/uniffi-rs) binding of the MNemo
engine: the Rust side of the Swift and Kotlin packages (MOBILE-1). Not
published on its own; the Swift package (`mnemo/bindings/swift/`) and the
Kotlin package (`mnemo/bindings/kotlin/`) are built from it.

- **UniFFI `=0.32.2`**, proc-macro mode (no UDL file).
- **Minimum Rust 1.92** for this crate, the engine's (since v0.15; the
  `msrv-uniffi` CI job). Nothing in the lockfile is held back for it any
  more.
- **License note:** UniFFI is MPL-2.0 (file-level copyleft; we don't modify
  it). The Kotlin bindings also load JNA at runtime (Apache-2.0 or
  LGPL-2.1); the Kotlin package's README and NOTICE list it.

## The API

One object, `MnemoDb`, plus a few free functions. Metadata crosses the
FFI as a JSON object string; keys as bytes, never strings.

**Key bytes leave Rust as a copy it can't wipe.** `generate_key` and
`import_with_passphrase` return a key that UniFFI copies into a buffer the
host language owns and Rust never frees, so the engine can't zero that copy
the way it zeroes its own. The Swift and Kotlin stores zero the buffers they
own as soon as the key is stored.

| area | calls |
|---|---|
| open and lifecycle | `MnemoDb.create`, `create_with_key`, `open`, `open_with_key`, `open_read_only`, `open_read_only_with_key`, `close`, `flush`, `len`, `is_empty`, `dimensions`, `path`; free `generate_key`, `compact_file`, `compact_file_with_key` |
| embedder | `set_embedder(MnemoEmbedder)`: implemented in Swift or Kotlin |
| writes | `remember`, `remember_text` (`RememberOptions`: type, importance, metadata, key, TTL, agent), `update` (`MemoryPatch`), `delete` |
| reads | `get`, `history`, `recall`, `recall_text` (`RecallOptions`: top-k, types, keywords, filter, `max_bytes`), `about`, `essence`, `stats` |
| threads | `ingest_thread`, `thread_export`, `thread_forget` |
| result cache | `cache_put`, `cache_get` (exact key) |
| tuning | `set_vector_cache_budget`, `set_cache_capacity` |
| moving devices | `export_with_passphrase` (a copy that opens with a passphrase alone); free `import_with_passphrase` (a copy re-protected by a new device key, returned to the app) |

**Mobile defaults:** a 16 MiB vector column budget and a 2 MiB page cache
(the engine's desktop defaults are 96 MiB and 8 MiB). `MnemoConfig` and
the two tuning calls change them.

**The embedder** is called synchronously on the calling thread and never
while the handle's lock is held, so it may call back into the same
database. A throw becomes `MnemoError.Embedder`, and nothing is written.

**Threads of the host language:** a `MnemoDb` may be used from any thread;
calls are serialized and blocking, so keep them off the main thread (the
Swift and Kotlin layers add async wrappers).

**Errors:** one enum, `MnemoError` (Swift) / `MnemoException` (Kotlin):
`Locked`, `WrongPassphrase`, `HeaderTampered`, `UnsupportedVersion`,
`NotFound`, `ReadOnly`, `DimensionMismatch`, `Invalid`, `Io`, `Corrupt`,
`Closed`, `Embedder`. No panic crosses the FFI; a handle whose lock was
poisoned by a panic reports `Corrupt` on every later call (reopen the
file).

## Building and generating bindings

```bash
cd mnemo
cargo test -p mnemo-uniffi
cargo build -p mnemo-uniffi --release --lib
cargo run -p mnemo-uniffi --features cli --bin uniffi-bindgen -- \
  generate --library target/release/libmnemo_uniffi.dylib --language swift --out-dir out/swift
```

Use `--language kotlin` for Kotlin, and the `.so` on Linux.
