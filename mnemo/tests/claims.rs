//! 13a step 3 (v0.17): claims and presence (13.3, 12.4's step 7): the
//! lease, tokens and `Lost`, the early lapse as a hint, presence locks and
//! `who()`, no lock on the brain between calls, and 12.4's race: two
//! processes claim the same topics in lockstep, and exactly one wins each,
//! while another thread in each spawns children (FLAKE-1's pattern).
//!
//! **12.4's "1,000-race test" runs weekly and on demand, not on every
//! push:** each round is two durable commits, and macOS's full fsync makes
//! 1,000 rounds take about 80 s. So a normal run races 200 rounds, and
//! `MNEMO_STRESS=1` races 1,000 (`fuzz-weekly.yml`'s `claim-race` job, on
//! Linux and macOS). `CL_RACE_ROUNDS` sets any other count.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mnemo_engine::{
    Alive, ClaimOutcome, ClaimStatus, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig,
    MnemoError, SharedMnemo, SharedOptions,
};

const PW: &str = "claims-test-passphrase";

fn brain(dir: &Path) -> PathBuf {
    let p = dir.join("c.mnemo");
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&p, PW, cfg).unwrap();
    db.remember(Memory::new(
        "a fact",
        MemoryType::Semantic,
        vec![1.0, 0.0, 0.0, 0.0],
    ))
    .unwrap();
    db.close().unwrap();
    p
}

fn opts() -> SharedOptions {
    let mut o = SharedOptions::default();
    o.deadline = Duration::from_secs(10);
    o
}

fn shared(p: &Path, agent: &str) -> SharedMnemo {
    let mut s = Mnemo::open_shared(p, PW, opts()).unwrap();
    s.set_author(Some(agent.into()));
    s
}

fn token(o: ClaimOutcome) -> String {
    match o {
        ClaimOutcome::Claimed { token, .. } => token,
        other => panic!("not claimed: {other:?}"),
    }
}

#[test]
fn lease_renew_release_and_lost_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    let mut db = Mnemo::open(&p, PW).unwrap();
    // No agent: refused. Bad topics and leases too.
    assert!(matches!(db.claim("t", None), Err(MnemoError::Invalid(_))));
    db.set_author(Some("worker-a".into()));
    for (topic, ttl) in [
        ("", None),
        ("two\nlines", None),
        ("t", Some(29)),
        ("t", Some(86_401)),
    ] {
        assert!(
            matches!(db.claim(topic, ttl), Err(MnemoError::Invalid(_))),
            "{topic:?} {ttl:?}"
        );
    }
    let a = token(db.claim("PLAN.md tail", None).unwrap());
    assert_eq!(a.len(), 32);
    // Another agent sees it held; so does the same agent (no token reuse).
    db.set_author(Some("worker-b".into()));
    assert!(matches!(
        db.claim("PLAN.md tail", Some(60)).unwrap(),
        ClaimOutcome::HeldBy { ref agent, .. } if agent == "worker-a"
    ));
    db.set_author(Some("worker-a".into()));
    assert!(matches!(
        db.claim("PLAN.md tail", None).unwrap(),
        ClaimOutcome::HeldBy { .. }
    ));
    // Check, renew, a wrong token.
    let Ok(ClaimStatus::Held { expires_at: e0 }) = db.check_claim("PLAN.md tail", &a) else {
        panic!()
    };
    assert_eq!(
        db.check_claim("PLAN.md tail", "nope").unwrap(),
        ClaimStatus::Lost
    );
    assert_eq!(
        db.renew("PLAN.md tail", "nope", None).unwrap(),
        ClaimStatus::Lost
    );
    assert_eq!(
        db.release("PLAN.md tail", "nope").unwrap(),
        ClaimStatus::Lost
    );
    db.__set_clock_offset_for_testing(600);
    let Ok(ClaimStatus::Held { expires_at: e1 }) = db.renew("PLAN.md tail", &a, None) else {
        panic!()
    };
    // Renewed for its lease from the shifted now (a wall-clock second may
    // tick between the two calls).
    assert!((600..=602).contains(&(e1 - e0)), "{}", e1 - e0);
    // It lapses at expires_at: another agent takes it, and the old token is
    // lost everywhere.
    db.__set_clock_offset_for_testing(600 + 15 * 60);
    assert_eq!(
        db.check_claim("PLAN.md tail", &a).unwrap(),
        ClaimStatus::Lost
    );
    db.set_author(Some("worker-b".into()));
    let b = token(db.claim("PLAN.md tail", Some(60)).unwrap());
    assert_ne!(a, b);
    for st in [
        db.check_claim("PLAN.md tail", &a).unwrap(),
        db.renew("PLAN.md tail", &a, None).unwrap(),
        db.release("PLAN.md tail", &a).unwrap(),
    ] {
        assert_eq!(st, ClaimStatus::Lost);
    }
    // Release frees it at once; a released token is lost.
    assert_eq!(
        db.release("PLAN.md tail", &b).unwrap(),
        ClaimStatus::Released
    );
    assert_eq!(
        db.check_claim("PLAN.md tail", &b).unwrap(),
        ClaimStatus::Lost
    );
    db.set_author(Some("worker-a".into()));
    let c = token(db.claim("PLAN.md tail", None).unwrap());
    // One record per topic, changed in place, and never a recall hit.
    let claims: Vec<Memory> = db
        .memories()
        .unwrap()
        .into_iter()
        .filter(|m| m.metadata.get("_mnemo.kind").and_then(|v| v.as_str()) == Some("claim"))
        .collect();
    assert_eq!(claims.len(), 1);
    assert_eq!(db.claims().unwrap().len(), 1);
    // It survives a reopen.
    db.close().unwrap();
    let mut db = Mnemo::open(&p, PW).unwrap();
    db.__set_clock_offset_for_testing(600 + 15 * 60);
    assert!(matches!(
        db.check_claim("PLAN.md tail", &c).unwrap(),
        ClaimStatus::Held { .. }
    ));
}

