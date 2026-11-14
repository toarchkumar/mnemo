//! 12.3: keys (`docs/design/12.3-keys.md`).
//!
//! The parts `mnemo init`, `recovery-code`, `rekey` and, later, `onboard`,
//! `unlock` and the MCP server are made of:
//! - [`recovery`]: the recovery code's encoder and parser;
//! - [`KeyStore`]: where a brain's secret is kept, found by its salt;
//! - [`resolve`]: the chain of key sources, picked by the file's kind;
//! - [`create_generated`] and [`rekey_stored`]: the orders that keep a
//!   secret that opens the file at every step;
//! - [`display`]: the rule for showing a recovery code.
//!
//! The OS keychain store arrives in step 2. Until then
//! [`default_store`] is `None` in a release build.

pub mod display;
#[cfg(all(
    feature = "keychain",
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
mod os_store;
pub mod recovery;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use zeroize::Zeroizing;

pub use crate::crypto::SecretKind;
use crate::crypto::{KdfParams, SALT_LEN};
use crate::error::{MnemoError, Result};
use crate::format::{Header, PAGE_SIZE};
use crate::store::{Mnemo, MnemoConfig};

/// A secret that opens a brain.
pub enum Secret {
    /// A passphrase a person chose.
    Passphrase(Zeroizing<String>),
    /// A raw 256-bit key.
    Key(Zeroizing<[u8; 32]>),
}

impl Secret {
    /// Which kind it is.
    pub fn kind(&self) -> SecretKind {
        match self {
            Secret::Passphrase(_) => SecretKind::Passphrase,
            Secret::Key(_) => SecretKind::Key,
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Secret::Passphrase(p) => p.as_bytes(),
            Secret::Key(k) => &k[..],
        }
    }
}

/// What a brain's page 0 says about its key, read without the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileKeyInfo {
    /// The salt, which names the brain's keychain entry.
    pub salt: [u8; SALT_LEN],
    /// The KDF parameters.
    pub kdf: KdfParams,
    /// The kind of secret the parameters say it takes: raw-key parameters
    /// mean a key, anything else a passphrase.
    pub kind: SecretKind,
}

/// Read the salt and KDF parameters from `path`'s page 0, with no key and
/// no lock. A torn page 0 is an error here (step 2 adds the salts from the
/// WAL's header frames).
pub fn file_info(path: impl AsRef<Path>) -> Result<FileKeyInfo> {
    let mut file = std::fs::File::open(path.as_ref())?;
    let mut page = [0u8; PAGE_SIZE];
    file.read_exact(&mut page)?;
    let h = Header::from_page(&page)?;
    let kdf = KdfParams {
        m_cost: h.m_cost,
        t_cost: h.t_cost,
        p_cost: h.p_cost,
    };
    Ok(FileKeyInfo {
        salt: h.salt,
        kdf,
        kind: if kdf.below_passphrase_floor() {
            SecretKind::Key
        } else {
            SecretKind::Passphrase
        },
    })
}

/// The lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// --- The key store ----------------------------------------------------------

/// A secret as a key store keeps it, typed so an entry is never guessed.
pub enum StoredSecret {
    /// A key: `key:` and 64 hex digits.
    Key(Zeroizing<[u8; 32]>),
    /// A passphrase: `passphrase:` and the passphrase.
    Passphrase(Zeroizing<String>),
}

impl StoredSecret {
    /// The entry's text.
    pub fn encode(&self) -> Zeroizing<String> {
        match self {
            StoredSecret::Key(k) => Zeroizing::new(format!("key:{}", hex(&k[..]))),
            StoredSecret::Passphrase(p) => Zeroizing::new(format!("passphrase:{}", p.as_str())),
        }
    }

    /// Read an entry's text back.
    pub fn decode(text: &str) -> Result<StoredSecret> {
        if let Some(h) = text.strip_prefix("key:") {
            return parse_hex_key(h)
                .map(StoredSecret::Key)
                .ok_or_else(|| MnemoError::Invalid("a key entry that isn't 64 hex digits".into()));
        }
        if let Some(p) = text.strip_prefix("passphrase:") {
            return Ok(StoredSecret::Passphrase(Zeroizing::new(p.to_string())));
        }
        Err(MnemoError::Invalid(
            "a key-store entry of an unknown type".into(),
        ))
    }

