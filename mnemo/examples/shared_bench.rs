//! v0.14.0 step 5: the 12.4 shared-mode bench (the design note's
//! "Implementation order" item 5, RB-007.6 rec. 3, and the open-wait note
//! from step 4b). A dev-hooks example, so nothing ships.
//!
//! ```text
//! cargo run --release --example shared_bench -- --cells 1,2,3,4 --sizes 1000,10000,100000
//! cargo run --release --example shared_bench -- --cells 5,6,7,8 --sizes 10000,100000 --secs 8
//! cargo run --release --example shared_bench -- --cells 9 --sizes 10000 --secs 20 --valve-ms 250,500,1000,2000
//! cargo run --release --features failpoints --example shared_bench -- --cells 10,11 --sizes 10000,100000
//! cargo run --release --example shared_bench -- --cells 12 --sizes 10000,100000
//! cargo run --release --features failpoints --example shared_bench -- --cells 10 --sizes 100000 --commits 500
//! ```
//!
//! Cells 10 and 11 (v0.20.0, v0200-3) are the incremental catch-up's
//! (`docs/design/catch-up.md`): 10 is a reader's catch-up after another
//! handle's one-memory commit, the recalls after it and the memory it
//! keeps (`--commits N` for the long run, `--reps N` for its loops); 11 is what the first recall
//! pays to rebuild each lazily built part a catch-up may drop; 12 is cell
//! 10's footprint control (as many recalls, no commit). With the
//! `failpoints` feature, cell 10 also counts incremental and full
//! catch-ups.
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

use mnemo_engine::{
    Memory, MemoryType, Mnemo, MnemoError, RecallRequest, SharedMnemo, SharedOptions, Ulid,
};
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

/// The physical footprint in MB (`vmmap --summary`, macOS): what memory
/// pressure acts on. `ps`'s RSS also counts freed pages the allocator
/// hasn't returned. `None` elsewhere.
fn footprint_mb() -> Option<f64> {
    let o = Command::new("vmmap")
        .args(["--summary", &std::process::id().to_string()])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout);
    let v = s
        .lines()
        .find(|l| l.starts_with("Physical footprint:"))?
        .split_whitespace()
        .last()?
        .to_string();
    let (num, unit) = v.split_at(v.len() - 1);
    let x: f64 = num.parse().ok()?;
    match unit {
        "K" => Some(x / 1024.0),
        "M" => Some(x),
        "G" => Some(x * 1024.0),
        _ => None,
    }
}

fn recall_req(rng: &mut impl Rng) -> RecallRequest {
    RecallRequest::new(vector(rng))
        .top_k(10)
        .track_access(false)
}

#[cfg(feature = "failpoints")]
fn catch_ups(s: &SharedMnemo) -> Value {
    let (inc, full) = s.__catch_ups();
    json!({ "incremental": inc, "full": full })
}

#[cfg(not(feature = "failpoints"))]
fn catch_ups(_: &SharedMnemo) -> Value {
    Value::Null
}

/// The handle's full reloads so far (0 without the counter).
#[cfg(feature = "failpoints")]
fn full_reloads(s: &SharedMnemo) -> u64 {
    s.__catch_ups().1
}

#[cfg(not(feature = "failpoints"))]
fn full_reloads(_: &SharedMnemo) -> u64 {
    0
}

/// What the handle holds (the dev-hooks budget report, MB), with the
/// vector column's filled rows: live data, to set against the footprint.
fn held(s: &mut SharedMnemo) -> Value {
    let (r, filled) = s
        .read(|db| Ok((db.__budget_report()?, db.__column_filled().len())))
        .unwrap();
    let mb = |b: u64| round(b as f64 / 1_048_576.0);
    json!({
        "total_mb": mb(r.ram_total),
        "catalog_mb": mb(r.ram_catalog),
        "id_map_mb": mb(r.ram_id_map),
        "ann_mb": mb(r.ram_ann_centroids
            + r.ram_ann_codebook
            + r.ram_ann_posting_ids
            + r.ram_ann_posting_codes
            + r.ram_ann_assignment),
        "vector_column_mb": mb(r.ram_vector_column),
        "column_rows_filled": filled,
    })
}