#[test]
fn a_clock_that_steps_back_cant_stretch_a_claim() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    let mut db = Mnemo::open(&p, PW).unwrap();
    db.set_author(Some("worker-a".into()));
    db.__set_clock_offset_for_testing(3_600);
    let a = token(db.claim("t", Some(60)).unwrap());
    // The clock goes back an hour: the claim is bounded by one lease from
    // the new now, not an hour and a minute.
    db.__set_clock_offset_for_testing(0);
    let Ok(ClaimStatus::Held { expires_at }) = db.check_claim("t", &a) else {
        panic!()
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(expires_at <= now + 61, "{expires_at} vs {now}");
    db.set_author(Some("worker-b".into()));
    assert!(matches!(
        db.claim("t", None).unwrap(),
        ClaimOutcome::HeldBy { .. }
    ));
    db.__set_clock_offset_for_testing(61);
    token(db.claim("t", None).unwrap());
}

#[test]
fn presence_and_who() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    // A long-lived handle holds its presence lock; the CLI's kind doesn't.
    let mut server = shared(&p, "planner-a");
    server.hold_presence().unwrap();
    assert!(server
        .write(|db| db.touch_presence(Some("planner"), Some("mcp")))
        .unwrap());
    // Written at most once a minute, unless something changed.
    assert!(!server
        .write(|db| db.touch_presence(Some("planner"), Some("mcp")))
        .unwrap());
    assert!(server
        .write(|db| db.touch_presence(Some("planner"), Some("cli")))
        .unwrap());
    let tok = token(server.write(|db| db.claim("v0.17 step 3", None)).unwrap());
    let mut cli = Mnemo::open_shared(&p, PW, opts()).unwrap();
    cli.set_author(Some("worker-a".into()));
    assert!(cli
        .write(|db| db.touch_presence(Some("worker"), Some("cli")))
        .unwrap());
    let who = cli.read(|db| db.who()).unwrap();
    let by = |a: &str| who.iter().find(|p| p.agent == a).unwrap().clone();
    assert_eq!(by("planner-a").alive, Alive::Yes);
    assert_eq!(by("planner-a").claims, ["v0.17 step 3"]);
    assert_eq!(by("planner-a").tool.as_deref(), Some("cli"));
    assert_eq!(by("worker-a").alive, Alive::Unknown);
    assert_eq!(by("worker-a").role.as_deref(), Some("worker"));
    // The server goes: its lock is free, so it's not alive. Its claim stays
    // until a taker sees it gone twice (next test).
    drop(server);
    let who = cli.read(|db| db.who()).unwrap();
    assert_eq!(
        who.iter().find(|p| p.agent == "planner-a").unwrap().alive,
        Alive::No
    );
    assert!(matches!(
        cli.read(|db| db.check_claim("v0.17 step 3", &tok)).unwrap(),
        ClaimStatus::Held { .. }
    ));
    // One presence record per agent.
    let n = cli
        .read(|db| db.memories())
        .unwrap()
        .iter()
        .filter(|m| m.metadata.get("_mnemo.kind").and_then(|v| v.as_str()) == Some("presence"))
        .count();
    assert_eq!(n, 2);
}