    fn same_as(&self, other: &StoredSecret) -> bool {
        self.encode() == other.encode()
    }

    /// As a [`Secret`].
    pub fn into_secret(self) -> Secret {
        match self {
            StoredSecret::Key(k) => Secret::Key(k),
            StoredSecret::Passphrase(p) => Secret::Passphrase(p),
        }
    }

    fn from_secret(s: &Secret) -> StoredSecret {
        match s {
            Secret::Key(k) => StoredSecret::Key(k.clone()),
            Secret::Passphrase(p) => StoredSecret::Passphrase(p.clone()),
        }
    }
}

/// Where brains' secrets are kept, each found by its brain's salt: the OS
/// keychain (the `keychain` feature), or the test store.
pub trait KeyStore {
    /// How to name it to a person ("the test key store at …").
    fn describe(&self) -> String;
    /// The entry for `salt`, if there is one.
    fn get(&self, salt: &[u8; SALT_LEN]) -> Result<Option<StoredSecret>>;
    /// Store `secret` under `salt`, replacing any entry. `label` is for a
    /// person browsing the store (the brain's path); nothing looks it up.
    fn put(&self, salt: &[u8; SALT_LEN], secret: &StoredSecret, label: &str) -> Result<()>;
    /// Remove the entry for `salt`, if there is one.
    fn delete(&self, salt: &[u8; SALT_LEN]) -> Result<()>;
    /// Whether an entry is known to be on disk when [`KeyStore::put`]
    /// returns. None of the OS stores documents it, so the default is
    /// `false`, and the keys note's section 4 rule applies: a new key the
    /// display rule can't show isn't left in the store alone.
    fn durable(&self) -> bool {
        false
    }
    /// Whether the store can be used here at all: an error says why not
    /// (no Secret Service on the session bus, no access). It reads one
    /// entry that never exists, so it asks nothing of the user.
    fn check_available(&self) -> Result<()> {
        Ok(())
    }
}

/// **Tests only.** A key store in a directory, one 0600 file per salt.
/// [`default_store`] uses it when `MNEMO_DEV_KEYSTORE` names a directory,
/// which only a `dev-hooks` build reads. Never in a release build.
#[cfg(feature = "dev-hooks")]
#[doc(hidden)]
pub struct DirKeyStore {
    dir: PathBuf,
}

#[cfg(feature = "dev-hooks")]
impl DirKeyStore {
    /// A store in `dir`, which must exist.
    pub fn new(dir: impl Into<PathBuf>) -> DirKeyStore {
        DirKeyStore { dir: dir.into() }
    }

    fn entry(&self, salt: &[u8; SALT_LEN]) -> PathBuf {
        self.dir.join(hex(salt))
    }
}

#[cfg(feature = "dev-hooks")]
impl KeyStore for DirKeyStore {
    fn describe(&self) -> String {
        format!("the test key store at {}", self.dir.display())
    }

    fn get(&self, salt: &[u8; SALT_LEN]) -> Result<Option<StoredSecret>> {
        match std::fs::read_to_string(self.entry(salt)) {
            Ok(text) => StoredSecret::decode(Zeroizing::new(text).trim_end()).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(MnemoError::Io(std::io::Error::other(format!(
                "{} (reading): {e}",
                self.describe()
            )))),
        }
    }

