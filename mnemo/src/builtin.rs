//! 12.3 step 5 (v0.15): the built-in embedder, behind the `embed-candle`
//! feature (on in the release CLI binaries and wheels, off for the library
//! and the mobile packages).
//!
//! **The model** is [`MODEL`] (`Snowflake/snowflake-arctic-embed-xs`,
//! Apache-2.0) at the pinned [`REVISION`]: a 6-layer BERT with 384
//! dimensions, CLS pooling, up to 512 tokens, unit-normalised. Queries are
//! embedded with the model's own instruction ([`QUERY_PREFIX`]), recorded
//! in the identity as the [`QUERY_CONVENTION`]. It runs on candle, in pure
//! Rust on the CPU.
//!
//! **The files** ([`FILES`]: `config.json`, `tokenizer.json`,
//! `model.safetensors`, 91 MB together) aren't in the binary. They're
//! fetched once, over HTTPS, into the platform's cache directory
//! ([`model_dir`]), either on first use ([`BuiltinEmbedder::load_or_install`],
//! which reports the size first) or ahead of time with [`install`] (`mnemo
//! embedder install`). Each file's SHA-256 is in the binary and is checked
//! before the model is loaded, every time, so a changed or truncated file is
//! refused, never used.
//!
//! **A model directory of your own** ([`BuiltinEmbedder::from_dir`], the
//! CLI's `--model-dir`) holds the same three files. When they're the pinned
//! files, it's the built-in embedder. Otherwise it's loaded the same way
//! (BERT, CLS pooling, the arctic query prefix) under an identity that
//! records each file's SHA-256, so a file embedded with it can't be read
//! with another model by mistake.
//!
//! **Identifier-aware recall** (built-in identity only): a recall by text
//! scores no keywords for the question's plain words, since arctic-xs's
//! vector does better alone; the identifiers in it (`snapshot_list`,
//! `--features`, `RecallRequest::from_text`, `store.rs`, …) become explicit
//! keywords instead ([`crate::Embedder::query_keywords`]). Keywords the
//! caller gives still win.
//!
//! **Environment:** `MNEMO_CACHE_DIR` moves the cache (the model goes under
//! `<dir>/models/`). `MNEMO_MODEL_URL` replaces `https://huggingface.co` as
//! the download's base, for a mirror; the hashes are checked all the same.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use sha2::{Digest, Sha256};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use crate::embed::{Embedder, EmbedderIdentity};
use crate::error::{MnemoError, Result};

/// The model's Hugging Face repository.
pub const MODEL: &str = "Snowflake/snowflake-arctic-embed-xs";
/// The pinned revision (a commit of [`MODEL`]'s repository).
pub const REVISION: &str = "d8c86521100d3556476a063fc2342036d45c106f";
/// The vector width.
pub const DIMENSIONS: usize = 384;
/// The longest input, in tokens; longer text is truncated.
pub const MAX_TOKENS: usize = 512;
/// The instruction arctic-xs puts before a query (not before a document).
pub const QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";
/// The identity's name for that query side
/// ([`EmbedderIdentity::query_convention`]).
pub const QUERY_CONVENTION: &str = "arctic-query-prefix";
/// Where the files come from unless `MNEMO_MODEL_URL` says otherwise.
const DEFAULT_BASE_URL: &str = "https://huggingface.co";
/// Texts per forward pass.
const BATCH: usize = 8;

/// One of the model's files, as pinned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// The file's name, in the repository and in the model directory.
    pub name: &'static str,
    /// Its SHA-256, in lowercase hex.
    pub sha256: &'static str,
    /// Its size in bytes.
    pub size: u64,
}

