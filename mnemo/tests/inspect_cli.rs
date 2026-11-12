//! `mnemo inspect` (v0.13, INSP-1 step 5): the read-only inspector.
//!
//! These run the real binary and talk to it over raw HTTP: it binds
//! loopback only; a wrong token, a wrong `Host` and any method but `GET`
//! are refused; the file is byte-identical after a session (reload
//! included) and nothing else is written; a file a writer holds gives the
//! clear message; each view's JSON is right. The acceptance test reads a
//! passphrase export made through the iOS sample's code path
//! (`tests/fixtures/inspect/ios_export.mnemo`, written by the uniffi test
//! `write_the_inspect_fixture`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use mnemo_engine::{
    Embedder, EmbedderIdentity, EssenceScope, EssenceSection, KdfParams, Memory, MemoryType, Mnemo,
    MnemoConfig, Scope, ThreadTurn,
};
use serde_json::Value;

const PW: &str = "inspect-pw";

/// A running `mnemo inspect`, killed on drop.
struct Inspector {
    child: Child,
    port: u16,
    token: String,
}

impl Drop for Inspector {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(path: &Path, pp: &str, env: &[(&str, &Path)]) -> Inspector {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mnemo"));
    cmd.args(["inspect", path.to_str().unwrap()])
        .env("MNEMO_PASSPHRASE", pp)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("run mnemo inspect");
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let url = loop {
        match lines.next() {
            Some(Ok(l)) if l.starts_with("http://") => break l,
            Some(Ok(_)) => continue,
            _ => {
                let mut err = String::new();
                child
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut err)
                    .unwrap();
                panic!("inspect exited before printing its URL: {err}");
            }
        }
    };
    // Keep draining stdout so the child never blocks on a full pipe.
    std::thread::spawn(move || for _ in lines {});
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .expect("a loopback URL");
    let (port, token) = rest.split_once("/?t=").unwrap();
    Inspector {
        child,
        port: port.parse().unwrap(),
        token: token.to_string(),
    }
}

/// One raw request; returns (status, body).
fn request(port: u16, method: &str, target: &str, host: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "{method} {target} HTTP/1.1\r\nHost: {host}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    let body = out
        .split_once("\r\n\r\n")
        .map(|x| x.1)
        .unwrap_or("")
        .to_string();
    (status, body)
}

impl Inspector {
    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
    fn get(&self, path: &str, query: &str) -> (u16, String) {
        let sep = if query.is_empty() { "" } else { "&" };
        request(
            self.port,
            "GET",
            &format!("{path}?t={}{sep}{query}", self.token),
            &self.host(),
        )
    }
    fn json(&self, path: &str, query: &str) -> Value {
        let (status, body) = self.get(path, query);
        assert_eq!(status, 200, "{path}?{query}: {body}");
        serde_json::from_str(&body).unwrap()
    }
}

struct Words;
impl Embedder for Words {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "inspect-test-words".into(),
            dimensions: 4,
            normalize: false,
        }
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| vec![t.len() as f32 + 1.0, 1.0, 0.5, 0.25])
            .collect())
    }
}

/// Memories over two agents, a keyed fact with three versions, a thread
/// with turns and essence, project essence, and a few flushes (snapshots).
fn fixture(dir: &Path) -> PathBuf {
    let path = dir.join("i.mnemo");
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&path, PW, cfg).unwrap();
    db.set_embedder(Words).unwrap();
    let v = |i: f32| vec![i + 1.0, 0.5, 0.25, 1.0];
    db.remember(
        Memory::new("Deploys go out on Tuesdays", MemoryType::Procedural, v(0.0)).with_agent("ops"),
    )
    .unwrap();
    db.remember(
        Memory::new("The user likes oat milk", MemoryType::Semantic, v(1.0))
            .with_agent("assistant")
            .with_scope(Scope::Shared),
    )
    .unwrap();
    db.flush().unwrap();
    for (i, region) in ["eu-west-1", "eu-central-1", "us-east-2"]
        .iter()
        .enumerate()
    {
        db.remember_keyed(
            Memory::new(
                format!("region: {region}"),
                MemoryType::Semantic,
                v(2.0 + i as f32),
            )
            .with_agent("ops"),
            "deploy:region",
        )
        .unwrap();
        db.flush().unwrap();
    }
    let turns: Vec<ThreadTurn> = [
        ("user", "Plan the offsite in Lisbon"),
        ("assistant", "Booked for May"),
    ]
    .iter()
    .enumerate()
    .map(|(i, (role, text))| ThreadTurn {
        turn_id: format!("t{i}"),
        role: role.to_string(),
        content: text.to_string(),
        timestamp: Some(1_794_389_400 + i as i64),
    })
    .collect();
    db.ingest_thread("offsite", &turns).unwrap();
    db.essence_add(
        EssenceScope::Thread("offsite".into()),
        EssenceSection::Decisions,
        "The offsite is in Lisbon in May",
        None,
    )
    .unwrap();
    db.essence_add(
        EssenceScope::Project,
        EssenceSection::Conventions,
        "Every change goes through a PR",
        None,
    )
    .unwrap();
    db.close().unwrap();
    path
}