    fn put(&self, salt: &[u8; SALT_LEN], secret: &StoredSecret, _label: &str) -> Result<()> {
        use std::io::Write;
        let path = self.entry(salt);
        let tmp = self.dir.join(format!("{}.tmp", hex(salt)));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(secret.encode().as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn delete(&self, salt: &[u8; SALT_LEN]) -> Result<()> {
        match std::fs::remove_file(self.entry(salt)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Each entry is written to a temp file, synced and renamed. A test can
    /// make it say otherwise with `MNEMO_DEV_KEYSTORE_NOT_DURABLE`, to stand
    /// in for an OS store.
    fn durable(&self) -> bool {
        std::env::var_os("MNEMO_DEV_KEYSTORE_NOT_DURABLE").is_none()
    }
}

/// The key store this build uses, if it has one: in a `dev-hooks` build,
/// the directory `MNEMO_DEV_KEYSTORE` names when it's set; otherwise the OS
/// keychain when the `keychain` feature is on (macOS, Windows, Linux).
/// `None` otherwise. Making one asks nothing of the store: whether it can be
/// used shows at the first lookup, or through
/// [`KeyStore::check_available`].
pub fn default_store() -> Option<Box<dyn KeyStore>> {
    #[cfg(feature = "dev-hooks")]
    {
        if let Some(dir) = std::env::var_os("MNEMO_DEV_KEYSTORE").filter(|d| !d.is_empty()) {
            return Some(Box::new(DirKeyStore::new(PathBuf::from(dir))));
        }
        // A test that wants no store at all, in a `keychain` build, so it
        // never reaches the real keychain of whoever runs it.
        if std::env::var_os("MNEMO_DEV_NO_KEYCHAIN").is_some() {
            return None;
        }
    }
    #[cfg(all(
        feature = "keychain",
        any(target_os = "macos", target_os = "windows", target_os = "linux")
    ))]
    return Some(Box::new(os_store::OsKeyStore));
    #[allow(unreachable_code)]
    None
}

/// The keychain's service name for every brain's entry; the account is the
/// brain's salt in lower-case hex (the keys note, section 4).
pub const KEYCHAIN_SERVICE: &str = "io.github.toarchkumar.mnemo.brain";

/// Store `secret` under `salt`, then read it back. A store that doesn't
/// give back what it was given has its entry removed, and the call fails.
pub fn store_and_check(
    store: &dyn KeyStore,
    salt: &[u8; SALT_LEN],
    secret: &StoredSecret,
    label: &str,
) -> Result<()> {
    store.put(salt, secret, label)?;
    match store.get(salt) {
        Ok(Some(back)) if back.same_as(secret) => Ok(()),
        other => {
            let _ = store.delete(salt);
            Err(MnemoError::Invalid(format!(
                "{} didn't give back the entry it was just given{}; nothing was changed",
                store.describe(),
                match other {
                    Err(e) => format!(" ({e})"),
                    _ => String::new(),
                }
            )))
        }
    }
}

// --- Keys as text, key files, the key command --------------------------------

fn parse_hex_key(h: &str) -> Option<Zeroizing<[u8; 32]>> {
    let h = h.trim();
    if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut k = Zeroizing::new([0u8; 32]);
    for (i, byte) in k.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(k)
}

/// A key written as text: 64 hex digits, or a recovery code. `what` names
/// where it came from, for the error.
pub fn parse_key_text(text: &str, what: &str) -> Result<Zeroizing<[u8; 32]>> {
    if let Some(k) = parse_hex_key(text) {
        return Ok(k);
    }
    if recovery::looks_like_code(text) {
        return recovery::parse(text).map_err(|e| MnemoError::Invalid(format!("{what}: {e}")));
    }
    Err(MnemoError::Invalid(format!(
        "{what} holds neither 64 hex digits nor a recovery code"
    )))
}

/// Read a key file: 32 raw bytes, 64 hex digits, or a recovery code. On
/// Unix it must be a regular file this user owns, with no access for group
/// or others, as `ssh` asks of a private key (the keys note, section 6).
pub fn read_key_file(path: impl AsRef<Path>) -> Result<Zeroizing<[u8; 32]>> {
    let path = path.as_ref();
    let meta = std::fs::metadata(path).map_err(|e| {
        MnemoError::Invalid(format!("reading the key file {}: {e}", path.display()))
    })?;
    if !meta.is_file() {
        return Err(MnemoError::Invalid(format!(
            "the key file {} isn't a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(MnemoError::Invalid(format!(
                "refusing the key file {}: another user owns it; give it to this user, then \
                 `chmod 600 {}`",
                path.display(),
                path.display()
            )));
        }
        let mode = meta.mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(MnemoError::Invalid(format!(
                "refusing the key file {}: group or others can read it (mode {mode:o}); run \
                 `chmod 600 {}`",
                path.display(),
                path.display()
            )));
        }
    }
    let bytes = Zeroizing::new(std::fs::read(path).map_err(|e| {
        MnemoError::Invalid(format!("reading the key file {}: {e}", path.display()))
    })?);
    if bytes.len() == 32 {
        let mut k = Zeroizing::new([0u8; 32]);
        k.copy_from_slice(&bytes);
        return Ok(k);
    }
    let text = Zeroizing::new(String::from_utf8_lossy(&bytes).into_owned());
    if let Some(k) = parse_hex_key(&text) {
        return Ok(k);
    }
    if recovery::looks_like_code(&text) {
        // A code's own error names the group that's wrong.
        return parse_key_text(&text, &format!("the key file {}", path.display()));
    }
    Err(MnemoError::Invalid(format!(
        "{}: a key file holds 32 raw bytes, 64 hex digits or a recovery code",
        path.display()
    )))
}