/// The model's three files at [`REVISION`].
pub const FILES: [ModelFile; 3] = [
    ModelFile {
        name: "config.json",
        sha256: "d7d071046ab952af96b7abad788db7ab3fc997b465e1b9914ff39707092254ec",
        size: 737,
    },
    ModelFile {
        name: "tokenizer.json",
        sha256: "91f1def9b9391fdabe028cd3f3fcc4efd34e5d1f08c3bf2de513ebb5911a1854",
        size: 711_649,
    },
    ModelFile {
        name: "model.safetensors",
        sha256: "ee789e0b1d6ecbbd5ce37b474af556cc1a1319cee4417d9e3b11f82e90300706",
        size: 90_272_656,
    },
];

/// The download's size in bytes (all of [`FILES`]).
pub fn download_size() -> u64 {
    FILES.iter().map(|f| f.size).sum()
}

/// `bytes` as megabytes for a person, such as `91.0 MB`.
pub fn human_size(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// The built-in embedder's identity: [`MODEL`]`@`[`REVISION`], 384
/// dimensions, normalised, with the arctic query convention. A file
/// embedded with it records this identity.
pub fn identity() -> EmbedderIdentity {
    EmbedderIdentity {
        name: format!("{MODEL}@{REVISION}"),
        dimensions: DIMENSIONS,
        normalize: true,
        query_convention: QUERY_CONVENTION.to_string(),
    }
}

/// The cache directory: `MNEMO_CACHE_DIR` if set, otherwise the platform's
/// (`~/Library/Caches/mnemo` on macOS, `%LOCALAPPDATA%\mnemo\cache` on
/// Windows, `$XDG_CACHE_HOME/mnemo` or `~/.cache/mnemo` elsewhere).
pub fn cache_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("MNEMO_CACHE_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    let home = || {
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
    };
    let dir = if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Caches").join("mnemo"))
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("mnemo").join("cache"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|d| d.is_absolute())
            .map(|d| d.join("mnemo"))
            .or_else(|| home().map(|h| h.join(".cache").join("mnemo")))
    };
    dir.ok_or_else(|| {
        MnemoError::Invalid(
            "the built-in embedder has no cache directory (no HOME); set MNEMO_CACHE_DIR".into(),
        )
    })
}

/// Where the pinned model's files are kept:
/// `<cache>/models/snowflake-arctic-embed-xs/<revision>/`.
pub fn model_dir() -> Result<PathBuf> {
    Ok(cache_dir()?
        .join("models")
        .join("snowflake-arctic-embed-xs")
        .join(REVISION))
}

/// Whether every file is in [`model_dir`] at its pinned size. The hashes are
/// checked when the model loads.
pub fn is_installed() -> bool {
    model_dir().is_ok_and(|d| {
        FILES.iter().all(|f| {
            std::fs::metadata(d.join(f.name)).is_ok_and(|m| m.is_file() && m.len() == f.size)
        })
    })
}