/// Cell 10: a reader's catch-up after another handle's one-memory commit,
/// the recalls after it, and the memory the process keeps.
fn cell10(n: usize, reps: usize, commits: usize, scratch: &Path) -> Value {
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let p = copy_of(dir.path(), n, "c10");
    let mut rng = rand::rngs::StdRng::seed_from_u64(10);
    let mut a = shared(&p);
    let mut b = shared(&p);
    for _ in 0..20 {
        let q = recall_req(&mut rng);
        a.read(|db| db.recall(&q)).unwrap();
    }
    let mut warm = Vec::new();
    for _ in 0..reps {
        let q = recall_req(&mut rng);
        let t = Instant::now();
        a.read(|db| db.recall(&q)).unwrap();
        warm.push(ms(t.elapsed()));
    }
    // The control: a warm recall after the thread sleeps about as long as a
    // commit takes, with no commit. On the machine this was measured on, a
    // recall after any such pause runs slower, whatever the engine does,
    // so the recalls after a commit are read against this row too.
    let mut idle = Vec::new();
    for _ in 0..reps {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let q = recall_req(&mut rng);
        let t = Instant::now();
        a.read(|db| db.recall(&q)).unwrap();
        idle.push(ms(t.elapsed()));
    }
    let start = (rss_kb() as f64 / 1024.0, footprint_mb());
    let held_start = held(&mut a);
    let (mut empty, mut first, mut second, mut direct) = (vec![], vec![], vec![], vec![]);
    for i in 0..reps {
        let m = mem(&mut rng, format!("other's write {i}"));
        b.write(move |db| db.remember(m)).unwrap();
        let t = Instant::now();
        a.read(|_| Ok(())).unwrap();
        empty.push(ms(t.elapsed()));
        let q = recall_req(&mut rng);
        let t = Instant::now();
        a.read(|db| db.recall(&q)).unwrap();
        first.push(ms(t.elapsed()));
        let q = recall_req(&mut rng);
        let t = Instant::now();
        a.read(|db| db.recall(&q)).unwrap();
        second.push(ms(t.elapsed()));
    }
    for i in 0..reps {
        let m = mem(&mut rng, format!("other's write b{i}"));
        b.write(move |db| db.remember(m)).unwrap();
        let q = recall_req(&mut rng);
        let t = Instant::now();
        a.read(|db| db.recall(&q)).unwrap();
        direct.push(ms(t.elapsed()));
    }
    // Warm recalls again, back to back with no commit: does the caught-up
    // state recall as fast as the state it started from?
    let mut warm_after = Vec::new();
    for _ in 0..reps {
        let q = recall_req(&mut rng);
        let t = Instant::now();
        a.read(|db| db.recall(&q)).unwrap();
        warm_after.push(ms(t.elapsed()));
    }
    let after = (rss_kb() as f64 / 1024.0, footprint_mb());
    let held_after = held(&mut a);
    let counts = catch_ups(&a);
    // The long run: `commits` more one-memory commits, each followed by a
    // catch-up and a recall, with the footprint every 25.
    let mut series = Vec::new();
    let long_start = (rss_kb() as f64 / 1024.0, footprint_mb());
    let mut long = Vec::new();
    // Each commit's catch-up (an empty read) and the recall after it, split
    // by the path the catch-up took: the full reloads here are the fallback
    // that section 8 compares with today's reload.
    let (mut cu_inc, mut cu_full, mut rec_inc, mut rec_full) = (vec![], vec![], vec![], vec![]);
    for i in 0..commits {
        let m = mem(&mut rng, format!("long run {i}"));
        b.write(move |db| db.remember(m)).unwrap();
        let before = full_reloads(&a);
        let t = Instant::now();
        a.read(|_| Ok(())).unwrap();
        let cu = ms(t.elapsed());
        let full = full_reloads(&a) != before;
        let q = recall_req(&mut rng);
        let t = Instant::now();
        a.read(|db| db.recall(&q)).unwrap();
        let r = ms(t.elapsed());
        long.push(r);
        if full {
            cu_full.push(cu);
            rec_full.push(r);
        } else {
            cu_inc.push(cu);
            rec_inc.push(r);
        }
        if (i + 1) % 25 == 0 {
            series.push(json!({
                "commits": i + 1,
                "rss_mb": round(rss_kb() as f64 / 1024.0),
                "footprint_mb": footprint_mb().map(round),
            }));
        }
    }
    let long_end = (rss_kb() as f64 / 1024.0, footprint_mb());
    json!({
        "reps": reps,
        "warm_recall_no_commit": stats(&mut warm),
        "warm_recall_after_the_catch_ups": stats(&mut warm_after),
        "warm_recall_after_20ms_sleep_no_commit": stats(&mut idle),
        "catch_up_empty_read_after_commit": stats(&mut empty),
        "first_recall_after_catch_up": stats(&mut first),
        "second_recall_after_catch_up": stats(&mut second),
        "recall_as_first_call_after_commit": stats(&mut direct),
        "rss_mb_before_after": [round(start.0), round(after.0)],
        "footprint_mb_before_after": [start.1.map(round), after.1.map(round)],
        "held_before_after": [held_start, held_after],
        "catch_ups": counts,
        "long_run": {
            "commits": commits,
            "recall_after_each_catch_up": stats(&mut long),
            "catch_up_incremental": stats(&mut cu_inc),
            "catch_up_full": stats(&mut cu_full),
            "recall_after_incremental": stats(&mut rec_inc),
            "recall_after_full": stats(&mut rec_full),
            "rss_mb_before_after": [round(long_start.0), round(long_end.0)],
            "footprint_mb_before_after": [long_start.1.map(round), long_end.1.map(round)],
            "series": series,
            "catch_ups": catch_ups(&a),
        },
    })
}

