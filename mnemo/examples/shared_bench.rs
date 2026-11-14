//! v0.14.0 step 5: the 12.4 shared-mode bench (the design note's
//! "Implementation order" item 5, RB-007.6 rec. 3, and the open-wait note
//! from step 4b). A dev-hooks example, so nothing ships.
//!
//! ```text
//! cargo run --release --example shared_bench -- --cells 1,2,3,4 --sizes 1000,10000,100000
//! cargo run --release --example shared_bench -- --cells 5,6,7,8 --sizes 10000,100000 --secs 8
//! ```
//!
//! Child processes are this binary run again with `MNEMO_SB_ROLE` set
//! (writers, readers, a compactor, an opener). Each child writes its counts
//! and latencies to a file in the run's directory. The results are printed
//! as one JSON object (also written to `--out`).
//!
//! Corpora come from `benches/corpus.rs`: 384 dimensions, clustered
//! vectors, an IVF+PQ index, the fast KDF; each cell works on a copy.

#[path = "../benches/corpus.rs"]
mod corpus;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mnemo_engine::{Memory, MemoryType, Mnemo, MnemoError, SharedMnemo, SharedOptions, Ulid};
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};

const ROLE: &str = "MNEMO_SB_ROLE";
const DIMS: usize = corpus::DIMENSIONS;

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} not set"))
}

fn vector(rng: &mut impl Rng) -> Vec<f32> {
    (0..DIMS).map(|_| rng.gen_range(-1.0f32..1.0)).collect()
}

fn mem(rng: &mut impl Rng, text: String) -> Memory {
    Memory::new(text, MemoryType::Semantic, vector(rng))
}

fn shared(p: &Path) -> SharedMnemo {
    loop {
        match Mnemo::open_shared(p, corpus::PASSPHRASE, SharedOptions::default()) {
            Ok(s) => return s,
            Err(MnemoError::Locked { .. }) => continue,
            Err(e) => panic!("open_shared: {e}"),
        }
    }
}