#[test]
fn it_binds_loopback_and_refuses_wrong_tokens_hosts_and_methods() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let ins = spawn(&path, PW, &[]);
    let host = ins.host();
    // The page and the API both need the token.
    assert_eq!(ins.get("/", "").0, 200);
    assert_eq!(request(ins.port, "GET", "/", &host).0, 403, "no token");
    assert_eq!(request(ins.port, "GET", "/api/about?t=nope", &host).0, 403);
    let last = if ins.token.ends_with('0') { '1' } else { '0' };
    let wrong = format!("/api/about?t={}{last}", &ins.token[..63]);
    assert_eq!(
        request(ins.port, "GET", &wrong, &host).0,
        403,
        "one digit off"
    );
    // DNS rebinding: the right token but another site's Host.
    let good = format!("/api/about?t={}", ins.token);
    assert_eq!(request(ins.port, "GET", &good, "evil.example:80").0, 403);
    assert_eq!(
        request(
            ins.port,
            "GET",
            &good,
            &format!("evil.example:{}", ins.port)
        )
        .0,
        403
    );
    assert_eq!(
        request(
            ins.port,
            "GET",
            &good,
            &format!("127.0.0.1:{}", ins.port + 1)
        )
        .0,
        403
    );
    assert_eq!(
        request(ins.port, "GET", &good, &format!("localhost:{}", ins.port)).0,
        200
    );
    // Only GET.
    for m in ["POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"] {
        assert_eq!(request(ins.port, m, &good, &host).0, 405, "{m}");
    }
    // No CORS: the response never grants another origin.
    let mut s = TcpStream::connect(("127.0.0.1", ins.port)).unwrap();
    write!(
        s,
        "GET {good} HTTP/1.1\r\nHost: {host}\r\nOrigin: https://evil.example\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    assert!(
        !raw.to_ascii_lowercase().contains("access-control-allow"),
        "{raw}"
    );
    assert!(raw.contains("Cache-Control: no-store"));
    // Not reachable on another interface, when this machine has one.
    if let Some(ip) = non_loopback_ip() {
        assert!(
            TcpStream::connect((ip, ins.port)).is_err(),
            "reachable on {ip}"
        );
    }
}

