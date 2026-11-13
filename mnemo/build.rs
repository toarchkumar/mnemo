//! 12.4 step 2a: on Windows, the engine's shared mode needs `std::fs::rename`
//! to replace a file another handle has open, which std does only from Rust
//! 1.85 (probed on CI, 2026-11-13: 1.75 to 1.84 refuse with os error 5). A
//! Windows build with an older compiler sets `mnemo_windows_old_rename`, and
//! shared mode then refuses to start with a clear error. Every other build,
//! the released binaries and wheels included, is unaffected.

use std::process::Command;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(mnemo_windows_old_rename)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RUSTC");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if windows && rustc_minor().is_some_and(|minor| minor < 85) {
        println!("cargo:rustc-cfg=mnemo_windows_old_rename");
    }
}

/// The minor version of the compiler building this crate (`rustc -V` reads
/// "rustc 1.84.1 (...)"), or `None` if it can't be read.
fn rustc_minor() -> Option<u32> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = Command::new(rustc).arg("-V").output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let version = text.split_whitespace().nth(1)?;
    let mut parts = version.split('.');
    if parts.next()? != "1" {
        return None;
    }
    parts.next()?.parse().ok()
}
