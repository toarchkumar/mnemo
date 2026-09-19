//! Fuzz target: MCP stdio JSON-RPC framing + request parse (Phase 4).
//!
//! `mnemo serve --mcp` reads newline-delimited JSON from stdin and
//! parses each line as a `Request`. A hostile MCP client (or any
//! process piping into stdin) can send arbitrary bytes; the parse
//! MUST NOT panic. Malformed input must surface as a JSON-RPC error
//! response, not as process death.
//!
//! Input encoding: interpret the bytes as UTF-8-lossy, split on
//! newlines, and feed each line into the parse. This mirrors how the
//! serve loop actually consumes input (line-by-line via `BufRead`).
//! `serde_json::from_str` returns `Err` on invalid JSON; the fuzz
//! target only asserts the process survives.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for line in text.lines() {
        let _ = mnemo::__fuzz::parse_mcp_request_line(line);
    }
});