/// Write a new key file: created new (never over an existing name, a
/// symlink included) with mode 0600, holding the key's 64 hex digits.
pub fn write_new_key_file(path: impl AsRef<Path>, key: &[u8; 32]) -> Result<()> {
    use std::io::Write;
    let path = path.as_ref();
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path).map_err(|e| {
        MnemoError::Invalid(format!("creating the key file {}: {e}", path.display()))
    })?;
    let text = Zeroizing::new(format!("{}\n", hex(key)));
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    Ok(())
}

/// The most a key command may print.
const COMMAND_OUTPUT_CAP: usize = 4096;

/// Run `cmd` (split as `--embed-cmd` is, no shell) and read a key from the
/// first line of its output (the keys note, section 6): stdin closed,
/// stderr passed through, stdout capped at 4 KiB, its exit status checked,
/// and killed after `timeout`.
pub fn run_key_command(cmd: &str, timeout: Duration) -> Result<Zeroizing<[u8; 32]>> {
    use std::process::{Command, Stdio};
    let argv = crate::embed_cmd::shlex_split(cmd)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| MnemoError::Invalid("MNEMO_KEY_COMMAND is empty or badly quoted".into()))?;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| MnemoError::Invalid(format!("the key command didn't start: {e}")))?;
    let mut out = child.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Zeroizing::new(Vec::new());
        let r = (&mut out)
            .take(COMMAND_OUTPUT_CAP as u64 + 1)
            .read_to_end(&mut buf);
        let _ = tx.send((r, buf));
    });
    let start = std::time::Instant::now();
    let fail = |child: &mut std::process::Child, msg: String| {
        let _ = child.kill();
        let _ = child.wait();
        Err(MnemoError::Invalid(msg))
    };
    let buf = match rx.recv_timeout(timeout) {
        Ok((Ok(_), buf)) => buf,
        Ok((Err(e), _)) => {
            return fail(&mut child, format!("reading the key command's output: {e}"))
        }
        Err(_) => {
            return fail(
                &mut child,
                format!(
                    "the key command didn't answer within {} s",
                    timeout.as_secs()
                ),
            )
        }
    };
    if buf.len() > COMMAND_OUTPUT_CAP {
        return fail(
            &mut child,
            "the key command printed more than 4 KiB".to_string(),
        );
    }
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None if start.elapsed() >= timeout => {
                return fail(
                    &mut child,
                    format!(
                        "the key command didn't answer within {} s",
                        timeout.as_secs()
                    ),
                )
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    if !status.success() {
        return Err(MnemoError::Invalid(format!(
            "the key command failed ({status})"
        )));
    }
    let text = Zeroizing::new(String::from_utf8_lossy(&buf).into_owned());
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return Err(MnemoError::Invalid(
            "the key command printed nothing".into(),
        ));
    }
    parse_key_text(line, "the key command's output")
}

// --- The chain ---------------------------------------------------------------