/// Fetch whatever [`model_dir`] is missing, checking each file's size and
/// SHA-256 before it takes its place, and return the directory. `notice`
/// hears, before any download, what's fetched, its size, and where to.
/// Nothing is fetched when the files are there; a file at the wrong size
/// is fetched again.
pub fn install(notice: &mut dyn FnMut(&str)) -> Result<PathBuf> {
    let dir = model_dir()?;
    let missing: Vec<&ModelFile> = FILES
        .iter()
        .filter(|f| std::fs::metadata(dir.join(f.name)).map_or(true, |m| m.len() != f.size))
        .collect();
    if missing.is_empty() {
        return Ok(dir);
    }
    let bytes: u64 = missing.iter().map(|f| f.size).sum();
    notice(&format!(
        "mnemo: downloading the built-in embedder ({MODEL}, {}) into {}",
        human_size(bytes),
        dir.display()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| {
        MnemoError::Invalid(format!(
            "the built-in embedder can't create {}: {e}",
            dir.display()
        ))
    })?;
    let base = std::env::var("MNEMO_MODEL_URL")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let base = base.trim_end_matches('/');
    for f in missing {
        let url = format!("{base}/{MODEL}/resolve/{REVISION}/{}", f.name);
        fetch(&url, &dir.join(f.name), f.sha256, f.size)?;
    }
    Ok(dir)
}

/// Download `url` to `dest`, through a temporary file beside it that
/// replaces `dest` only once its size and SHA-256 match.
pub(crate) fn fetch(url: &str, dest: &Path, sha256: &str, size: u64) -> Result<()> {
    let fail = |what: String| MnemoError::Invalid(format!("the built-in embedder's {what}"));
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("model file");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        .timeout_per_read(Some(std::time::Duration::from_secs(60)))
        .user_agent(concat!("mnemo/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| fail(format!("download of {name} failed ({url}): {e}")))?;
    let tmp = dest.with_file_name(format!("{name}.{}.part", std::process::id()));
    let result = (|| {
        let mut out = std::fs::File::create(&tmp)
            .map_err(|e| fail(format!("download can't write {}: {e}", tmp.display())))?;
        // One byte over the size is enough to tell a too-long body.
        let mut body = resp.body_mut().with_config().limit(size + 1).reader();
        let mut hasher = Sha256::new();
        let mut got: u64 = 0;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = match body.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    // ureq reports the limit as an error: the body is too long.
                    if got + buf.len() as u64 > size {
                        return Err(fail(format!(
                            "{name} is larger than the {size} bytes pinned"
                        )));
                    }
                    return Err(fail(format!("download of {name} failed: {e}")));
                }
            };
            got += n as u64;
            if got > size {
                return Err(fail(format!(
                    "{name} is larger than the {size} bytes pinned"
                )));
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(|e| fail(format!("download can't write {}: {e}", tmp.display())))?;
        }
        if got != size {
            return Err(fail(format!(
                "{name} is {got} bytes, not the {size} pinned (an interrupted download?)"
            )));
        }
        let digest = hex(&hasher.finalize());
        if digest != sha256 {
            return Err(fail(format!(
                "{name} doesn't match its pinned SHA-256 (got {digest}); nothing was installed"
            )));
        }
        out.sync_all()
            .map_err(|e| fail(format!("download can't write {}: {e}", tmp.display())))?;
        drop(out);
        std::fs::rename(&tmp, dest)
            .map_err(|e| fail(format!("download can't move {name} into place: {e}")))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The built-in embedder: arctic-xs on candle, as an [`Embedder`].
///
/// Load it with [`BuiltinEmbedder::load_or_install`] (fetching the files on
/// first use), [`BuiltinEmbedder::load`] (the cached files only) or
/// [`BuiltinEmbedder::from_dir`], then register it with
/// [`crate::Mnemo::set_embedder`].
pub struct BuiltinEmbedder {
    model: BertModel,
    tokenizer: Mutex<Tokenizer>,
    identity: EmbedderIdentity,
    /// Whether the files are the pinned ones (the built-in identity), which
    /// turns on identifier-aware recall.
    builtin: bool,
}

impl std::fmt::Debug for BuiltinEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltinEmbedder")
            .field("identity", &self.identity)
            .field("builtin", &self.builtin)
            .finish_non_exhaustive()
    }
}

impl BuiltinEmbedder {
    /// Load the model from [`model_dir`], fetching it first if it isn't
    /// there ([`install`]; `notice` hears the size before the download).
    pub fn load_or_install(notice: &mut dyn FnMut(&str)) -> Result<Self> {
        let dir = install(notice)?;
        Self::load_pinned(&dir)
    }

    /// Load the model from [`model_dir`] without fetching anything. Fails
    /// if a file is missing or doesn't match its pinned hash.
    pub fn load() -> Result<Self> {
        let dir = model_dir()?;
        if !is_installed() {
            return Err(MnemoError::Invalid(format!(
                "the built-in embedder isn't installed in {} (run `mnemo embedder install`)",
                dir.display()
            )));
        }
        Self::load_pinned(&dir)
    }

    /// Load the three files from a directory of your own. When they're the
    /// pinned files this is the built-in embedder; otherwise its identity
    /// is named by the files' SHA-256s
    /// (`local:config.json=<sha>,tokenizer.json=<sha>,model.safetensors=<sha>`)
    /// and recall keeps the default keyword weight.
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let files = read_files(dir)?;
        let hashes: Vec<String> = files.iter().map(|b| hex(&Sha256::digest(b))).collect();
        let pinned = FILES.iter().zip(&hashes).all(|(f, h)| f.sha256 == h);
        let mut e = Self::build(files)?;
        if !pinned {
            let named: Vec<String> = FILES
                .iter()
                .zip(&hashes)
                .map(|(f, h)| format!("{}={h}", f.name))
                .collect();
            e.identity.name = format!("local:{}", named.join(","));
            e.builtin = false;
        }
        Ok(e)
    }

    /// Whether this is the pinned model (the built-in identity).
    pub fn is_builtin(&self) -> bool {
        self.builtin
    }

    fn load_pinned(dir: &Path) -> Result<Self> {
        let files = read_files(dir)?;
        for (f, bytes) in FILES.iter().zip(&files) {
            let got = hex(&Sha256::digest(bytes));
            if got != f.sha256 {
                return Err(MnemoError::Invalid(format!(
                    "the built-in embedder's {} in {} doesn't match its pinned SHA-256 \
                     (got {got}); delete it and run `mnemo embedder install`",
                    f.name,
                    dir.display()
                )));
            }
        }
        Self::build(files)
    }

    /// The model from its three files' bytes, in [`FILES`] order.
    fn build(files: [Vec<u8>; 3]) -> Result<Self> {
        let err = |what: &str, e: String| {
            MnemoError::Invalid(format!("the built-in embedder can't load its {what}: {e}"))
        };
        let [config, tokenizer, weights] = files;
        let config: Config =
            serde_json::from_slice(&config).map_err(|e| err("config.json", e.to_string()))?;
        let device = Device::Cpu;
        // The weights are read into memory, not mapped, so no `unsafe`.
        let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)
            .map_err(|e| err("model.safetensors", e.to_string()))?;
        let model =
            BertModel::load(vb, &config).map_err(|e| err("model.safetensors", e.to_string()))?;
        let mut tok =
            Tokenizer::from_bytes(&tokenizer).map_err(|e| err("tokenizer.json", e.to_string()))?;
        tok.with_truncation(Some(TruncationParams {
            max_length: MAX_TOKENS,
            ..Default::default()
        }))
        .map_err(|e| err("tokenizer.json", e.to_string()))?;
        tok.with_padding(Some(PaddingParams::default()));
        let mut identity = identity();
        identity.dimensions = config.hidden_size;
        Ok(BuiltinEmbedder {
            model,
            tokenizer: Mutex::new(tok),
            identity,
            builtin: true,
        })
    }

    /// Embed `texts` in batches: padded within a batch, masked, CLS-pooled
    /// and normalised.
    fn vectors(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let fail = |e: String| MnemoError::Invalid(format!("the built-in embedder failed: {e}"));
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(BATCH) {
            let enc = self
                .tokenizer
                .lock()
                .map_err(|_| fail("its tokenizer lock is poisoned".into()))?
                .encode_batch(chunk.to_vec(), true)
                .map_err(|e| fail(e.to_string()))?;
            let pooled = (|| -> candle_core::Result<Tensor> {
                let ids: Vec<Tensor> = enc
                    .iter()
                    .map(|e| Tensor::new(e.get_ids(), &Device::Cpu))
                    .collect::<candle_core::Result<_>>()?;
                let mask: Vec<Tensor> = enc
                    .iter()
                    .map(|e| Tensor::new(e.get_attention_mask(), &Device::Cpu))
                    .collect::<candle_core::Result<_>>()?;
                let ids = Tensor::stack(&ids, 0)?;
                let mask = Tensor::stack(&mask, 0)?;
                let types = ids.zeros_like()?;
                let hidden = self.model.forward(&ids, &types, Some(&mask))?;
                // CLS pooling: the first token's vector.
                let cls = hidden.narrow(1, 0, 1)?.squeeze(1)?;
                let norm = cls.sqr()?.sum_keepdim(1)?.sqrt()?;
                cls.broadcast_div(&norm)
            })()
            .map_err(|e| fail(e.to_string()))?;
            out.extend(pooled.to_vec2::<f32>().map_err(|e| fail(e.to_string()))?);
        }
        Ok(out)
    }
}

