//! v0.21.0 step 2 (spec.v0210 rev 2, RB-019.8): the merge's properties over
//! random interleavings. Two (and three) copies of one brain each take a
//! random run of writes, edits, keyed writes, deletes, essence, task,
//! message and thread operations, with each copy's clock ahead of, behind,
//! or in the same millisecond as the others'. The rules
//! (`__merge_records_for_testing`, what `merge_files` writes) must give:
//!
//! - merge(a,b) = merge(b,a), in every record (so every key's chain, the
//!   superseded flags, the conflict stamps, the copies' ids, the
//!   tombstones, every task's and finding's status and history), with the
//!   report mirrored; claims and presence come from the first input only;
//! - merge(x,x) = x; merge(merge(a,b),b) = merge(a,b) = merge(merge(a,b),a);
//! - merge(merge(a,b),c) = merge(a,merge(b,c));
//! - merging a copy with its own later copy (a copy of it, written on
//!   after) gives that later copy;
//! - every (id, content) of a fact in either input is in the output, as the
//!   record or as a conflict copy, unless a delete names it or the output's
//!   version descends from it (decision.v0210-ancestry: its stamp and hash
//!   are among the output version's `_mnemo.hlc_prev`); and nothing a delete names is
//!   in the output under any id.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mnemo_engine::{
    EssenceScope, EssenceSection, KdfParams, Memory, MemoryPatch, MemoryType, MergeOptions,
    MergeReport, MergeSecret, Mnemo, MnemoConfig, TaskSpec, ThreadTurn, Ulid,
};
use proptest::prelude::*;
use serde_json::Value;

const PW: &str = "proptest-merge-passphrase";
const HOUR: i64 = 3_600_000;

#[derive(Clone, Debug)]
enum Op {
    Remember(u8),
    Keyed(u8, u8),
    Edit(u8, u8),
    Forget(u8),
    ForgetKeyed(u8),
    EssenceEdit(u8),
    EssenceResolve,
    TaskAdd(u8),
    TaskTake,
    TaskReady,
    /// v0210-4b: a move to blocked, and a note that moves nothing
    /// (decision.v0210-task-status).
    TaskBlock,
    TaskNote(u8),
    Post(u8),
    AckAll,
    Turn(u8),
    ThreadForget,
    // RB-020.4: what a build before v0.21 writes: a fact and a key's
    // version with no stamps, and a delete with no tombstone.
    OlderRemember(u8),
    OlderKeyed(u8, u8),
    OlderForget(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        any::<u8>().prop_map(Op::Remember),
        (0u8..3, any::<u8>()).prop_map(|(k, v)| Op::Keyed(k, v)),
        (any::<u8>(), 0u8..4).prop_map(|(t, v)| Op::Edit(t, v)),
        any::<u8>().prop_map(Op::Forget),
        (0u8..3).prop_map(Op::ForgetKeyed),
        (0u8..4).prop_map(Op::EssenceEdit),
        Just(Op::EssenceResolve),
        (0u8..3).prop_map(Op::TaskAdd),
        Just(Op::TaskTake),
        Just(Op::TaskReady),
        Just(Op::TaskBlock),
        (0u8..4).prop_map(Op::TaskNote),
        (0u8..4).prop_map(Op::Post),
        Just(Op::AckAll),
        (0u8..4).prop_map(Op::Turn),
        Just(Op::ThreadForget),
        any::<u8>().prop_map(Op::OlderRemember),
        (0u8..3, any::<u8>()).prop_map(|(k, v)| Op::OlderKeyed(k, v)),
        any::<u8>().prop_map(Op::OlderForget),
    ]
}

/// A copy's clock: the same millisecond as the base, or an hour either way.
fn clock() -> impl Strategy<Value = i64> {
    prop_oneof![Just(0i64), Just(HOUR), Just(-HOUR), Just(1i64)]
}