/// The key command's timeout in the CLI.
pub const CLI_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// The key command's timeout in the MCP server, which has no terminal.
pub const SERVER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Where a secret came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// `MNEMO_PASSPHRASE`.
    EnvPassphrase,
    /// `MNEMO_KEY`.
    EnvKey,
    /// `MNEMO_KEY_FILE`.
    KeyFile(PathBuf),
    /// `MNEMO_KEY_COMMAND`.
    KeyCommand,
    /// A key-store entry; the text describes the store.
    Store(String),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::EnvPassphrase => f.write_str("MNEMO_PASSPHRASE"),
            Source::EnvKey => f.write_str("MNEMO_KEY"),
            Source::KeyFile(p) => write!(f, "MNEMO_KEY_FILE ({})", p.display()),
            Source::KeyCommand => f.write_str("MNEMO_KEY_COMMAND"),
            Source::Store(s) => write!(f, "this brain's entry in {s}"),
        }
    }
}

/// What [`resolve`] found.
pub struct Resolved {
    /// The secret.
    pub secret: Secret,
    /// Where it came from.
    pub source: Source,
    /// The next source of the same kind, for the error when this one
    /// doesn't open the file: the next environment source that's present,
    /// or the key store, which isn't looked up (it may have no entry).
    pub next: Option<Source>,
    /// Sources that were skipped, and why, to show the user.
    pub notes: Vec<String>,
}

/// The chain's answer.
pub enum Resolution {
    /// A source was present and gave a secret.
    Found(Resolved),
    /// No source was present. `kind` is the file's kind (the CLI then
    /// prompts for a passphrase file, at a terminal).
    NoSource {
        /// The file's kind.
        kind: SecretKind,
        /// Sources that were skipped, to show the user.
        notes: Vec<String>,
    },
}

fn env_set(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn kind_word(k: SecretKind) -> &'static str {
    match k {
        SecretKind::Key => "a key",
        SecretKind::Passphrase => "a passphrase",
    }
}