#[test]
fn a_gone_holders_claim_lapses_early_only_after_two_looks() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    let mut a = shared(&p, "worker-a");
    a.hold_presence().unwrap();
    let ta = token(a.write(|db| db.claim("with a lock", None)).unwrap());
    // A handle without a presence lock: its claim never lapses early.
    let mut plain = shared(&p, "worker-c");
    token(plain.write(|db| db.claim("without a lock", None)).unwrap());
    drop(plain);

    let mut db = Mnemo::open(&p, PW).unwrap();
    db.set_author(Some("worker-b".into()));
    // Alive: held, and no look recorded.
    assert!(matches!(
        db.claim("with a lock", None).unwrap(),
        ClaimOutcome::HeldBy { .. }
    ));
    db.close().unwrap();
    drop(a); // the process is gone (its lock is free)
    let mut db = Mnemo::open(&p, PW).unwrap();
    db.set_author(Some("worker-b".into()));
    // First look: still held, the look recorded.
    assert!(matches!(
        db.claim("with a lock", None).unwrap(),
        ClaimOutcome::HeldBy { .. }
    ));
    db.__set_clock_offset_for_testing(5);
    assert!(matches!(
        db.claim("with a lock", None).unwrap(),
        ClaimOutcome::HeldBy { .. }
    ));
    // Ten seconds after the first look: it lapses, and the old token is lost.
    db.__set_clock_offset_for_testing(10);
    let tb = token(db.claim("with a lock", None).unwrap());
    assert_eq!(
        db.check_claim("with a lock", &ta).unwrap(),
        ClaimStatus::Lost
    );
    assert!(matches!(
        db.check_claim("with a lock", &tb).unwrap(),
        ClaimStatus::Held { .. }
    ));
    // No lock registered: held until expires_at, however long it's gone.
    db.__set_clock_offset_for_testing(60);
    assert!(matches!(
        db.claim("without a lock", None).unwrap(),
        ClaimOutcome::HeldBy { .. }
    ));
    db.__set_clock_offset_for_testing(15 * 60 + 1);
    token(db.claim("without a lock", None).unwrap());
}

/// No lock on the brain between calls: a plain open (which takes the
/// brain's lock for its life) succeeds between a shared handle's calls,
/// with its presence lock held.
#[test]
fn no_lock_on_the_brain_between_calls() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    let mut s = shared(&p, "worker-a");
    s.hold_presence().unwrap();
    for i in 0..3 {
        let t = token(s.write(|db| db.claim(&format!("t{i}"), None)).unwrap());
        s.read(|db| db.check_claim(&format!("t{i}"), &t)).unwrap();
        let f = OpenOptions::new().read(true).write(true).open(&p).unwrap();
        assert!(
            fs4::fs_std::FileExt::try_lock_exclusive(&f).is_ok(),
            "the brain is locked between calls"
        );
        fs4::fs_std::FileExt::unlock(&f).unwrap();
        drop(f);
        let mut plain = Mnemo::open(&p, PW).unwrap();
        plain.set_author(Some("worker-b".into()));
        assert!(matches!(
            plain.claim(&format!("t{i}"), None).unwrap(),
            ClaimOutcome::HeldBy { .. }
        ));
        plain.close().unwrap();
    }
}