/// This machine's address on its default route, if it has one (no packet
/// is sent).
fn non_loopback_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[test]
fn each_view_serves_the_right_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let ins = spawn(&path, PW, &[]);

    let about = ins.json("/api/about", "");
    assert_eq!(about["format_version"], 9);
    assert_eq!(about["embedder"]["name"], "inspect-test-words");
    assert_eq!(about["dimensions"], 4);
    assert_eq!(about["threads"], 1);
    assert_eq!(about["keys"], 1);
    assert!(about["recall_mode"].is_string() && about["loaded_at"].is_i64());

    // Memories: turns are left out (they're under Threads).
    let all = ins.json("/api/memories", "");
    let contents: Vec<&str> = all["memories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert!(contents.contains(&"Deploys go out on Tuesdays"));
    assert!(
        !contents.iter().any(|c| c.contains("Booked for May")),
        "{contents:?}"
    );
    assert_eq!(
        all["agents"].as_array().unwrap().len(),
        3,
        "{}",
        all["agents"]
    );
    let ops = ins.json("/api/memories", "agent=ops");
    assert!(ops["memories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["agent"] == "ops"));
    assert_eq!(ops["total"], 4, "one procedure and three keyed versions");
    let keyed = ins.json("/api/memories", "keyed=yes");
    assert_eq!(keyed["total"], 3);
    let unkeyed = ins.json("/api/memories", "keyed=no&type=procedural");
    assert_eq!(unkeyed["total"], 1);
    let in_thread = ins.json("/api/memories", "thread=offsite");
    assert!(in_thread["memories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["thread"] == "offsite" && m["kind"] == "essence"));
    // Keyword search (BM25), best first, with a score.
    let hits = ins.json("/api/memories", "q=oat+milk");
    assert_eq!(hits["memories"][0]["content"], "The user likes oat milk");
    assert!(hits["memories"][0]["score"].as_f64().unwrap() > 0.0);
    let ident = ins.json("/api/memories", "q=eu-central-1");
    assert_eq!(ident["memories"][0]["content"], "region: eu-central-1");
    assert_eq!(ins.json("/api/memories", "q=zebra")["total"], 0);

    // Keyed history: oldest to newest, with when each stopped being current.
    let keys = ins.json("/api/keys", "");
    assert_eq!(keys[0]["key"], "deploy:region");
    assert_eq!(keys[0]["owner"], "ops");
    assert_eq!(keys[0]["versions"], 3);
    assert_eq!(keys[0]["current"], "region: us-east-2");
    let h = ins.json("/api/history", "key=deploy%3Aregion&owner=ops");
    let vs = h["versions"].as_array().unwrap();
    let chain: Vec<&str> = vs.iter().map(|v| v["content"].as_str().unwrap()).collect();
    assert_eq!(
        chain,
        [
            "region: eu-west-1",
            "region: eu-central-1",
            "region: us-east-2"
        ]
    );
    assert!(vs[0]["invalid_at"].is_i64() && vs[1]["invalid_at"].is_i64());
    assert!(vs[2]["invalid_at"].is_null() && vs[2]["current"] == true);
    assert_eq!(ins.get("/api/history", "key=missing").0, 404);

    // Threads: the essence render, and the turns only on demand.
    let threads = ins.json("/api/threads", "");
    assert_eq!(threads["threads"][0]["id"], "offsite");
    assert_eq!(threads["threads"][0]["turns"], 2);
    assert!(threads["project_essence"]
        .as_str()
        .unwrap()
        .contains("Every change goes through a PR"));
    let t = ins.json("/api/thread", "id=offsite");
    assert!(t["essence"].as_str().unwrap().contains("Lisbon in May"));
    assert_eq!(t["turn_count"], 2);
    assert!(t.get("turns").is_none());
    let t = ins.json("/api/thread", "id=offsite&turns=1");
    assert_eq!(t["turns"][1]["content"], "Booked for May");
    assert_eq!(t["turns"][0]["role"], "user");

    // Snapshots: the step 4 fields and the list.
    let s = ins.json("/api/snapshots", "");
    let list = s["list"].as_array().unwrap();
    assert!(list.len() >= 4, "{s}");
    assert_eq!(s["retained"], list.len());
    assert_eq!(s["oldest_restorable_at"], list[0]["created_at"]);
    assert!(s["pinned_bytes"].is_u64());

    assert_eq!(ins.get("/api/nothing", "").0, 404);
    let page = ins.get("/", "").1;
    assert!(page.contains("<title>mnemo inspect</title>"));
}

#[test]
fn the_file_is_unchanged_and_nothing_else_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let path = fixture(&data);
    let before = std::fs::read(&path).unwrap();
    let ins = spawn(
        &path,
        PW,
        &[
            ("HOME", &home),
            ("TMPDIR", &home),
            ("TMP", &home),
            ("TEMP", &home),
        ],
    );
    for (p, q) in [
        ("/", ""),
        ("/api/about", ""),
        ("/api/memories", "q=oat"),
        ("/api/keys", ""),
        ("/api/history", "key=deploy%3Aregion&owner=ops"),
        ("/api/thread", "id=offsite&turns=1"),
        ("/api/snapshots", ""),
        ("/api/reload", ""),
    ] {
        assert_eq!(ins.get(p, q).0, 200, "{p}");
    }
    drop(ins);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the session changed the file"
    );
    let names = |d: &Path| -> Vec<String> {
        std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(
        names(&data),
        ["i.mnemo"],
        "a file appeared beside the .mnemo"
    );
    assert!(
        names(&home).is_empty(),
        "files appeared in HOME/TMPDIR: {:?}",
        names(&home)
    );
}

#[test]
fn reload_reads_the_file_again_and_a_writer_is_never_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let ins = spawn(&path, PW, &[]);
    assert_eq!(ins.json("/api/memories", "q=Lisbon")["total"], 1);
    // The inspector holds no lock: a writer opens the file while it runs.
    let mut db = Mnemo::open(&path, PW).unwrap();
    db.remember(Memory::new(
        "A note about Lisbon trams",
        MemoryType::Semantic,
        vec![2.0, 1.0, 0.0, 1.0],
    ))
    .unwrap();
    db.flush().unwrap();
    // While the writer holds it, a reload says so and the old data stays.
    let (status, body) = ins.get("/api/reload", "");
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("a writer has this file open"), "{body}");
    assert_eq!(ins.json("/api/memories", "q=Lisbon")["total"], 1);
    db.close().unwrap();
    // Afterwards it reads the new memory.
    let about = ins.json("/api/reload", "");
    assert!(about["loaded_at"].is_i64());
    assert_eq!(ins.json("/api/memories", "q=Lisbon")["total"], 2);
}

#[test]
fn a_file_a_writer_holds_gives_the_clear_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let _writer = Mnemo::open(&path, PW).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(["inspect", path.to_str().unwrap()])
        .env("MNEMO_PASSPHRASE", PW)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("a writer has this file open; close it or try again"),
        "{err}"
    );
}