/// Find the secret for the brain at `path` (the keys note, section 6).
///
/// It reads the file's KDF parameters first, with no key: raw-key
/// parameters make it a key file, anything else a passphrase file. It takes
/// the first present source of that kind: `MNEMO_KEY`, `MNEMO_KEY_FILE`,
/// `MNEMO_KEY_COMMAND`, the key store, for a key; `MNEMO_PASSPHRASE`, a
/// passphrase entry in the key store, for a passphrase. A present source of
/// the other kind is named in [`Resolved::notes`] as skipped. With no source
/// of the file's kind, the first present source of the other kind is used,
/// once (for a key file, only a passphrase source). Nothing is tried: the
/// caller opens with what comes back.
///
/// **The key store is looked up last, and only when it's needed** (step
/// 2): when no earlier source of the file's kind is present, or for the one
/// try of the other kind. So a keychain that would ask the user something,
/// or can't be reached at all, is never touched when `MNEMO_KEY` answers. A
/// store error is a note when another source answers, and the error only
/// when the store was the one left.
///
/// **When page 0 doesn't parse,** the salts and the kind come from the
/// header frames in the file's write-ahead log (newest first), and the
/// store is looked up under each in turn; the open that follows heals the
/// header. With neither, the kind is unknown: the first environment source
/// present is used.
pub fn resolve(
    path: impl AsRef<Path>,
    store: Option<&dyn KeyStore>,
    command_timeout: Duration,
) -> Result<Resolution> {
    let path = path.as_ref();
    let file_name = path.display().to_string();
    let mut notes = Vec::new();
    let (kind, salts) = match file_info(path) {
        Ok(i) => (Some(i.kind), vec![i.salt]),
        Err(_) => match crate::store::wal_header_salts(path) {
            Ok(c) if !c.is_empty() => {
                notes.push(format!(
                    "{file_name}'s header couldn't be read; its write-ahead log names {} salt(s) \
                     to look up",
                    c.len()
                ));
                (
                    Some(kind_of_kdf(c[0].1)),
                    c.iter().map(|(s, _)| *s).collect(),
                )
            }
            _ => (None, Vec::new()),
        },
    };
    let env = env_sources();
    let Some(kind) = kind else {
        // The kind is unknown, and there's no salt to look up.
        let Some((src, _)) = env.first().cloned() else {
            return Ok(Resolution::NoSource {
                kind: SecretKind::Passphrase,
                notes,
            });
        };
        notes.push(format!(
            "{file_name}'s header couldn't be read, so its kind of secret is unknown"
        ));
        let secret = fetch(&src, None, command_timeout)?;
        return Ok(Resolution::Found(Resolved {
            secret,
            source: src,
            next: None,
            notes,
        }));
    };
    let mine: Vec<&(Source, SecretKind)> = env.iter().filter(|(_, k)| *k == kind).collect();
    let other: Vec<&(Source, SecretKind)> = env.iter().filter(|(_, k)| *k != kind).collect();
    let skipped = |notes: &mut Vec<String>, using: &Source| {
        for (s, k) in &other {
            notes.push(format!(
                "{s} holds {}, but {file_name} uses {}; skipped it, using {using}",
                kind_word(*k),
                kind_word(kind)
            ));
        }
    };
    // An environment source of the file's kind: the store isn't touched.
    if let Some(first) = mine.first() {
        skipped(&mut notes, &first.0);
        let secret = fetch(&first.0, None, command_timeout)?;
        return Ok(Resolution::Found(Resolved {
            secret,
            source: first.0.clone(),
            // The next source of this kind: the next environment one, else
            // the store, which isn't looked up now (it may have no entry).
            next: mine
                .get(1)
                .map(|(s, _)| s.clone())
                .or_else(|| store.map(|st| Source::Store(st.describe()))),
            notes,
        }));
    }
    // The store, last.
    let looked = match store {
        None => Ok(None),
        // Section 8: a store that can't be used here at all (no Secret
        // Service on the session bus, no access) is no store, and the chain
        // goes on without it. Any other error is the store's own.
        Some(st) => match lookup_salts(st, &salts) {
            Ok(e) => Ok(e.map(|e| (st.describe(), e))),
            Err(_) if st.check_available().is_err() => Ok(None),
            Err(e) => Err(e),
        },
    };
    let (store_other, store_error) = match looked {
        Ok(Some((desc, e))) if stored_kind(&e) == kind => {
            let src = Source::Store(desc);
            skipped(&mut notes, &src);
            return Ok(Resolution::Found(Resolved {
                secret: e.into_secret(),
                source: src,
                next: None,
                notes,
            }));
        }
        Ok(found) => (found, None),
        Err(e) => (None, Some(e)),
    };
    // No source of the file's kind: the first of the other kind, once.
    if let Some(first) = other.first() {
        if let (Some(e), Some(st)) = (&store_error, store) {
            notes.push(format!("{} couldn't be read ({e})", st.describe()));
        }
        notes.push(format!(
            "{file_name} uses {}, and no source of that kind is set; trying {} once",
            kind_word(kind),
            first.0
        ));
        let secret = fetch(&first.0, None, command_timeout)?;
        return Ok(Resolution::Found(Resolved {
            secret,
            source: first.0.clone(),
            next: None,
            notes,
        }));
    }
    if let Some(e) = store_error {
        return Err(e);
    }
    if let Some((desc, e)) = store_other {
        let src = Source::Store(desc);
        notes.push(format!(
            "{file_name} uses {}, and no source of that kind is set; trying {src} once",
            kind_word(kind)
        ));
        return Ok(Resolution::Found(Resolved {
            secret: e.into_secret(),
            source: src,
            next: None,
            notes,
        }));
    }
    Ok(Resolution::NoSource { kind, notes })
}

/// The environment's sources that are present, in the table's order.
fn env_sources() -> Vec<(Source, SecretKind)> {
    let mut present = Vec::new();
    if env_set("MNEMO_PASSPHRASE").is_some() {
        present.push((Source::EnvPassphrase, SecretKind::Passphrase));
    }
    if env_set("MNEMO_KEY").is_some() {
        present.push((Source::EnvKey, SecretKind::Key));
    }
    if let Some(f) = env_set("MNEMO_KEY_FILE") {
        present.push((Source::KeyFile(PathBuf::from(f)), SecretKind::Key));
    }
    if env_set("MNEMO_KEY_COMMAND").is_some() {
        present.push((Source::KeyCommand, SecretKind::Key));
    }
    present
}