/// The three files' bytes from `dir`, in [`FILES`] order.
fn read_files(dir: &Path) -> Result<[Vec<u8>; 3]> {
    let read = |f: &ModelFile| {
        std::fs::read(dir.join(f.name)).map_err(|e| {
            MnemoError::Invalid(format!(
                "the built-in embedder can't read {}: {e}",
                dir.join(f.name).display()
            ))
        })
    };
    Ok([read(&FILES[0])?, read(&FILES[1])?, read(&FILES[2])?])
}

impl Embedder for BuiltinEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        self.identity.clone()
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.vectors(&texts.iter().map(|t| t.to_string()).collect::<Vec<_>>())
    }

    fn embed_query(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.vectors(
            &texts
                .iter()
                .map(|t| format!("{QUERY_PREFIX}{t}"))
                .collect::<Vec<_>>(),
        )
    }

    fn query_keywords(&self, text: &str) -> Option<String> {
        // Identifier-aware recall, for the pinned model only: the question's
        // identifiers as explicit keywords, or none (vector-only).
        self.builtin
            .then(|| crate::ident::identifiers(text).join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    /// Serve `body` once over plain HTTP on a local port; returns the URL.
    fn serve_once(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut r = std::io::BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            while r.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                line.clear();
            }
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            s.write_all(&body).unwrap();
        });
        format!("http://{addr}/f")
    }

    #[test]
    fn a_download_takes_its_place_only_when_it_matches() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("config.json");
        let body = b"{\"hidden_size\": 384}".to_vec();
        let sha = hex(&Sha256::digest(&body));
        fetch(&serve_once(body.clone()), &dest, &sha, body.len() as u64).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);

        // The wrong bytes: refused, nothing left behind, the old file kept.
        let bad = b"{\"hidden_size\": 768}".to_vec();
        let e = fetch(
            &serve_once(bad.clone()),
            &dest,
            &"0".repeat(64),
            bad.len() as u64,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("doesn't match its pinned SHA-256"), "{e}");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        // Too short and too long.
        let e = fetch(
            &serve_once(body.clone()),
            &dest,
            &sha,
            body.len() as u64 + 5,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("not the"), "{e}");
        let e = fetch(&serve_once(body.clone()), &dest, &sha, 4)
            .unwrap_err()
            .to_string();
        assert!(e.contains("larger than"), "{e}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_unreachable_server_is_an_error_not_a_hang() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let dir = tempfile::tempdir().unwrap();
        let e = fetch(
            &format!("http://{addr}/f"),
            &dir.path().join("x"),
            &"0".repeat(64),
            1,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("download of x failed"), "{e}");
    }

    #[test]
    fn the_pins() {
        assert_eq!(download_size(), 90_985_042);
        assert_eq!(human_size(download_size()), "91.0 MB");
        let id = identity();
        assert_eq!(
            id.name,
            "Snowflake/snowflake-arctic-embed-xs@d8c86521100d3556476a063fc2342036d45c106f"
        );
        assert_eq!((id.dimensions, id.normalize), (384, true));
        assert_eq!(id.query_convention, "arctic-query-prefix");
        for f in FILES {
            assert_eq!(f.sha256.len(), 64);
        }
    }
}