#[test]
fn a_raw_key_file_unlocks_and_a_v8_file_opens() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k.mnemo");
    std::fs::copy(fixtures.join("v8_key.mnemo"), &path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(["inspect", path.to_str().unwrap(), "--key-file"])
        .arg(fixtures.join("v8_key.key"))
        .env_remove("MNEMO_PASSPHRASE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    out.read_line(&mut first).unwrap();
    let mut url = String::new();
    out.read_line(&mut url).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert!(url.starts_with("http://127.0.0.1:"), "{first}{url}");

    // The passphrase v8 fixture, through the usual path.
    std::fs::copy(
        fixtures.join("v8_passphrase.mnemo"),
        dir.path().join("p.mnemo"),
    )
    .unwrap();
    let ins = spawn(&dir.path().join("p.mnemo"), "v8-fixture-passphrase", &[]);
    let about = ins.json("/api/about", "");
    assert_eq!(about["format_version"], 8);
    // v0.12.3 recorded no identity (EMBED-ID-1 is v0.13); the manifest names it.
    assert!(about["embedder"].is_null());
    assert_eq!(about["manifest_embedder"]["name"], "fixture-embedder-r1");
    assert!(ins.json("/api/thread", "id=offsite")["essence"]
        .as_str()
        .unwrap()
        .contains("Lisbon"));
    assert_eq!(ins.json("/api/snapshots", "")["retained"], 7);
}

/// RB-001.14's acceptance test: a passphrase export made through the iOS
/// sample's code path shows its memories, threads and essence.
#[test]
fn it_shows_an_ios_passphrase_export() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inspect/ios_export.mnemo");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mnemo-notes-export.mnemo");
    std::fs::copy(src, &path).unwrap();
    let ins = spawn(&path, "inspect-fixture-passphrase", &[]);

    let about = ins.json("/api/about", "");
    assert_eq!(about["embedder"]["name"], "demo-word-hash");
    assert_eq!(about["dimensions"], 64);
    let m = ins.json("/api/memories", "type=episodic");
    let notes: Vec<&str> = m["memories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["content"].as_str().unwrap())
        .collect();
    assert!(
        notes.contains(&"Buy oat milk and coffee beans on the way home"),
        "{notes:?}"
    );
    assert_eq!(notes.len(), 3);
    let h = ins.json("/api/history", "key=fact%3Amilk&owner=default");
    let chain: Vec<&str> = h["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["content"].as_str().unwrap())
        .collect();
    assert_eq!(chain, ["milk: oat", "milk: almond"]);
    let t = ins.json("/api/thread", "id=demo-thread&turns=1");
    assert_eq!(t["turn_count"], 3);
    assert!(t["essence"].as_str().unwrap().contains("Sage green walls"));
    assert!(t["essence"]
        .as_str()
        .unwrap()
        .contains("Paint delivery booked"));
    assert_eq!(
        t["turns"][2]["content"],
        "Book the paint delivery for Friday"
    );
    // The export has no project-scope essence, so there's none to show.
    assert!(ins.json("/api/threads", "")["project_essence"].is_null());
}