/// Median, p99 (nearest rank), mean and count of `v`, in milliseconds.
fn stats(v: &mut [f64]) -> Value {
    if v.is_empty() {
        return json!({ "n": 0 });
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| v[(((v.len() as f64) * p).ceil() as usize).clamp(1, v.len()) - 1];
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    json!({
        "n": v.len(),
        "p50_ms": round(pct(0.50)),
        "p99_ms": round(pct(0.99)),
        "mean_ms": round(mean),
        "max_ms": round(v[v.len() - 1]),
    })
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// A working copy of the cached corpus of `n` memories.
fn copy_of(dir: &Path, n: usize, name: &str) -> PathBuf {
    let src = corpus::setup_cached_corpus(n, DIMS);
    let dst = dir.join(format!("{name}-{n}.mnemo"));
    std::fs::copy(&src, &dst).unwrap();
    dst
}

/// Some ids from the brain, for `get`.
fn some_ids(p: &Path, k: usize) -> Vec<Ulid> {
    let mut db = Mnemo::open_read_only(p, corpus::PASSPHRASE).unwrap();
    let mut ids: Vec<Ulid> = db.memories().unwrap().into_iter().map(|m| m.id).collect();
    ids.truncate(k.max(1));
    ids
}

fn rss_kb() -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

// --- Children -----------------------------------------------------------------

fn child() -> ! {
    let role = var(ROLE);
    let path = PathBuf::from(var("SB_PATH"));
    let dir = PathBuf::from(var("SB_DIR"));
    let id = var("SB_ID");
    let secs: f64 = var("SB_SECS").parse().unwrap();
    // Everyone starts together.
    let start_at: f64 = var("SB_START").parse().unwrap();
    let mut rng = rand::rngs::StdRng::seed_from_u64(id.bytes().map(u64::from).sum());
    let out = match role.as_str() {
        "writer" | "reader" | "opener" | "compactor" => {
            let mut s = if role == "writer" || role == "reader" {
                Some(shared(&path))
            } else {
                None
            };
            let ids = if role == "reader" {
                some_ids(&path, 200)
            } else {
                Vec::new()
            };
            wait_until(start_at);
            let end = Instant::now() + Duration::from_secs_f64(secs);
            let mut lat = Vec::new();
            let mut locked = 0u64;
            let mut n = 0u64;
            while Instant::now() < end {
                let t = Instant::now();
                let r = match role.as_str() {
                    "writer" => {
                        let m = mem(&mut rng, format!("{id} write {n}"));
                        s.as_mut()
                            .unwrap()
                            .write(move |db| db.remember(m))
                            .map(|_| ())
                    }
                    "reader" => {
                        let uid = ids[rng.gen_range(0..ids.len())];
                        s.as_mut().unwrap().read(|db| db.get(&uid)).map(|_| ())
                    }
                    "opener" => Mnemo::open_shared(&path, corpus::PASSPHRASE, {
                        let mut o = SharedOptions::default();
                        o.deadline = Duration::from_secs(10);
                        o
                    })
                    .map(drop),
                    _ => {
                        let r = Mnemo::compact_file(path.to_str().unwrap(), corpus::PASSPHRASE)
                            .map(|_| ());
                        // The compaction's own duration (cell 8).
                        lat.push(ms(t.elapsed()));
                        std::thread::sleep(Duration::from_millis(100));
                        r
                    }
                };
                match r {
                    Ok(()) => {
                        if role != "compactor" {
                            lat.push(ms(t.elapsed()));
                        }
                        n += 1;
                    }
                    Err(MnemoError::Locked { .. }) if role == "compactor" => {
                        lat.pop();
                        locked += 1;
                    }
                    Err(MnemoError::Locked { .. }) => locked += 1,
                    Err(e) => panic!("{role} {id}: {e}"),
                }
            }
            json!({ "role": role, "ops": n, "locked": locked, "lat_ms": lat })
        }
        "rss" => {
            let before = rss_kb();
            let mut s = shared(&path);
            s.read(|db| Ok(db.len())).unwrap();
            let after = rss_kb();
            json!({ "rss_before_kb": before, "rss_after_kb": after })
        }
        other => panic!("unknown role {other}"),
    };
    std::fs::write(dir.join(format!("{id}.json")), out.to_string()).unwrap();
    std::process::exit(0);
}

fn wait_until(t: f64) {
    loop {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        if now >= t {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn spawn(
    role: &str,
    id: &str,
    path: &Path,
    dir: &Path,
    secs: f64,
    start: f64,
    young: bool,
) -> Child {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.env(ROLE, role)
        .env("SB_PATH", path)
        .env("SB_DIR", dir)
        .env("SB_ID", id)
        .env("SB_SECS", secs.to_string())
        .env("SB_START", start.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    if young {
        c.env("MNEMO_DEV_BACKOFF", "young");
    } else {
        c.env_remove("MNEMO_DEV_BACKOFF");
    }
    c.spawn().unwrap()
}

fn read_child(dir: &Path, id: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{id}.json"))).unwrap()).unwrap()
}

/// Run `roles` (role, count) together for `secs` on a copy of the `n`
/// corpus; returns each role's ops per second and latency stats.
fn run_mix(n: usize, roles: &[(&str, usize)], secs: f64, young: bool, scratch: &Path) -> Value {
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let path = copy_of(dir.path(), n, "mix");
    // Children open (and readers load ids) before the common start.
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        + 3.0
        + n as f64 / 50_000.0;
    let mut kids = Vec::new();
    for (role, count) in roles {
        for i in 0..*count {
            let id = format!("{role}{i}");
            kids.push((
                id.clone(),
                spawn(role, &id, &path, dir.path(), secs, start, young),
            ));
        }
    }
    let mut per_role: BTreeMap<String, (u64, u64, Vec<f64>)> = BTreeMap::new();
    for (id, mut k) in kids {
        assert!(k.wait().unwrap().success(), "child {id} failed");
        let v = read_child(dir.path(), &id);
        let e = per_role
            .entry(v["role"].as_str().unwrap().to_string())
            .or_default();
        e.0 += v["ops"].as_u64().unwrap();
        e.1 += v["locked"].as_u64().unwrap();
        e.2.extend(
            v["lat_ms"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap()),
        );
    }
    let mut out = serde_json::Map::new();
    for (role, (ops, locked, mut lat)) in per_role {
        out.insert(
            role,
            json!({
                "per_sec": round(ops as f64 / secs),
                "locked": locked,
                "latency": stats(&mut lat),
            }),
        );
    }
    Value::Object(out)
}

// --- Cells --------------------------------------------------------------------

/// Cell 1: the per-call check when nobody else wrote.
fn cell1(n: usize, reps: usize, scratch: &Path) -> Value {
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let p = copy_of(dir.path(), n, "c1");
    let ids = some_ids(&p, 500);
    let mut rng = rand::rngs::StdRng::seed_from_u64(1);
    let mut s = shared(&p);
    for _ in 0..50 {
        s.read(|_| Ok(())).unwrap();
    }
    let (mut empty, mut get) = (Vec::new(), Vec::new());
    for _ in 0..reps {
        let t = Instant::now();
        s.read(|_| Ok(())).unwrap();
        empty.push(ms(t.elapsed()));
        let id = ids[rng.gen_range(0..ids.len())];
        let t = Instant::now();
        s.read(|db| db.get(&id)).unwrap();
        get.push(ms(t.elapsed()));
    }
    drop(s);
    let mut db = Mnemo::open(&p, corpus::PASSPHRASE).unwrap();
    let mut plain_get = Vec::new();
    for _ in 0..reps {
        let id = ids[rng.gen_range(0..ids.len())];
        let t = Instant::now();
        db.get(&id).unwrap();
        plain_get.push(ms(t.elapsed()));
    }
    json!({
        "shared_empty_read_window": stats(&mut empty),
        "shared_get_in_read_window": stats(&mut get),
        "plain_get": stats(&mut plain_get),
    })
}

/// Cells 2 and 3: a write window's cost, with fsyncs, and the no-op windows.
fn cell2_3(n: usize, reps: usize, scratch: &Path) -> Value {
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let p = copy_of(dir.path(), n, "c2");
    let mut rng = rand::rngs::StdRng::seed_from_u64(2);
    let mut s = shared(&p);
    let (mut lat, mut fsyncs) = (Vec::new(), Vec::new());
    for i in 0..reps {
        let m = mem(&mut rng, format!("shared write {i}"));
        let f0 = mnemo_engine::__fsync_count();
        let t = Instant::now();
        s.write(move |db| db.remember(m)).unwrap();
        lat.push(ms(t.elapsed()));
        fsyncs.push((mnemo_engine::__fsync_count() - f0) as f64);
    }
    // 3: an empty write window and a refused write.
    let txn = |s: &mut SharedMnemo| s.read(|db| Ok(db.__transaction_id())).unwrap();
    let t0 = txn(&mut s);
    let (mut empty, mut refused, mut after_refused, mut noop_fsyncs) =
        (Vec::new(), Vec::new(), Vec::new(), 0u64);
    // Empty write windows back to back.
    for _ in 0..reps {
        let f0 = mnemo_engine::__fsync_count();
        let t = Instant::now();
        s.write(|_| Ok(())).unwrap();
        empty.push(ms(t.elapsed()));
        noop_fsyncs += mnemo_engine::__fsync_count() - f0;
    }
    // A refused write, then the next call: a failed write window clears
    // the stamp, so the next call reloads.
    for _ in 0..reps {
        let f0 = mnemo_engine::__fsync_count();
        let t = Instant::now();
        let zero = Memory::new("zero", MemoryType::Semantic, vec![0.0; DIMS]);
        assert!(s.write(move |db| db.remember(zero)).is_err());
        refused.push(ms(t.elapsed()));
        let t = Instant::now();
        s.read(|_| Ok(())).unwrap();
        after_refused.push(ms(t.elapsed()));
        noop_fsyncs += mnemo_engine::__fsync_count() - f0;
    }
    let t1 = txn(&mut s);
    drop(s);
    let mut db = Mnemo::open(&p, corpus::PASSPHRASE).unwrap();
    let (mut plain, mut plain_fsyncs) = (Vec::new(), Vec::new());
    for i in 0..reps {
        let m = mem(&mut rng, format!("plain write {i}"));
        let f0 = mnemo_engine::__fsync_count();
        let t = Instant::now();
        db.remember(m).unwrap();
        db.flush().unwrap();
        plain.push(ms(t.elapsed()));
        plain_fsyncs.push((mnemo_engine::__fsync_count() - f0) as f64);
    }
    let avg = |v: &[f64]| round(v.iter().sum::<f64>() / v.len() as f64);
    json!({
        "shared_remember_window": stats(&mut lat),
        "shared_remember_fsyncs_mean": avg(&fsyncs),
        "plain_remember_and_flush": stats(&mut plain),
        "plain_remember_fsyncs_mean": avg(&plain_fsyncs),
        "noop_empty_write_window": stats(&mut empty),
        "noop_refused_write": stats(&mut refused),
        "next_read_after_a_refused_write": stats(&mut after_refused),
        "noop_transaction_id_unchanged": t0 == t1,
        "noop_fsyncs_total": noop_fsyncs,
    })
}

/// Cell 4: the full-reload catch-up, and RSS per shared handle.
fn cell4(n: usize, reps: usize, scratch: &Path) -> Value {
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let p = copy_of(dir.path(), n, "c4");
    let mut rng = rand::rngs::StdRng::seed_from_u64(4);
    let mut a = shared(&p);
    let mut b = shared(&p);
    let mut lat = Vec::new();
    for i in 0..reps {
        let m = mem(&mut rng, format!("other's write {i}"));
        b.write(move |db| db.remember(m)).unwrap();
        let t = Instant::now();
        a.read(|_| Ok(())).unwrap();
        lat.push(ms(t.elapsed()));
    }
    drop((a, b));
    // RSS of a fresh process before and after one shared handle opens.
    let mut rss = Vec::new();
    for i in 0..3 {
        let id = format!("rss{i}");
        let mut c = spawn("rss", &id, &p, dir.path(), 0.0, 0.0, false);
        assert!(c.wait().unwrap().success());
        rss.push(read_child(dir.path(), &id));
    }
    let mut deltas: Vec<f64> = rss
        .iter()
        .map(|v| {
            (v["rss_after_kb"].as_u64().unwrap() as f64
                - v["rss_before_kb"].as_u64().unwrap() as f64)
                / 1024.0
        })
        .collect();
    deltas.sort_by(|x, y| x.partial_cmp(y).unwrap());
    json!({
        "reload_read_after_another_commit": stats(&mut lat),
        "rss_per_shared_handle_mb_median": round(deltas[deltas.len() / 2]),
        "rss_samples": rss,
    })
}

/// Cell 8: the open wait under compaction: an opener against a compaction every
/// ~100 ms and three writers.
fn cell8(n: usize, secs: f64, scratch: &Path) -> Value {
    let v = run_mix(
        n,
        &[("writer", 3), ("compactor", 1), ("opener", 1)],
        secs,
        false,
        scratch,
    );
    json!({
        "opener": v["opener"],
        "writer": v["writer"],
        "compactor": v["compactor"],
    })
}

fn main() {
    if std::env::var(ROLE).is_ok() {
        child();
    }
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str, default: &str| -> String {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
            .unwrap_or_else(|| default.to_string())
    };
    let cells: Vec<u32> = arg("--cells", "1,2,3,4,5,6,7,8")
        .split(',')
        .map(|c| c.parse().unwrap())
        .collect();
    let sizes: Vec<usize> = arg("--sizes", "1000,10000,100000")
        .split(',')
        .map(|c| c.parse().unwrap())
        .collect();
    let secs: f64 = arg("--secs", "8").parse().unwrap();
    let out_path = arg("--out", "");
    let scratch = PathBuf::from(arg(
        "--scratch",
        &std::env::temp_dir().display().to_string(),
    ));
    let mut out = serde_json::Map::new();
    let reps_for = |n: usize, small: usize| if n >= 100_000 { small / 2 } else { small };
    for &n in &sizes {
        let key = n.to_string();
        let mut cellv = serde_json::Map::new();
        if cells.contains(&1) {
            eprintln!("cell 1 at {n}");
            cellv.insert(
                "1_per_call_check".into(),
                cell1(n, reps_for(n, 2000), &scratch),
            );
        }
        if cells.contains(&2) || cells.contains(&3) {
            eprintln!("cells 2 and 3 at {n}");
            cellv.insert(
                "2_3_write_and_noop".into(),
                cell2_3(n, reps_for(n, 200), &scratch),
            );
        }
        if cells.contains(&4) {
            eprintln!("cell 4 at {n}");
            cellv.insert(
                "4_reload_and_rss".into(),
                cell4(n, reps_for(n, 60), &scratch),
            );
        }
        let mixes: [(&str, &[(&str, usize)]); 4] = [
            ("1w_1r", &[("writer", 1), ("reader", 1)]),
            ("1w_7r", &[("writer", 1), ("reader", 7)]),
            ("2w", &[("writer", 2)]),
            ("8w", &[("writer", 8)]),
        ];
        let several: [(&str, &[(&str, usize)]); 2] = [
            ("4w_2r", &[("writer", 4), ("reader", 2)]),
            ("8w_2r", &[("writer", 8), ("reader", 2)]),
        ];
        for (shape, young) in [("doubling", false), ("young", true)] {
            if young && !cells.contains(&7) {
                continue;
            }
            if !(young || cells.contains(&5) || cells.contains(&6) || cells.contains(&7)) {
                continue;
            }
            if cells.contains(&5) || cells.contains(&7) {
                for (name, roles) in mixes {
                    eprintln!("cell 5 ({shape}) {name} at {n}");
                    cellv.insert(
                        format!("5_{shape}_{name}"),
                        run_mix(n, roles, secs, young, &scratch),
                    );
                }
            }
            if cells.contains(&6) || cells.contains(&7) {
                for (name, roles) in several {
                    eprintln!("cell 6 ({shape}) {name} at {n}");
                    cellv.insert(
                        format!("6_{shape}_{name}"),
                        run_mix(n, roles, secs, young, &scratch),
                    );
                }
            }
        }
        if cells.contains(&8) {
            eprintln!("cell 8 at {n}");
            cellv.insert(
                "8_open_wait_under_compaction".into(),
                cell8(n, secs * 2.5, &scratch),
            );
        }
        out.insert(key, Value::Object(cellv));
    }
    let v = Value::Object(out);
    let text = serde_json::to_string_pretty(&v).unwrap();
    println!("{text}");
    if !out_path.is_empty() {
        std::fs::write(&out_path, text).unwrap();
    }
}