fn vector(n: u32) -> Vec<f32> {
    let x = n as f32;
    vec![1.0 + (x * 0.37).sin(), (x * 0.11).cos(), 0.5, 0.25]
}

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The base every copy starts from.
fn base(p: &Path) {
    let mut db = Mnemo::create(p, PW, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    for i in 0..3 {
        db.remember(Memory::new(
            format!("base fact {i}"),
            MemoryType::Semantic,
            vector(i),
        ))
        .unwrap();
    }
    db.remember_keyed(
        Memory::new("key zero, base", MemoryType::Semantic, vector(10)),
        "k0",
    )
    .unwrap();
    db.essence_add(
        EssenceScope::Project,
        EssenceSection::Open,
        "an open item",
        None,
    )
    .unwrap();
    db.set_author(Some("planner-a".into()));
    let mut spec = TaskSpec::new("The base task");
    spec.role = Some("worker".into());
    db.task_add(&spec).unwrap();
    db.close().unwrap();
}

fn facts(db: &mut Mnemo) -> Vec<Ulid> {
    db.memories()
        .unwrap()
        .into_iter()
        .filter(|m| {
            m.metadata.get("_mnemo.kind").is_none()
                && m.metadata.get("topic").and_then(Value::as_str) != Some("manifest")
                && m.metadata.get("_mnemo.key").is_none()
        })
        .map(|m| m.id)
        .collect()
}

fn of_kind(db: &mut Mnemo, kind: &str) -> Vec<Memory> {
    db.memories()
        .unwrap()
        .into_iter()
        .filter(|m| m.metadata.get("_mnemo.kind").and_then(Value::as_str) == Some(kind))
        .collect()
}

/// Run `ops` on the copy at `p` with its clock `offset` ms from `t`, and
/// read what a merge reads from it.
fn diverge(p: &Path, ops: &[Op], t: u64, offset: i64, who: &str) -> (Vec<Memory>, Vec<Ulid>) {
    let mut db = Mnemo::open(p, PW).unwrap();
    db.__freeze_hlc_clock_for_testing(Some((t as i64 + offset) as u64));
    db.set_author(Some(who.into()));
    let mut n = 0u32;
    for o in ops {
        n += 1;
        // A write the engine refuses (nothing to act on) is skipped.
        let _ = match o {
            Op::Remember(x) => db
                .remember(Memory::new(
                    format!("{who} wrote {x}"),
                    MemoryType::Semantic,
                    vector(100 + n),
                ))
                .map(|_| ()),
            Op::Keyed(k, x) => db
                .remember_keyed(
                    Memory::new(
                        format!("key {k} is {x}"),
                        MemoryType::Semantic,
                        vector(200 + n),
                    ),
                    &format!("k{k}"),
                )
                .map(|_| ()),
            Op::Edit(target, x) => {
                let f = facts(&mut db);
                if f.is_empty() {
                    Ok(())
                } else {
                    db.update(
                        &f[*target as usize % f.len()],
                        MemoryPatch {
                            content: Some(format!("edited to {x}")),
                            vector: Some(vector(300 + *x as u32)),
                            ..Default::default()
                        },
                    )
                    .map(|_| ())
                }
            }
            Op::Forget(target) => {
                let f = facts(&mut db);
                if f.is_empty() {
                    Ok(())
                } else {
                    db.delete(&f[*target as usize % f.len()])
                }
            }
            Op::ForgetKeyed(k) => {
                match db
                    .history_in(&format!("k{k}"), mnemo_engine::KeyScope::private_default())
                    .ok()
                    .and_then(|h| h.first().map(|m| m.id))
                {
                    Some(id) => db.delete(&id),
                    None => Ok(()),
                }
            }
            Op::EssenceEdit(x) => match of_kind(&mut db, "essence").first() {
                Some(e) => db
                    .essence_update(&e.id, &format!("the item, take {x}"), None)
                    .map(|_| ()),
                None => Ok(()),
            },
            Op::EssenceResolve => match of_kind(&mut db, "essence").first() {
                Some(e) => db.essence_resolve(&e.id).map(|_| ()),
                None => Ok(()),
            },
            Op::TaskAdd(s) => {
                let mut spec = TaskSpec::new(format!("Task {s}"));
                spec.role = Some("worker".into());
                db.task_add(&spec).map(|_| ())
            }
            Op::TaskTake => db.task_next(Some("worker")).map(|_| ()),
            Op::TaskReady => {
                let mine: Vec<String> = db
                    .tasks(None, None)
                    .unwrap()
                    .into_iter()
                    .filter(|t| t.status == "active" && t.owner.as_deref() == Some(who))
                    .map(|t| t.slug)
                    .collect();
                match mine.first() {
                    Some(s) => db.task_update(s, Some("ready"), None, None).map(|_| ()),
                    None => Ok(()),
                }
            }
            Op::TaskBlock | Op::TaskNote(_) => {
                let mine: Vec<String> = db
                    .tasks(None, None)
                    .unwrap()
                    .into_iter()
                    .filter(|t| t.status == "active" && t.owner.as_deref() == Some(who))
                    .map(|t| t.slug)
                    .collect();
                let (to, note) = match o {
                    Op::TaskNote(x) => (None, format!("note {x} from {who}")),
                    _ => (Some("blocked"), format!("blocked by {who}")),
                };
                match mine.first() {
                    Some(s) => db.task_update(s, to, Some(&note), None).map(|_| ()),
                    None => Ok(()),
                }
            }
            Op::Post(x) => db
                .post(
                    "planner-a",
                    &format!("note {x}"),
                    "",
                    &[],
                    None,
                    Some(who),
                    None,
                )
                .map(|_| ()),
            Op::AckAll => {
                let ids: Vec<Ulid> = db
                    .inbox(Some("planner-a"), None, true)
                    .unwrap()
                    .into_iter()
                    .map(|m| m.id)
                    .collect();
                ids.iter()
                    .try_for_each(|id| db.ack(id, None, Some("planner-a")).map(|_| ()))
            }
            Op::Turn(u) => db
                .ingest_thread_with_vectors(
                    "t",
                    &[ThreadTurn {
                        turn_id: format!("u{u}"),
                        role: "user".into(),
                        content: format!("turn {u} from {who}"),
                        timestamp: None,
                    }],
                    &[vector(400 + n)],
                )
                .map(|_| ()),
            Op::ThreadForget => db.thread_forget("t").map(|_| ()),
            Op::OlderRemember(_) | Op::OlderKeyed(..) | Op::OlderForget(_) => {
                db.__write_as_older_build_for_testing(true);
                let r = match o {
                    Op::OlderRemember(x) => db
                        .remember(Memory::new(
                            format!("{who}, as an older build, wrote {x}"),
                            MemoryType::Semantic,
                            vector(500 + n),
                        ))
                        .map(|_| ()),
                    Op::OlderKeyed(k, x) => db
                        .remember_keyed(
                            Memory::new(
                                format!("key {k} is {x}, by an older build"),
                                MemoryType::Semantic,
                                vector(600 + n),
                            ),
                            &format!("k{k}"),
                        )
                        .map(|_| ()),
                    Op::OlderForget(target) => {
                        let f = facts(&mut db);
                        if f.is_empty() {
                            Ok(())
                        } else {
                            db.delete(&f[*target as usize % f.len()])
                        }
                    }
                    _ => unreachable!(),
                };
                db.__write_as_older_build_for_testing(false);
                r
            }
        };
    }
    db.close().unwrap();
    let mut ro = Mnemo::open_read_only(p, PW).unwrap();
    ro.__merge_input_for_testing().unwrap()
}

type Input = (Vec<Memory>, Vec<Ulid>);

fn merge(a: &Input, b: &Input, now: u64) -> (Vec<Memory>, MergeReport) {
    Mnemo::__merge_records_for_testing(a.0.clone(), a.1.clone(), b.0.clone(), b.1.clone(), now)
}

fn local(m: &Memory) -> bool {
    matches!(
        m.metadata.get("_mnemo.kind").and_then(Value::as_str),
        Some("claim" | "presence")
    )
}

/// Records as compared: by id, as JSON, claims and presence left out.
fn norm(recs: &[Memory]) -> Vec<Value> {
    let mut v: Vec<&Memory> = recs.iter().filter(|m| !local(m)).collect();
    v.sort_by_key(|m| m.id);
    v.iter().map(|m| serde_json::to_value(m).unwrap()).collect()
}

/// Records without the names of key versions (`_mnemo.ver`,
/// `_mnemo.hlc_prev`), for the laws a version an older build marked in
/// place keeps but for its name.
fn unnamed(recs: &[Value]) -> Vec<Value> {
    recs.iter()
        .map(|r| {
            let mut r = r.clone();
            if let Some(md) = r.get_mut("metadata").and_then(Value::as_object_mut) {
                if md.contains_key("_mnemo.key") {
                    md.remove("_mnemo.ver");
                    md.remove("_mnemo.hlc_prev");
                }
            }
            r
        })
        .collect()
}

fn mirrored(r: &MergeReport) -> MergeReport {
    let mut m = r.clone();
    std::mem::swap(&mut m.a_only, &mut m.b_only);
    m.lead_ms.swap(0, 1);
    m.hlc_since.swap(0, 1);
    m.local.swap(0, 1);
    for k in &mut m.kept_keys {
        k.deleted_in = k.deleted_in.other();
    }
    m.kept_keys.sort();
    m
}

fn kind_of(m: &Memory) -> Option<&str> {
    m.metadata.get("_mnemo.kind").and_then(Value::as_str)
}

/// Every fact's (id, content) from `input` is in `out`, as the record or as
/// a conflict copy, unless a delete names it.
fn covered(input: &Input, out: &[Memory], removed: &BTreeSet<Ulid>) -> Result<(), String> {
    let by_id: BTreeMap<Ulid, &Memory> = out.iter().map(|m| (m.id, m)).collect();
    for r in &input.0 {
        if kind_of(r).is_some() || removed.contains(&r.id) {
            continue;
        }
        let version = (
            r.metadata.get("_mnemo.hlc").cloned(),
            r.metadata.get("_mnemo.ver").cloned(),
        );
        let descends = by_id.get(&r.id).is_some_and(|m| {
            m.metadata
                .get("_mnemo.hlc_prev")
                .and_then(Value::as_array)
                .is_some_and(|p| {
                    p.iter().any(|x| {
                        let x = x.as_array();
                        version.0.is_some()
                            && x.map(|x| (x.first().cloned(), x.get(1).cloned()))
                                == Some((version.0.clone(), version.1.clone()))
                    })
                })
        });
        if descends {
            continue;
        }
        let as_record = by_id.get(&r.id).is_some_and(|m| m.content == r.content);
        let as_copy = out.iter().any(|m| {
            m.metadata
                .get("_mnemo.merge_source")
                .and_then(Value::as_str)
                == Some(&r.id.to_string())
                && m.content == r.content
        });
        if !as_record && !as_copy {
            return Err(format!("{} {:?} is lost", r.id, r.content));
        }
    }
    Ok(())
}

/// Nothing a live tombstone names is in the output, under its id or as a
/// conflict copy of it.
fn nothing_deleted_survives(inputs: &[&Input], out: &[Memory]) -> Result<(), String> {
    for i in inputs {
        let live: BTreeSet<Ulid> = i.0.iter().map(|m| m.id).collect();
        let mut named: BTreeSet<String> =
            i.0.iter()
                .filter(|m| kind_of(m) == Some("tombstone"))
                .filter_map(|m| {
                    m.metadata
                        .get("_mnemo.tombstone_of")
                        .and_then(Value::as_str)
                })
                .filter(|x| !live.iter().any(|l| l.to_string() == *x))
                .map(str::to_string)
                .collect();
        named.extend(
            i.1.iter()
                .filter(|d| !live.contains(d))
                .map(|d| d.to_string()),
        );
        for m in out {
            let src = m
                .metadata
                .get("_mnemo.merge_source")
                .and_then(Value::as_str);
            if named.contains(&m.id.to_string()) || src.is_some_and(|s| named.contains(s)) {
                return Err(format!("{} survived its delete", m.id));
            }
        }
    }
    Ok(())
}

/// The records that differ, for a short failure message.
fn diff(l: &[Value], r: &[Value]) -> String {
    let key = |v: &Value| v["id"].as_str().unwrap_or("").to_string();
    let lm: BTreeMap<String, &Value> = l.iter().map(|v| (key(v), v)).collect();
    let rm: BTreeMap<String, &Value> = r.iter().map(|v| (key(v), v)).collect();
    let mut out = String::new();
    for k in lm.keys().chain(rm.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (lm.get(k), rm.get(k));
        if a != b {
            let short = |v: Option<&&Value>| {
                v.map_or("absent".to_string(), |v| {
                    let mut v = (*v).clone();
                    v.as_object_mut().unwrap().remove("vector");
                    v.to_string()
                })
            };
            out += &format!("\n{k}\n  left:  {}\n  right: {}", short(a), short(b));
        }
    }
    out
}

fn as_input(recs: Vec<Memory>) -> Input {
    (recs, Vec::new())
}

/// merge(a,b) = merge(b,a) with the report mirrored, the idempotence laws,
/// and coverage, on two copies.
/// An older build writes a key this build wrote in the same file while
/// that file's clock ran ahead: this build's stamps are then ahead of the
/// older build's write, which has none, so no rule can see that it came
/// after (decision.v0210-5f-older-builds). The laws skip such a history, as
/// the copy-of-a-copy law skips older deletes.
fn older_key_write_after_an_ahead_one(history: &[(&[Op], i64)]) -> bool {
    let mut ahead = BTreeSet::new();
    for (ops, clock) in history {
        for o in *ops {
            match o {
                Op::Keyed(k, _) if *clock > 0 => {
                    ahead.insert(*k);
                }
                Op::OlderKeyed(k, _) if ahead.contains(k) => return true,
                _ => {}
            }
        }
    }
    false
}

fn two_copies(ops_a: &[Op], ops_b: &[Op], ca: i64, cb: i64) -> Result<(), TestCaseError> {
    if older_key_write_after_an_ahead_one(&[(ops_a, ca)])
        || older_key_write_after_an_ahead_one(&[(ops_b, cb)])
    {
        return Ok(());
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("base.mnemo");
    base(&p);
    let (pa, pb) = (dir.path().join("a.mnemo"), dir.path().join("b.mnemo"));
    std::fs::copy(&p, &pa).unwrap();
    std::fs::copy(&p, &pb).unwrap();
    let t = wall_ms();
    let a = diverge(&pa, ops_a, t, ca, "worker-a");
    let b = diverge(&pb, ops_b, t, cb, "worker-b");
    let now = wall_ms();

    let (ab, r_ab) = merge(&a, &b, now);
    let (ba, r_ba) = merge(&b, &a, now);
    prop_assert!(
        norm(&ab) == norm(&ba),
        "merge(a,b) = merge(b,a):{}",
        diff(&norm(&ab), &norm(&ba))
    );
    prop_assert_eq!(mirrored(&r_ab), r_ba, "the report, mirrored");

    let out = as_input(ab.clone());
    for (name, x) in [("merge(merge(a,b),b)", &b), ("merge(merge(a,b),a)", &a)] {
        let again = norm(&merge(&out, x, now).0);
        prop_assert!(again == norm(&ab), "{}:{}", name, diff(&norm(&ab), &again));
    }
    // merge(x,x) = x, but for the names of key versions an older build
    // marked superseded in place (RB-020.4): the first merge names them,
    // and after that merging again changes nothing.
    for x in [&a, &b] {
        let lone = (x.0.clone(), Vec::new());
        let xx = merge(&lone, &lone, now).0;
        let (l, r) = (unnamed(&norm(&x.0)), unnamed(&norm(&xx)));
        prop_assert!(l == r, "merge(x,x) = x:{}", diff(&l, &r));
        let again = (xx.clone(), Vec::new());
        let (l, r) = (norm(&xx), norm(&merge(&again, &again, now).0));
        prop_assert!(l == r, "merge(xx,xx) = xx:{}", diff(&l, &r));
    }

    let removed: BTreeSet<Ulid> = r_ab.removed.iter().copied().collect();
    prop_assert!(
        covered(&a, &ab, &removed).is_ok(),
        "{:?}",
        covered(&a, &ab, &removed)
    );
    prop_assert!(
        covered(&b, &ab, &removed).is_ok(),
        "{:?}",
        covered(&b, &ab, &removed)
    );
    prop_assert!(
        nothing_deleted_survives(&[&a, &b], &ab).is_ok(),
        "{:?}",
        nothing_deleted_survives(&[&a, &b], &ab)
    );
    Ok(())
}

/// merge(merge(a,b),c) = merge(a,merge(b,c)) on three copies.
fn three_copies(ops: [&[Op]; 3], clocks: [i64; 3]) -> Result<(), TestCaseError> {
    if (0..3).any(|i| older_key_write_after_an_ahead_one(&[(ops[i], clocks[i])])) {
        return Ok(());
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("base.mnemo");
    base(&p);
    let paths: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|n| dir.path().join(format!("{n}.mnemo")))
        .collect();
    for q in &paths {
        std::fs::copy(&p, q).unwrap();
    }
    let t = wall_ms();
    let a = diverge(&paths[0], ops[0], t, clocks[0], "worker-a");
    let b = diverge(&paths[1], ops[1], t, clocks[1], "worker-b");
    let c = diverge(&paths[2], ops[2], t, clocks[2], "worker-c");
    let now = wall_ms();
    let ab_c = merge(&as_input(merge(&a, &b, now).0), &c, now).0;
    let a_bc = merge(&a, &as_input(merge(&b, &c, now).0), now).0;
    prop_assert!(
        norm(&ab_c) == norm(&a_bc),
        "merge(merge(a,b),c) = merge(a,merge(b,c)):{}",
        diff(&norm(&ab_c), &norm(&a_bc))
    );
    prop_assert!(
        nothing_deleted_survives(&[&a, &b, &c], &ab_c).is_ok(),
        "{:?}",
        nothing_deleted_survives(&[&a, &b, &c], &ab_c)
    );
    Ok(())
}

/// decision.v0210-conflict-flags: a fourth copy made from the first after
/// its ops (a copy of a copy, whose edits descend from the first's), merged
/// in four groupings.
fn a_copy_of_a_copy(ops: [&[Op]; 4], clocks: [i64; 4]) -> Result<(), TestCaseError> {
    // d is a copy of a after a's writes.
    if (0..3).any(|i| older_key_write_after_an_ahead_one(&[(ops[i], clocks[i])]))
        || older_key_write_after_an_ahead_one(&[(ops[0], clocks[0]), (ops[3], clocks[3])])
    {
        return Ok(());
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("base.mnemo");
    base(&p);
    let paths: Vec<_> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| dir.path().join(format!("{n}.mnemo")))
        .collect();
    for q in &paths[..3] {
        std::fs::copy(&p, q).unwrap();
    }
    let t = wall_ms();
    let a = diverge(&paths[0], ops[0], t, clocks[0], "worker-a");
    std::fs::copy(&paths[0], &paths[3]).unwrap();
    let b = diverge(&paths[1], ops[1], t, clocks[1], "worker-b");
    let c = diverge(&paths[2], ops[2], t, clocks[2], "worker-c");
    let d = diverge(&paths[3], ops[3], t, clocks[3], "worker-d");
    let now = wall_ms();
    let m = |x: &Input, y: &Input| as_input(merge(x, y, now).0);
    let groupings = [
        ("((a+b)+c)+d", m(&m(&m(&a, &b), &c), &d)),
        ("a+(b+(c+d))", m(&a, &m(&b, &m(&c, &d)))),
        ("(a+d)+(b+c)", m(&m(&a, &d), &m(&b, &c))),
        ("(d+c)+(b+a)", m(&m(&d, &c), &m(&b, &a))),
    ];
    let first = norm(&groupings[0].1 .0);
    for (name, g) in &groupings[1..] {
        prop_assert!(
            norm(&g.0) == first,
            "{} = {}:{}",
            groupings[0].0,
            name,
            diff(&first, &norm(&g.0))
        );
    }
    // d descends from a: merging a with it changes nothing, but for the
    // tombstones a merge writes for d's deleted rows (RB-020.2), as it
    // does merging d with itself. Not when a or d deleted as an older
    // build does: a deleted row goes from the file once a quarter of the
    // rows are deleted, and nothing can know that delete then.
    if !ops[0]
        .iter()
        .chain(ops[3])
        .any(|o| matches!(o, Op::OlderForget(_)))
    {
        let dd = norm(&merge(&d, &d, now).0);
        for (name, g) in [("a+d", m(&a, &d)), ("d+a", m(&d, &a))] {
            prop_assert!(norm(&g.0) == dd, "{} = d:{}", name, diff(&dd, &norm(&g.0)));
        }
    }
    prop_assert!(
        nothing_deleted_survives(&[&a, &b, &c, &d], &groupings[0].1 .0).is_ok(),
        "{:?}",
        nothing_deleted_survives(&[&a, &b, &c, &d], &groupings[0].1 .0)
    );
    Ok(())
}

/// Records as JSON without their vectors (a file's fp16 column reads them
/// back rounded).
fn norm_no_vectors(recs: &[Memory]) -> Vec<Value> {
    norm(recs)
        .into_iter()
        .map(|mut j| {
            j.as_object_mut().unwrap().remove("vector");
            j
        })
        .collect()
}

/// RB-020.4: two copies merged through `merge_files` (the file a user
/// gets, reopened), then written on (edits and forgets of the records that
/// won and of their conflict copies among them) before the next merge. The
/// file holds what the rules gave; merging it with itself or with either
/// input gives it back, as a merge writes it (its deleted rows as
/// tombstones).
fn writes_on_an_output(ops: [&[Op]; 3], clocks: [i64; 2]) -> Result<(), TestCaseError> {
    if (0..2).any(|i| older_key_write_after_an_ahead_one(&[(ops[i], clocks[i])])) {
        return Ok(());
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("base.mnemo");
    base(&p);
    let (pa, pb, px) = (
        dir.path().join("a.mnemo"),
        dir.path().join("b.mnemo"),
        dir.path().join("x.mnemo"),
    );
    std::fs::copy(&p, &pa).unwrap();
    std::fs::copy(&p, &pb).unwrap();
    let t = wall_ms();
    let a = diverge(&pa, ops[0], t, clocks[0], "worker-a");
    let b = diverge(&pb, ops[1], t, clocks[1], "worker-b");
    let now = wall_ms();
    Mnemo::merge_files(
        &pa,
        MergeSecret::Passphrase(PW),
        &pb,
        MergeSecret::Passphrase(PW),
        &px,
        &MergeOptions::at(now),
    )
    .unwrap();
    let read = Mnemo::open_read_only(&px, PW)
        .unwrap()
        .__merge_input_for_testing()
        .unwrap();
    let rules = merge(&a, &b, now).0;
    prop_assert!(
        norm_no_vectors(&read.0) == norm_no_vectors(&rules),
        "the file holds what the rules gave:{}",
        diff(&norm_no_vectors(&rules), &norm_no_vectors(&read.0))
    );
    // This build writes on the output. (An older build's delete leaves a
    // deleted row, which a commit drops once a quarter of the rows are
    // deleted; a later merge can't know it then.)
    let ops_x: Vec<Op> = ops[2]
        .iter()
        .filter(|o| {
            !matches!(
                o,
                Op::OlderRemember(_) | Op::OlderKeyed(..) | Op::OlderForget(_)
            )
        })
        .cloned()
        .collect();
    let x = diverge(&px, &ops_x, wall_ms(), 0, "worker-x");
    let now = wall_ms() + 1;
    let xx = norm(&merge(&x, &x, now).0);
    // merge(x,x) = x, but for the conflict copies a write on x settled
    // (its record edited to the copy's text, RB-020.3): they go, and the
    // report lists them.
    let lone = (x.0.clone(), Vec::new());
    let (again, r_again) = merge(&lone, &lone, now);
    let again = norm(&again);
    let settled: BTreeSet<Ulid> = r_again.settled.iter().copied().collect();
    let kept: Vec<Memory> =
        x.0.iter()
            .filter(|m| !settled.contains(&m.id))
            .cloned()
            .collect();
    prop_assert!(
        again == norm(&kept),
        "merge(x,x) = x:{}",
        diff(&norm(&kept), &again)
    );
    for (name, (l, r)) in [
        ("merge(x,a)", (&x, &a)),
        ("merge(a,x)", (&a, &x)),
        ("merge(x,b)", (&x, &b)),
        ("merge(b,x)", (&b, &x)),
    ] {
        let got = norm(&merge(l, r, now).0);
        prop_assert!(got == xx, "{} = x:{}", name, diff(&xx, &got));
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    #[test]
    fn prop_two_copies(
        ops_a in prop::collection::vec(op(), 0..10),
        ops_b in prop::collection::vec(op(), 0..10),
        ca in clock(),
        cb in clock(),
    ) {
        two_copies(&ops_a, &ops_b, ca, cb)?;
    }

    #[test]
    fn prop_three_copies(
        ops_a in prop::collection::vec(op(), 0..8),
        ops_b in prop::collection::vec(op(), 0..8),
        ops_c in prop::collection::vec(op(), 0..8),
        ca in clock(),
        cb in clock(),
        cc in clock(),
    ) {
        three_copies([&ops_a, &ops_b, &ops_c], [ca, cb, cc])?;
    }

    #[test]
    fn prop_a_copy_of_a_copy(
        ops_a in prop::collection::vec(op(), 0..6),
        ops_b in prop::collection::vec(op(), 0..6),
        ops_c in prop::collection::vec(op(), 0..6),
        ops_d in prop::collection::vec(op(), 0..6),
        ca in clock(),
        cb in clock(),
        cc in clock(),
        cd in clock(),
    ) {
        a_copy_of_a_copy([&ops_a, &ops_b, &ops_c, &ops_d], [ca, cb, cc, cd])?;
    }

    #[test]
    fn prop_writes_on_an_output(
        ops_a in prop::collection::vec(op(), 0..6),
        ops_b in prop::collection::vec(op(), 0..6),
        ops_x in prop::collection::vec(op(), 1..6),
        ca in clock(),
        cb in clock(),
    ) {
        writes_on_an_output([&ops_a, &ops_b, &ops_x], [ca, cb])?;
    }
}

// The cases proptest saved in `proptest_merge.proptest-regressions`, as
// they shrank. A change to `op()` makes a saved seed replay another case
// (v0210-4b added two ops), so each one is kept here as its inputs; the
// comment names its seed's first 6 hex digits.

/// A saved case: its seed's first 6 hex digits, each copy's ops and clock.
type Saved<const N: usize> = (&'static str, [Vec<Op>; N], [i64; N]);

#[test]
fn saved_cases_of_two_copies() {
    use Op::*;
    let cases: [Saved<2>; 6] = [
        (
            "2217d7",
            [vec![TaskTake, TaskReady], vec![TaskTake]],
            [HOUR, HOUR],
        ),
        (
            "a3c0f9",
            [vec![Keyed(0, 0), ForgetKeyed(0)], vec![]],
            [0, 0],
        ),
        (
            "ed02c2",
            [vec![Keyed(0, 0), ForgetKeyed(0), Keyed(0, 0)], vec![]],
            [0, 0],
        ),
        (
            "364a48",
            [
                vec![Keyed(1, 0), ForgetKeyed(0)],
                vec![Keyed(0, 0), Keyed(1, 0), ForgetKeyed(1)],
            ],
            [0, 0],
        ),
        ("fd93c0", [vec![Keyed(0, 0)], vec![Keyed(0, 0)]], [0, 0]),
        (
            "ccd5d7",
            [vec![Keyed(0, 0), ForgetKeyed(0), OlderKeyed(0, 21)], vec![]],
            [0, 0],
        ),
    ];
    for (seed, [a, b], [ca, cb]) in cases {
        two_copies(&a, &b, ca, cb).unwrap_or_else(|e| panic!("{seed}: {e}"));
    }
}

#[test]
fn saved_cases_of_three_copies() {
    use Op::*;
    let cases: [Saved<3>; 8] = [
        (
            "3ec93e",
            [vec![Keyed(0, 0)], vec![Keyed(0, 0)], vec![]],
            [0, 0, 0],
        ),
        (
            "9072db",
            [
                vec![],
                vec![EssenceEdit(2)],
                vec![EssenceEdit(2), EssenceResolve],
            ],
            [0, 0, 0],
        ),
        (
            "0815b7",
            [vec![Edit(190, 0)], vec![Edit(130, 0)], vec![Edit(64, 1)]],
            [1, -HOUR, 0],
        ),
        (
            "083bdc",
            [
                vec![EssenceEdit(0)],
                vec![EssenceEdit(2)],
                vec![EssenceResolve, EssenceEdit(2)],
            ],
            [0, 0, 0],
        ),
        (
            "d05bcb",
            [vec![TaskTake], vec![], vec![TaskTake, TaskReady]],
            [0, 0, 0],
        ),
        (
            "0344e8",
            [
                vec![EssenceEdit(0)],
                vec![EssenceEdit(1)],
                vec![EssenceEdit(0)],
            ],
            [0, 0, HOUR],
        ),
        (
            "32adae",
            [
                vec![ForgetKeyed(0)],
                vec![Keyed(0, 0), ForgetKeyed(0)],
                vec![Keyed(0, 0)],
            ],
            [0, 0, 0],
        ),
        (
            "f787fd",
            [
                vec![ThreadForget, AckAll, EssenceEdit(3), TaskReady, Post(2)],
                vec![EssenceResolve, Post(0), TaskReady, Forget(252)],
                vec![EssenceEdit(3), Remember(175)],
            ],
            [HOUR, HOUR, HOUR],
        ),
    ];
    for (seed, [a, b, c], clocks) in cases {
        three_copies([&a, &b, &c], clocks).unwrap_or_else(|e| panic!("{seed}: {e}"));
    }
}

#[test]
fn saved_cases_of_a_copy_of_a_copy() {
    use Op::*;
    let cases: [Saved<4>; 18] = [
        (
            "112dca",
            [
                vec![Keyed(0, 0)],
                vec![],
                vec![OlderKeyed(0, 0)],
                vec![OlderKeyed(0, 0)],
            ],
            [-HOUR, 0, 0, 0],
        ),
        (
            "bbed7a",
            [
                vec![OlderKeyed(0, 0)],
                vec![OlderKeyed(0, 0)],
                vec![],
                vec![ForgetKeyed(0)],
            ],
            [0, 0, 0, 0],
        ),
        (
            "0cdacf",
            [vec![], vec![], vec![], vec![OlderKeyed(0, 0)]],
            [0, 0, 0, 0],
        ),
        (
            "da61a6",
            [
                vec![Keyed(2, 0)],
                vec![Keyed(2, 0)],
                vec![],
                vec![ForgetKeyed(2)],
            ],
            [0, 0, 0, 0],
        ),
        (
            "a1a4b5",
            [vec![TaskTake, TaskReady], vec![TaskTake], vec![], vec![]],
            [0, 0, 0, 0],
        ),
        (
            "a70856",
            [
                vec![Keyed(0, 0)],
                vec![Keyed(0, 0)],
                vec![],
                vec![ForgetKeyed(0)],
            ],
            [-HOUR, 0, 0, 0],
        ),
        (
            "2ab898",
            [
                vec![],
                vec![Keyed(0, 0)],
                vec![Keyed(0, 0)],
                vec![Keyed(0, 0)],
            ],
            [0, 0, 0, 0],
        ),
        (
            "e61eac",
            [
                vec![Keyed(1, 0)],
                vec![],
                vec![Keyed(1, 0)],
                vec![Keyed(1, 1)],
            ],
            [0, 0, 0, 0],
        ),
        (
            "0d4571",
            [
                vec![Keyed(2, 0)],
                vec![],
                vec![Keyed(2, 1)],
                vec![ForgetKeyed(2)],
            ],
            [HOUR, 0, 0, 0],
        ),
        (
            "e424a4",
            [
                vec![Turn(2), EssenceEdit(3)],
                vec![
                    TaskReady,
                    Remember(165),
                    AckAll,
                    Remember(31),
                    Remember(233),
                ],
                vec![ForgetKeyed(1), Turn(0), ForgetKeyed(0), EssenceResolve],
                vec![EssenceEdit(0), Post(1), ForgetKeyed(1)],
            ],
            [1, 1, 0, 1],
        ),
        (
            "91d13c",
            [
                vec![Keyed(2, 0)],
                vec![],
                vec![Keyed(2, 0)],
                vec![Keyed(2, 1), ForgetKeyed(2)],
            ],
            [-HOUR, 0, 0, 0],
        ),
        (
            "8be26e",
            [vec![Turn(0), ThreadForget], vec![], vec![], vec![Turn(0)]],
            [0, 0, 0, 0],
        ),
        (
            "6e5fb2",
            [
                vec![Turn(1), ThreadForget],
                vec![Turn(1)],
                vec![],
                vec![Turn(1)],
            ],
            [0, 0, 0, 0],
        ),
        (
            "bd1071",
            [
                vec![],
                vec![
                    ThreadForget,
                    Forget(178),
                    ForgetKeyed(1),
                    EssenceResolve,
                    TaskAdd(2),
                ],
                vec![
                    ForgetKeyed(2),
                    Turn(3),
                    EssenceResolve,
                    Turn(3),
                    ThreadForget,
                ],
                vec![EssenceEdit(0), Remember(243), Turn(0)],
            ],
            [1, 1, -HOUR, 0],
        ),
        (
            "c0ae0d",
            [vec![], vec![Edit(4, 1)], vec![], vec![Edit(184, 0)]],
            [0, 0, 0, 0],
        ),
        (
            "fe20b6",
            [
                vec![OlderKeyed(0, 0)],
                vec![],
                vec![],
                vec![OlderKeyed(0, 1)],
            ],
            [0, 0, 0, 0],
        ),
        (
            "71685b",
            [vec![OlderForget(0)], vec![], vec![], vec![OlderForget(0)]],
            [0, 0, 0, 0],
        ),
        (
            "df9fb1",
            [
                vec![OlderForget(0), Forget(0)],
                vec![],
                vec![],
                vec![Forget(0)],
            ],
            [0, 0, 0, 0],
        ),
    ];
    for (seed, [a, b, c, d], clocks) in cases {
        a_copy_of_a_copy([&a, &b, &c, &d], clocks).unwrap_or_else(|e| panic!("{seed}: {e}"));
    }
}

#[test]
fn saved_cases_of_writes_on_an_output() {
    use Op::*;
    let cases: [Saved<3>; 5] = [
        (
            "39e50c",
            [
                vec![Edit(234, 3), Turn(2), TaskAdd(1)],
                vec![TaskTake, Turn(2), Keyed(0, 239), Edit(27, 1), AckAll],
                vec![Edit(100, 3)],
            ],
            [HOUR, HOUR, 0],
        ),
        (
            "8fed47",
            [vec![], vec![], vec![OlderForget(0), OlderForget(0)]],
            [0, 0, 0],
        ),
        (
            "df536c",
            [
                vec![Keyed(1, 0)],
                vec![OlderKeyed(1, 0)],
                vec![ForgetKeyed(1)],
            ],
            [0, 0, 0],
        ),
        (
            "d38297",
            [
                vec![EssenceEdit(0)],
                vec![EssenceEdit(1)],
                vec![EssenceEdit(1)],
            ],
            [HOUR, 0, 0],
        ),
        (
            "f324e3",
            [
                vec![OlderKeyed(0, 0)],
                vec![OlderKeyed(0, 0)],
                vec![Keyed(0, 0)],
            ],
            [0, 0, 0],
        ),
    ];
    for (seed, [a, b, x], [ca, cb, _]) in cases {
        writes_on_an_output([&a, &b, &x], [ca, cb]).unwrap_or_else(|e| panic!("{seed}: {e}"));
    }
}