/// The first entry `store` has under any of `salts`, in order.
fn lookup_salts(store: &dyn KeyStore, salts: &[[u8; SALT_LEN]]) -> Result<Option<StoredSecret>> {
    for salt in salts {
        if let Some(e) = store.get(salt)? {
            return Ok(Some(e));
        }
    }
    Ok(None)
}

fn stored_kind(e: &StoredSecret) -> SecretKind {
    match e {
        StoredSecret::Key(_) => SecretKind::Key,
        StoredSecret::Passphrase(_) => SecretKind::Passphrase,
    }
}

fn kind_of_kdf(kdf: KdfParams) -> SecretKind {
    if kdf.below_passphrase_floor() {
        SecretKind::Key
    } else {
        SecretKind::Passphrase
    }
}

/// Read the secret out of a present source.
fn fetch(src: &Source, entry: Option<StoredSecret>, timeout: Duration) -> Result<Secret> {
    Ok(match src {
        Source::EnvPassphrase => Secret::Passphrase(Zeroizing::new(
            env_set("MNEMO_PASSPHRASE").unwrap_or_default(),
        )),
        Source::EnvKey => {
            let text = Zeroizing::new(env_set("MNEMO_KEY").unwrap_or_default());
            Secret::Key(parse_key_text(&text, "MNEMO_KEY")?)
        }
        Source::KeyFile(p) => Secret::Key(read_key_file(p)?),
        Source::KeyCommand => {
            let cmd = env_set("MNEMO_KEY_COMMAND").unwrap_or_default();
            Secret::Key(run_key_command(&cmd, timeout)?)
        }
        Source::Store(_) => entry
            .ok_or_else(|| MnemoError::Invalid("the key-store entry went away".into()))?
            .into_secret(),
    })
}

// --- The orders ------------------------------------------------------------

/// [`create_generated`]'s `persist`: keep the key, given the brain's salt.
pub type Persist<'a> = dyn FnMut(&[u8; SALT_LEN], &[u8; 32]) -> Result<()> + 'a;