/// Cell 12 (v0200-3): the control for cell 10's footprint, the same warm-up
/// and as many recalls as cell 10 makes, with no commit, so what the
/// recalls themselves fill (the vector column, the page cache) is measured
/// apart from what the catch-ups keep.
fn cell12(n: usize, reps: usize, scratch: &Path) -> Value {
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let p = copy_of(dir.path(), n, "c12");
    let mut rng = rand::rngs::StdRng::seed_from_u64(10);
    let mut a = shared(&p);
    for _ in 0..20 + 2 * reps {
        let q = recall_req(&mut rng);
        a.read(|db| db.recall(&q)).unwrap();
    }
    let start = (rss_kb() as f64 / 1024.0, footprint_mb());
    for _ in 0..4 * reps {
        let q = recall_req(&mut rng);
        a.read(|db| db.recall(&q)).unwrap();
    }
    let after = (rss_kb() as f64 / 1024.0, footprint_mb());
    json!({
        "recalls": 4 * reps,
        "rss_mb_before_after": [round(start.0), round(after.0)],
        "footprint_mb_before_after": [start.1.map(round), after.1.map(round)],
    })
}

/// Cell 11 (v0200-3, addition (d)): what the first recall pays to rebuild
/// each lazily built part, dropped alone on a warm handle.
fn cell11(n: usize, reps: usize, scratch: &Path) -> Value {
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let p = copy_of(dir.path(), n, "c11");
    let mut rng = rand::rngs::StdRng::seed_from_u64(11);
    let mut a = shared(&p);
    for _ in 0..20 {
        let q = recall_req(&mut rng);
        a.read(|db| db.recall(&q)).unwrap();
    }
    let mut out = serde_json::Map::new();
    for which in [
        "none",
        "key_index",
        "superseded",
        "layers",
        "agent_set",
        "bm25_stats",
        "bm25_dicts",
        "ann",
        "column",
    ] {
        let mut lat = Vec::new();
        for _ in 0..reps {
            if which != "none" {
                a.read(|db| db.__drop_lazy_for_testing(which)).unwrap();
            }
            let q = recall_req(&mut rng);
            let t = Instant::now();
            a.read(|db| db.recall(&q)).unwrap();
            lat.push(ms(t.elapsed()));
        }
        out.insert(which.into(), stats(&mut lat));
    }
    Value::Object(out)
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
    let commits: usize = arg("--commits", "0").parse().unwrap();
    // Cell 10's loop count (default: 200 at 10K, 100 at 100K).
    let reps10: Option<usize> = args
        .iter()
        .position(|a| a == "--reps")
        .and_then(|i| args.get(i + 1))
        .map(|r| r.parse().unwrap());
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
        if cells.contains(&10) {
            eprintln!("cell 10 at {n}");
            cellv.insert(
                "10_catch_up".into(),
                cell10(n, reps10.unwrap_or(reps_for(n, 100) * 2), commits, &scratch),
            );
        }
        if cells.contains(&12) {
            eprintln!("cell 12 at {n}");
            cellv.insert(
                "12_recalls_alone".into(),
                cell12(n, reps_for(n, 100) * 2, &scratch),
            );
        }
        if cells.contains(&11) {
            eprintln!("cell 11 at {n}");
            cellv.insert("11_lazy_parts".into(), cell11(n, 20, &scratch));
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
        // LOCK-2 (cell 9): readers alone, with and without the valve; then
        // the threshold sweep on the cheap-read cells (RB-008.1 rec. 3).
        if cells.contains(&9) {
            let sweep: Vec<String> = arg("--valve-ms", "250,500,1000,2000")
                .split(',')
                .map(str::to_string)
                .collect();
            let alone: [(&str, &[(&str, usize)]); 2] =
                [("8r", &[("reader", 8)]), ("16r", &[("reader", 16)])];
            for (valve, off) in [("with", false), ("without", true)] {
                if off {
                    std::env::set_var("MNEMO_DEV_NO_VALVE", "1");
                }
                for (name, roles) in alone {
                    eprintln!("cell 9 {name} alone, {valve} the valve, at {n}");
                    cellv.insert(
                        format!("9_alone_{name}_{valve}_valve"),
                        run_mix(n, roles, secs, false, &scratch),
                    );
                }
                std::env::remove_var("MNEMO_DEV_NO_VALVE");
            }
            let cheap: [(&str, &[(&str, usize)]); 3] = [
                ("1w_7r", &[("writer", 1), ("reader", 7)]),
                ("1w_8r", &[("writer", 1), ("reader", 8)]),
                ("1w_16r", &[("writer", 1), ("reader", 16)]),
            ];
            for t in &sweep {
                std::env::set_var("MNEMO_DEV_VALVE_MS", t);
                for (name, roles) in cheap {
                    eprintln!("cell 9 T={t} ms {name} at {n}");
                    cellv.insert(
                        format!("9_t{t}_{name}"),
                        run_mix(n, roles, secs, false, &scratch),
                    );
                }
            }
            std::env::remove_var("MNEMO_DEV_VALVE_MS");
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