// --- 12.4's race: two processes, 1,000 topics --------------------------------

const ROLE: &str = "MNEMO_CLAIMS_ROLE";

/// The child entry point; returns at once in a normal run.
#[test]
fn claims_child() {
    let Ok(agent) = std::env::var(ROLE) else {
        return;
    };
    let path = PathBuf::from(std::env::var("CL_PATH").unwrap());
    let rounds: usize = std::env::var("CL_ROUNDS").unwrap().parse().unwrap();
    let out = PathBuf::from(std::env::var("CL_OUT").unwrap());
    // FLAKE-1's pattern: threads spawning children the whole time.
    let stop = Arc::new(AtomicBool::new(false));
    let spawners: Vec<_> = (0..1)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = Command::new("true")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            })
        })
        .collect();
    let mut s = Mnemo::open_shared(&path, PW, opts()).unwrap();
    s.set_author(Some(agent.clone()));
    s.hold_presence().unwrap();
    let mut log = String::new();
    // Lockstep, so each round is a real race: a process starts round i once
    // the other has finished round i - 1 (its progress file says so).
    let mine = out.with_extension("progress");
    let other = PathBuf::from(std::env::var("CL_OTHER").unwrap());
    let progress = |p: &Path| -> i64 {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(-1)
    };
    let publish = |n: i64| {
        let tmp = mine.with_extension("tmp");
        std::fs::write(&tmp, n.to_string()).unwrap();
        std::fs::rename(&tmp, &mine).unwrap();
    };
    publish(-1);
    for i in 0..rounds {
        while progress(&other) < i as i64 - 1 {
            std::thread::yield_now();
        }
        // The longest lease, so no claim lapses before the count at the end.
        let r = s.write(|db| db.claim(&format!("race-{i}"), Some(86_400)));
        let tag = match r {
            Ok(ClaimOutcome::Claimed { .. }) => "won",
            Ok(ClaimOutcome::HeldBy { .. }) => "held",
            Err(e) => {
                log.push_str(&format!("{i} error {e}\n"));
                publish(i as i64);
                continue;
            }
        };
        log.push_str(&format!("{i} {tag}\n"));
        publish(i as i64);
    }
    stop.store(true, Ordering::Relaxed);
    for t in spawners {
        t.join().unwrap();
    }
    std::fs::File::create(&out)
        .unwrap()
        .write_all(log.as_bytes())
        .unwrap();
    std::process::exit(0);
}