/// Create a brain at `path` under a freshly generated key (the keys note,
/// section 1). In this order, so there's never a brain at `path` whose key
/// is only in memory:
/// 1. the key is drawn, and the brain is created under a temp name next to
///    `path` with the raw-key parameters;
/// 2. `prepare` runs on it (the CLI writes its manifest), and it's closed;
/// 3. `persist` keeps the key, given the brain's salt (a key store's
///    [`store_and_check`], or a new key file);
/// 4. the temp file takes `path`'s name.
///
/// `path` must not exist. Dead `init` temps next to it are removed first.
/// On an error the temp file is removed.
pub fn create_generated(
    path: impl AsRef<Path>,
    config: MnemoConfig,
    prepare: &mut dyn FnMut(&mut Mnemo) -> Result<()>,
    persist: &mut Persist<'_>,
) -> Result<Zeroizing<[u8; 32]>> {
    let path = path.as_ref();
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(MnemoError::Invalid(format!(
            "{} already exists; nothing was changed",
            path.display()
        )));
    }
    crate::store::remove_dead_temps(path, "init");
    let key = Zeroizing::new(Mnemo::generate_key());
    let config = MnemoConfig {
        kdf: KdfParams::raw_key(),
        ..config
    };
    let (tmp, mut db) =
        crate::store::create_temp(path, "init", &key[..], SecretKind::Key, config, None)?;
    let result = (|| {
        prepare(&mut db)?;
        let salt = db.header_salt();
        db.close()?;
        persist(&salt, &key)?;
        #[cfg(feature = "dev-hooks")]
        if std::env::var_os("MNEMO_DEV_ABORT_INIT_BEFORE_RENAME").is_some() {
            std::process::abort();
        }
        if std::fs::symlink_metadata(path).is_ok() {
            return Err(MnemoError::Invalid(format!(
                "{} appeared while it was being created; nothing was changed there",
                path.display()
            )));
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(key),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// A rekey through [`rekey_stored`] failed. `entries` says what became of
/// the key-store entries.
#[derive(Debug)]
pub struct StoredRekeyError {
    /// The rekey's error.
    pub error: MnemoError,
    /// What became of the entries.
    pub entries: String,
    /// Whether the new secret is certainly not in force (the old salt is
    /// still on disk), so anything written for it can go.
    pub new_secret_dropped: bool,
}

impl std::fmt::Display for StoredRekeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; {}", self.error, self.entries)
    }
}

impl std::error::Error for StoredRekeyError {}

/// Rekey `db` to `new`, keeping it in `store` under the new salt (the keys
/// note, section 4). The order:
/// 1. the new salt is drawn;
/// 2. `new` is stored under it and read back ([`store_and_check`]);
/// 3. `before_commit` runs (the CLI shows a new key's recovery code here);
/// 4. the rekey commits;
/// 5. after an error, page 0 is read again: the new entry is deleted only
///    if the old salt is still there, and both are kept otherwise.
///
/// The old entry is kept: copies of the file made before still use it.
/// A key gets [`KdfParams::raw_key`]; a passphrase gets
/// [`KdfParams::for_passphrase`] of the file's parameters. Returns a note
/// for the user.
pub fn rekey_stored(
    db: &mut Mnemo,
    new: &Secret,
    store: &dyn KeyStore,
    label: &str,
    before_commit: &mut dyn FnMut() -> Result<()>,
) -> std::result::Result<String, StoredRekeyError> {
    let old_salt = db.header_salt();
    let kdf = match new.kind() {
        SecretKind::Key => KdfParams::raw_key(),
        SecretKind::Passphrase => db.kdf_params().for_passphrase(),
    };
    let stored = StoredSecret::from_secret(new);
    let mut new_salt: Option<[u8; SALT_LEN]> = None;
    let r = db.rekey_staged(new.bytes(), new.kind(), kdf, &mut |salt| {
        store_and_check(store, salt, &stored, label)?;
        new_salt = Some(*salt);
        before_commit()
    });
    match r {
        Ok(()) => Ok(format!(
            "the new secret is in {}; the old entry is still there, for copies of the file \
             made before this rekey",
            store.describe()
        )),
        Err(error) => {
            let Some(ns) = new_salt else {
                return Err(StoredRekeyError {
                    error,
                    entries: "the key store wasn't changed".into(),
                    new_secret_dropped: true,
                });
            };
            // REKEY-1's one exception: page 0 may or may not have landed,
            // whatever it reads now, so both entries stay.
            if Mnemo::rekey_left_either_secret(&error) {
                return Err(StoredRekeyError {
                    error,
                    entries: "both entries are kept, since the file may open with either \
                              secret"
                        .into(),
                    new_secret_dropped: false,
                });
            }
            let (entries, new_secret_dropped) = match db.disk_salt() {
                Ok(s) if s == old_salt => match store.delete(&ns) {
                    Ok(()) => (
                        "the old secret is still in force, and the new entry was removed"
                            .to_string(),
                        true,
                    ),
                    Err(e) => (
                        format!(
                            "the old secret is still in force; removing the new entry failed ({e})"
                        ),
                        true,
                    ),
                },
                Ok(s) if s == ns => (
                    "the file may now open only with the new secret, so both entries are kept"
                        .to_string(),
                    false,
                ),
                _ => (
                    "the file's header couldn't be read back, so both entries are kept".to_string(),
                    false,
                ),
            };
            Err(StoredRekeyError {
                error,
                entries,
                new_secret_dropped,
            })
        }
    }
}

/// The text `mnemo info` shows for a file's KDF parameters, when they need
/// a word: raw-key parameters with a passphrase, or test-grade ones.
pub fn kdf_grade_note(kdf: KdfParams, kind: SecretKind) -> Option<String> {
    if kind == SecretKind::Passphrase && kdf.below_passphrase_floor() {
        return Some(
            "below the passphrase floor: a passphrase on raw-key KDF parameters; `mnemo rekey` \
             or `mnemo compact` raises them"
                .into(),
        );
    }
    if kdf.is_test_grade() {
        return Some("test-grade KDF parameters".into());
    }
    None
}