#[cfg(unix)]
#[test]
fn two_processes_race_for_claims_and_exactly_one_wins_each() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    // 200 rounds, or 12.4's 1,000 under MNEMO_STRESS=1 (see the module
    // doc); CL_RACE_ROUNDS sets any other count.
    let stress = std::env::var("MNEMO_STRESS").as_deref() == Ok("1");
    let rounds: usize = std::env::var("CL_RACE_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if stress { 1_000 } else { 200 });
    let exe = std::env::current_exe().unwrap();
    let out = |a: &str| dir.path().join(format!("{a}.log"));
    let children: Vec<_> = [("worker-a", "worker-b"), ("worker-b", "worker-a")]
        .iter()
        .map(|(me, other)| {
            Command::new(&exe)
                .args([
                    "claims_child",
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                    "-q",
                ])
                .env(ROLE, me)
                .env("CL_PATH", &p)
                .env("CL_ROUNDS", rounds.to_string())
                .env("CL_OUT", out(me))
                .env("CL_OTHER", out(other).with_extension("progress"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    let read = |a: &str| -> Vec<String> {
        let text = std::fs::read_to_string(out(a)).unwrap();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        assert!(
            lines.iter().all(|l| !l.contains("error")),
            "{a}: {:?}",
            lines
                .iter()
                .filter(|l| l.contains("error"))
                .collect::<Vec<_>>()
        );
        assert_eq!(lines.len(), rounds, "{a}");
        lines
    };
    let (a, b) = (read("worker-a"), read("worker-b"));
    let mut a_wins = 0;
    for i in 0..rounds {
        let wa = a[i] == format!("{i} won");
        let wb = b[i] == format!("{i} won");
        assert!(wa ^ wb, "round {i}: {} / {}", a[i], b[i]);
        a_wins += usize::from(wa);
    }
    // Both won some (the rounds overlapped), and the brain agrees.
    assert!(
        a_wins > 0 && a_wins < rounds,
        "worker-a won {a_wins} of {rounds}"
    );
    let mut db = Mnemo::open(&p, PW).unwrap();
    let claims = db.claims().unwrap();
    assert_eq!(claims.len(), rounds);
    assert_eq!(
        claims.iter().filter(|c| c.holder == "worker-a").count(),
        a_wins
    );
}

/// RB-012.4: a CLI-style handle (no presence lock) beside the agent's
/// server moves `last_active` only: `who` still says alive, through `mcp`.
#[test]
fn rb012_4_a_cli_call_beside_the_server_keeps_it_alive() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    let mut server = shared(&p, "worker-a");
    server.hold_presence().unwrap();
    server
        .write(|db| db.touch_presence(Some("worker"), Some("mcp")))
        .unwrap();
    let mut cli = Mnemo::open(&p, PW).unwrap();
    cli.set_author(Some("worker-a".into()));
    cli.__set_clock_offset_for_testing(120);
    assert!(cli.touch_presence(Some("worker"), Some("cli")).unwrap());
    cli.close().unwrap();
    let mut db = Mnemo::open(&p, PW).unwrap();
    let w = db.who().unwrap();
    let a = w.iter().find(|x| x.agent == "worker-a").unwrap();
    assert_eq!(
        (a.alive, a.tool.as_deref()),
        (Alive::Yes, Some("mcp")),
        "{a:?}"
    );
    assert!(a.last_active > 0);
}

/// RB-012.6 item 8: the holder comes back between the two looks: the
/// claim holds, and once it's gone again two new looks are needed.
#[test]
fn rb012_6_the_holder_restarting_between_looks_keeps_its_claim() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    let mut a = shared(&p, "worker-a");
    a.hold_presence().unwrap();
    token(a.write(|db| db.claim("t", None)).unwrap());
    drop(a);
    let mut b = Mnemo::open(&p, PW).unwrap();
    b.set_author(Some("worker-b".into()));
    // First look: gone, recorded.
    assert!(matches!(
        b.claim("t", None).unwrap(),
        ClaimOutcome::HeldBy { .. }
    ));
    b.close().unwrap();
    // worker-a restarts.
    let mut a2 = shared(&p, "worker-a");
    a2.hold_presence().unwrap();
    let mut b = Mnemo::open(&p, PW).unwrap();
    b.set_author(Some("worker-b".into()));
    b.__set_clock_offset_for_testing(11);
    assert!(
        matches!(b.claim("t", None).unwrap(), ClaimOutcome::HeldBy { .. }),
        "alive again: held"
    );
    b.close().unwrap();
    drop(a2);
    // Gone again: the earlier look was cleared, so this is a first look...
    let mut b = Mnemo::open(&p, PW).unwrap();
    b.set_author(Some("worker-b".into()));
    b.__set_clock_offset_for_testing(22);
    assert!(matches!(
        b.claim("t", None).unwrap(),
        ClaimOutcome::HeldBy { .. }
    ));
    // ...and ten seconds later the second.
    b.__set_clock_offset_for_testing(33);
    token(b.claim("t", None).unwrap());
}
