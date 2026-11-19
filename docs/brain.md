# mnemo export

The brain's backup, written by mnemo 0.19.1 with `--git` to be committed beside the project, from a `.mnemo` file (format v9, 384 dimensions). It's plaintext: read the diff before committing.

Keyed memories by key, then tasks, findings and the project essence, then the rest by id. Left out: vectors, thread turns, thread essence, messages, claims, presence, private records, access stats, and security findings until they're verified against a release.

## 01M7NBTNEEDS2DJAJ5G0X4FPXS

- type: semantic
- agent: `cursor-agent`
- key: `architecture.export-git-order`
- scope: private
- created: 2026-11-17T18:20:39Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1794944263,"_mnemo.key":"architecture.export-git-order","_mnemo.superseded_by":"01M7NG7S71127YMD0F0AB48RH8","area":"architecture","topic":"export"}`

```text
mnemo export --git order (v0.18.0, PR 126 d4f95e7, bin/mnemo/export.rs select): keyed memories by key, then tasks by slug, then findings by Finding::sort_key (RB-9.2 < RB-9.10 < RB-10.1, every status, no age limit), then project essence (open items by section intent, conventions, decisions, state, open, next, lessons, timeline; then resolved), then the rest by id. Left out with --git: messages, claims, presence, saved turns, private records, access stats, tasks done over 30 days ago (done_long_ago), project essence resolved over 30 days ago (resolved_long_ago). Thread essence is NOT left out; it sits in the rest by id.
```

## 01M7NG7S71127YMD0F0AB48RH8

- type: semantic
- agent: `cursor-agent`
- key: `architecture.export-git-order`
- scope: private
- created: 2026-11-17T19:37:43Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"architecture.export-git-order","_mnemo.supersedes":"01M7NBTNEEDS2DJAJ5G0X4FPXS","area":"architecture","topic":"export"}`

```text
mnemo export --git order (v0.18.0, PRs 126 d4f95e7 and 127 635ae00, bin/mnemo/export.rs select): keyed memories by key, then tasks by slug, then findings by Finding::sort_key (RB-9.2 < RB-9.10 < RB-10.1), then project essence (open items by section intent, conventions, decisions, state, open, next, lessons, timeline; then resolved), then the rest by id. Left out with --git: messages, claims, presence, saved turns, private records, access stats, thread essence (kept_out_of_git), SECURITY findings at any status but verified (an unfixed security finding never reaches the public docs/brain.md), tasks done over 30 days ago, project essence resolved over 30 days ago. The plain export keeps thread essence and every finding.
```

## 01M7NT5YJQCT9S5JJ2SMR7TV14

- type: semantic
- agent: `cursor-agent`
- key: `architecture.findings`
- scope: private
- created: 2026-11-17T22:31:29Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"architecture.findings","area":"architecture","topic":"findings"}`

```text
Review findings (v0.18.0, store/findings.rs): _mnemo.kind=finding, a coordination record (sentinel vector, never a recall hit), id ^[A-Z]+-[0-9]+\.[0-9]+$ unique by prefix and numbers (RB-0900.1 = RB-900.1), batch = id prefix. Transitions in FINDING_TRANSITIONS; finding_triage/finding_verify write status_history. Since round 1 (PR 129, 61bb573): adding needs an author; update/update_text, delete (MCP forget, Python, mobile) and essence_remove refuse a finding's record (a wrong one is rejected); finding JSON carries record_id; a SECURITY finding is stored _mnemo.visibility=private until verified, and verified only against a release tag ^v\d+\.\d+\.\d+$ (then team). The finding tools read through coordination_records, which ignores visibility.
```

## 01M7PTYR4MZP0YYSD16S6KPHKZ

- type: semantic
- agent: `cursor-agent`
- key: `architecture.index-layout-2`
- scope: private
- created: 2026-11-18T08:04:16Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"architecture.index-layout-2","area":"architecture","topic":"ann-index"}`

```text
ANN index layout 2 (v0.19.0, PERF-2, PR 136 8a63540): the v9 index run's layout byte is 2 (V9_INDEX_LAYOUT); the body is flat little-endian partition-major arrays (IvfPqIndex::write_layout2/read_layout2: u32 dims, partitions, n_probe, n_rerank, m; subspace offsets; codeword counts; f32 centroids; f32 codebooks; u32 per-partition counts; u128 ids; u8 codes), every length checked before allocation. Layout 1 (msgpack, V9_INDEX_LAYOUT_MSGPACK) is still read and rewritten as 2 with the next base. v0.18 and older read layout 2 as no index (exact recall, no error). The id->partition map is built lazily (ensure_assignment) only for writes. No format bump.
```

## 01M7P24YERKT0YD8B0D3W0GX8W

- type: semantic
- agent: `cursor-agent`
- key: `architecture.inspect-coord`
- scope: private
- created: 2026-11-18T00:50:44Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"architecture.inspect-coord","area":"architecture","topic":"inspector"}`

```text
mnemo inspect Findings and Tasks tabs (v0.18.1, PRs 130 4c696f0 and 131): /api/findings (status, category, priority filters; counts over all; sorted by Finding::sort_key) and /api/tasks (status filter; the file's catalog order, as task list) built at load from the records already read via Finding::from_memory/Task::from_memory. Never sort coordination records by ULID for display: ids from one millisecond aren't ordered. Page text goes through el() (text nodes only); the page's JS can be syntax-checked with osascript -l JavaScript (no Node on this Mac).
```

## 01M7K1HJB5XRYWCRMJ9H63RZQP

- type: semantic
- agent: `cursor-agent`
- key: `architecture.keychain-reprompt`
- scope: private
- created: 2026-11-16T20:42:26Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"architecture.keychain-reprompt","area":"architecture","topic":"keychain-reprompt"}`

```text
KEYCHAIN-REPROMPT (2026-11-16): macOS ties a keychain item to the binary that reads it, and release binaries are ad-hoc signed, so each upgrade re-asks once; Always Allow lasts until the next upgrade. An open dialog blocks the read (no error); a refusal is a store error naming '(reading)'; a missing item is Resolution::NoSource. Since v0.17.1 the hook and the MCP server name a refusal with the Always Allow hint, and keys::SlowStoreNote says 'still waiting for the keychain' on stderr after 3 s. The lasting fix (a Developer ID) is the user's decision.
```

## 01M7K1HHYWP8TR0WP87FSZAVVP

- type: semantic
- agent: `cursor-agent`
- key: `architecture.onboard-views`
- scope: private
- created: 2026-11-16T20:42:26Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1794875186,"_mnemo.key":"architecture.onboard-views","_mnemo.superseded_by":"01M7KEBQBJG81AM75QYY8C1F0A","area":"architecture","topic":"onboard-views"}`

```text
ONBOARD-UX (v0.17.1, 2026-11-16): mnemo onboard has two views. At a terminal (stdout a TTY; the dev-hooks switch MNEMO_DEV_ONBOARD_TTY in tests) it groups the plan under Brain, Model, Claude Code, Cursor and Git with marks + ~ - = !, collapses what's already right into one line per group, then prints Done (one line per file), the guide's pointer and Next (restart, then human steps). With no terminal the output is v0.16.1's byte for byte (checked against the pre-PR build in 6 cases), since agents read it. Each plan Item carries its own tty lines.
```

## 01M7KEBQBJG81AM75QYY8C1F0A

- type: semantic
- agent: `cursor-agent`
- key: `architecture.onboard-views`
- scope: private
- created: 2026-11-17T00:26:26Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"architecture.onboard-views","_mnemo.supersedes":"01M7K1HHYWP8TR0WP87FSZAVVP","area":"architecture","topic":"onboard-views"}`

```text
ONBOARD-UX (v0.17.1, 2026-11-16, the user's choices): mnemo onboard has two views. At a terminal (stdout a TTY; MNEMO_DEV_ONBOARD_TTY in tests) each item is one line tagged as a boot log tags its lines (SysV init, systemd): [  OK   ], [ ADDED ], [UPDATED], [REMOVED], [ WARN  ], [FAILED ], [SKIPPED], then the part (Brain, Model, Claude Code, Cursor, Git); only the word is coloured. Tags are past-tense results, printed after the writes or when nothing needs writing; before a write the plan is untagged in the future tense ('Will add: ...'), then the question. apply() records each item's Outcome (Done, Failed, NotRun). After the lines: 'Done: N files changed.', 'Next:', the guide's pointer. With no terminal the output is v0.16.1's byte for byte, since agents read it.
```

## 01M7PTYRH9DG1W9XETQPV981X7

- type: semantic
- agent: `cursor-agent`
- key: `architecture.page-batch-reads`
- scope: private
- created: 2026-11-18T08:04:16Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"architecture.page-batch-reads","area":"architecture","topic":"pager"}`

```text
Pager batch reads (v0.19.0, PERF-2): read_pages_uncached(&[(page, gen)]) -> PageBatch does one positioned read per run of adjacent pages and AES-GCM decrypts in place (crypto::aead_decrypt_in_place) on scoped threads from PARALLEL_DECRYPT_MIN_PAGES (64, chosen not measured); RunReader refills 256 pages and reads PageBatch::into_images as a plain slice (a per-page lookup on that hot path cost ~10 ms of open at 100K). for_each_page streams a fill: threads started once, 15-page (120 KiB) buffers over a bounded channel, so the macOS allocator doesn't keep 2 MiB buffers (it did: +2.4 to 3.7 MB on a 10K handle). MNEMO_DEV_DECRYPT_THREADS (dev-hooks) caps threads for single-thread numbers.
```

## 01M7HTAC00BQ1BCWRQ09FV7SW8

- type: semantic
- agent: `cursor-agent`
- key: `architecture.row-flags`
- scope: private
- created: 2026-11-16T09:16:56Z
- importance: 0.7
- metadata: `{"_mnemo.key":"architecture.row-flags","area":"architecture","topic":"row-flags"}`

```text
Row flags of a v9 catalog row (since v0.17.0): ROW_SUPERSEDED 1, ROW_HAS_TTL 2, ROW_SHARED 4, ROW_SETTLED 8 (turns only), ROW_PUBLIC 16, ROW_REVIEW 32, ROW_PRIVATE 64 (moved from 8 before the tag, RB-013.3). One bit each, in one block in store/records.rs; row_visible(kind, flags, set) never reads a turn as private.
```

## 01M7P24Y2V9FWWM0YTGAM9XN06

- type: semantic
- agent: `cursor-agent`
- key: `architecture.task-refs`
- scope: private
- created: 2026-11-18T00:50:44Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"architecture.task-refs","area":"architecture","topic":"tasks"}`

```text
Task refs (v0.18.1, PR 131 98c2a68): TaskSpec.refs/Task.refs stored under _mnemo.refs, written only when non-empty (so re-adding an older task as it was stays unchanged); at most 32, one line, <=200 bytes. CLI task add --ref (repeatable), MCP task refs, Python task_add(refs=). export --git leaves out a task whose refs name an unverified SECURITY finding, matched by prefix and numbers; the CLI computes export::held_findings from every record before the visibility filter (those findings are private) and passes ExportOptions.held to select.
```

## 01M7FBQGKHRBEFAA0J1M60XAFN

- type: episodic
- agent: `cursor-agent`
- key: `bug.keyed-identical-f16`
- scope: private
- created: 2026-11-15T10:23:29Z
- importance: 0.8
- metadata: `{"_mnemo.invalid_at":1794759132,"_mnemo.key":"bug.keyed-identical-f16","_mnemo.superseded_by":"01M7FZP0Z91EZVH3S99XER9D25","area":"decisions"}`

```text
Found 2026-11-15 (v0.16.0 step 7a): on a v9 file an identical keyed re-write returns 'superseded', not 'unchanged', with any real embedder, from the CLI and from Python. v9 stores vectors as f16 in the vector column (FMT-9b, veccol::quantize), read_memory returns the dequantized vector, and Mnemo::is_identical_keyed_write (store/keyed.rs) compares it exactly with the incoming f32 vector, so only vectors already exact in f16 (tests' small integers) match. Effect: every re-write of the same fact adds a history version, against rule 2 and the guide's 'writing the same value again changes nothing'. Proposed fix: compare both vectors after the same f16 round trip (veccol::quantize then dequantize), with a test using a real-valued vector. Reported to PLANNER-A in step 7a's DONE; not fixed yet.
```

## 01M7FZP0Z91EZVH3S99XER9D25

- type: episodic
- agent: `cursor-agent`
- key: `bug.keyed-identical-f16`
- scope: private
- created: 2026-11-15T16:12:12Z
- importance: 0.8
- metadata: `{"_mnemo.key":"bug.keyed-identical-f16","_mnemo.supersedes":"01M7FBQGKHRBEFAA0J1M60XAFN","area":"findings"}`

```text
Fixed in v0.16.0 (PR 99, rel0160-rb011): an identical keyed re-write on a v9 file returned 'superseded' with any real embedder since v0.13, because the live vector is read back fp16-rounded from the vector column and was compared with the fresh f32 one. is_identical_keyed_write (store/keyed.rs) now takes `rounded` (a v9 entry in a file without float vectors) and compares dequantize(quantize(incoming)) with the live vector. Tests: tests/keyed_identical.rs (library and CLI; both fail on the old code) and test_identical_keyed_rewrite_is_unchanged in test_mnemo.py. The extra versions already in the dogfood brain stay as history.
```

## 01M7G76NN4G0B7PVB2RDCRW2WJ

- type: episodic
- agent: `cursor-agent`
- key: `bug.marker-only-turns`
- scope: private
- created: 2026-11-15T18:23:37Z
- importance: 0.8
- metadata: `{"_mnemo.key":"bug.marker-only-turns","area":"findings"}`

```text
Found in the v0.16.0 by-hand run, fixed in v0.16.1 (PR 100): transcript turns whose content is only markers ([thinking], [tool_use name=...], [tool_result ok, N bytes], [image]) were embedded and matched any query weakly (sim 0.49-0.50), so 6 of a turn-filtered recall's top 10 were markers. Fix: transcript::is_marker_only; plan_ingest leaves them out of the texts to embed and writes them with the sentinel vector and _mnemo.markers_only: true (kept for thread export and the turn_id dedupe); recall drops any marker-only turn by flag or content, so v0.16.0's saved turns are left out too; Mnemo::ingest_thread_new_turns counts new turns including marker-only ones, used by the hook's batch scan so an all-marker tail is still saved.
```

## 01M7NG7SM3T9VJZZ66KH3K96D4

- type: semantic
- agent: `cursor-agent`
- key: `ci.macos-runner-capacity`
- scope: private
- created: 2026-11-17T19:37:43Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"ci.macos-runner-capacity","area":"ci","topic":"flakes"}`

```text
A CI job shown as failed/cancelled with no steps (often rust (macos-latest) or mobile (aarch64-apple-ios)) is usually GitHub's macOS arm64 runner capacity: the annotation says 'The job was not acquired by Runner of type hosted even after multiple attempts'. Read it with gh api repos/toarchkumar/mnemo/check-runs/<job id>/annotations; re-run with gh run rerun <run> --failed. Seen 3 times on 2026-11-17 (52feb44, d4f95e7, PR 128); none was a code failure.
```

## 01M7P5H7GGAN1ZAZ7GX2SGY0YN

- type: procedural
- agent: `cursor-agent`
- key: `ci.mobile-concurrency-rerun`
- scope: private
- created: 2026-11-18T01:49:53Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"ci.mobile-concurrency-rerun","area":"ci","topic":"mobile"}`

```text
Mobile CI on a release branch uses one concurrency group per ref (mobile-ci-refs/heads/<branch>), which cancels the in-progress run. Re-running an older commit's Mobile CI on the branch (gh run rerun --failed) therefore cancels the head's in-progress run (annotation: 'Canceling since a higher priority waiting request ... exists'). Wait for the head's run to finish before re-running an older one (seen 2026-11-17, v0.18.1: 98c2a68 vs 72d78b5).
```

## 01M7Q1R5FB1AWK45B52YEC9ZSF

- type: episodic
- agent: `cursor-agent`
- key: `ci.shared-valve-timing`
- scope: private
- created: 2026-11-18T10:03:00Z
- importance: 0.5
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"ci.shared-valve-timing","area":"ci","topic":"flakes"}`

```text
shared_valve::two_writers_and_four_readers_all_get_their_turns (tests/shared_valve.rs:193, readers stopped getting in) failed once on a hosted macOS runner (PR 139, 2026-11-18): reads per 500 ms [140, 47, 0, 103, 79, 33] under GitHub macOS capacity constraints; it passed on the six runs before and on the re-run. Logged, not changed (like shared_lock and the Swift EmbedQueueTests timing); the Planner decides if it recurs.
```

## 01M7P5H7W2VWAVYRKZEBTW6R31

- type: episodic
- agent: `cursor-agent`
- key: `ci.swift-embed-queue-timing`
- scope: private
- created: 2026-11-18T01:49:53Z
- importance: 0.5
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"ci.swift-embed-queue-timing","area":"ci","topic":"flakes"}`

```text
Swift timing test EmbedQueueTests.slowEmbedsDontStarveTheAppsOtherTasks (EmbedQueueTests.swift:45, sinceLaunch < 1.2 s) failed once on a hosted macOS runner at 1.449 s (Mobile CI on 98c2a68, 2026-11-17); its re-run passed, and the next run measured 1.999 s for the whole test. Logged, not changed (one failure isn't a reason to loosen a test); the Planner decides if it recurs.
```

## 01M7PX5ZTQBZEFZ7XGMQ87C88X

- type: semantic
- agent: `cursor-agent`
- key: `convention.non-exhaustive-types`
- scope: private
- created: 2026-11-18T08:43:10Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"convention.non-exhaustive-types","area":"convention","topic":"rust-api"}`

```text
Rust API convention (v0.19.0, PR 137 b97e689): the 13a and findings record types (TaskSpec, Task, TaskTaken, TaskClaim, NewWork, Message, Claim, Presence, Stale, KindCounts, FindingSpec, Finding, FindingFilter) are #[non_exhaustive]. Outside the engine crate (the CLI bin, tests/, mnemo-python, users) a struct expression is refused even with ..Default::default() (E0639): build with TaskSpec::new(title), FindingSpec::new(id, category, priority, title) or Type::default(), then assign fields; destructuring needs a trailing `..`. New public record types should follow this, with Default or a constructor.
```

## 01M7GMSXQBCWKAQSMZ3CRK81VT

- type: episodic
- agent: `cursor-agent`
- key: `decision.13-1b-delivery`
- scope: private
- created: 2026-11-15T22:21:19Z
- importance: 0.8
- metadata: `{"_mnemo.key":"decision.13-1b-delivery","area":"decisions"}`

```text
2026-11-12, the user (13.1b, replacing 'push notifications are out of scope'): agents wake one another instead of the user relaying prompts. Every message and task keeps a delivery state sent -> delivered -> seen -> done, so a lost wake-up loses nothing; wake-ups go through a channel, a per-turn hook or MCP notifications; an optional mnemo watch (off by default) starts a session when work arrives; guardrails from the first version (a wake-up limit per role per hour, a cooldown, a hop cap, a kill switch); messages are information, never authority; the sender is unproven until 13b.
```

## 01M7H1YZ3YXNDK9RC73AC07YCM

- type: episodic
- agent: `cursor-agent`
- key: `decision.13a-export-coordination`
- scope: private
- created: 2026-11-16T02:11:16Z
- importance: 0.7
- metadata: `{"_mnemo.key":"decision.13a-export-coordination","area":"decision","topic":"decision.13a-export-coordination"}`

```text
2026-11-15 (13a step 4): export never writes claims or presence; export --git also leaves out messages and tasks done more than 30 days ago, orders tasks by slug after the keyed memories, and drops _mnemo.delivery, so a git export changes only with a task.
```

## 01M7GTRKWEZB9JGB14J28K3PCC

- type: episodic
- agent: `cursor-agent`
- key: `decision.13a-hop-carry`
- scope: private
- created: 2026-11-16T00:05:28Z
- importance: 0.7
- metadata: `{"_mnemo.invalid_at":1794790875,"_mnemo.key":"decision.13a-hop-carry","_mnemo.superseded_by":"01M7GXYRRZ20NX95CD13BG902Z","area":"decision","topic":"decision.13a-hop-carry"}`

```text
2026-11-15 (13a step 2): the pair carry takes the max hop count between the two parties (either direction) in the last 10 minutes, plus one, not the newest message's count: messages within one second share created_at, so 'newest' was ambiguous. A reply is one more than its conversation's highest.
```

## 01M7GXYRRZ20NX95CD13BG902Z

- type: episodic
- agent: `cursor-agent`
- key: `decision.13a-hop-carry`
- scope: private
- created: 2026-11-16T01:01:15Z
- importance: 0.7
- metadata: `{"_mnemo.key":"decision.13a-hop-carry","_mnemo.supersedes":"01M7GTRKWEZB9JGB14J28K3PCC","area":"decision","topic":"decision.13a-hop-carry"}`

```text
2026-11-15 (PLANNER-A DECISION at step 3, correcting round-1 change 4): the wake-up pair guardrail counts bursts, not chains: a message wakes nobody when its two agents exchanged 8 or more messages either way in the last 10 minutes. The per-thread hop cap of 8 stays for explicit replies (--conversation). _mnemo.hops and its carry (step 2: one above the pair's highest count in 10 min) stay stored as information; step 5 enforces. Reason: the carry kept climbing through slow legitimate exchanges (DISPATCH/DONE every 5 to 15 min).
```

## 01M7HTABMG3J4SAARA97J1KXW2

- type: episodic
- agent: `cursor-agent`
- key: `decision.13a-mixed-versions`
- scope: private
- created: 2026-11-16T09:16:55Z
- importance: 0.75
- metadata: `{"_mnemo.key":"decision.13a-mixed-versions","area":"decision","topic":"decision.13a-mixed-versions"}`

```text
2026-11-16 (RB-013.1): a v0.16 build's compact re-tags coordination rows as facts (it doesn't know row kind 5). v0.17 finds them anyway: coordination_records also takes legacy_coordination_ids, the episodic fact rows decrypted once per handle whose record is a coordination kind; a write re-tags; verify, about and the first coordination command report Mnemo::mistagged_coordination; mnemo compact re-tags the rest. CI's compat (v0.16.1, linux) job runs tests/v0_16_compat.rs against the published v0.16.1.
```

## 01M7H1YYS2GYMYVEEJMZ8PDTEF

- type: episodic
- agent: `cursor-agent`
- key: `decision.13a-race-rounds`
- scope: private
- created: 2026-11-16T02:11:16Z
- importance: 0.7
- metadata: `{"_mnemo.key":"decision.13a-race-rounds","area":"decision","topic":"decision.13a-race-rounds"}`

```text
2026-11-15 (PLANNER-A DECISION at step 3): the two-process claim race runs 200 rounds per push (about 15 s on macOS) and 12.4's 1,000 under MNEMO_STRESS=1, in fuzz-weekly.yml's claim-race job on Linux and macOS; CL_RACE_ROUNDS sets any count.
```

## 01M7GMSY39VM85EM9ZPNEHEG2B

- type: episodic
- agent: `cursor-agent`
- key: `decision.13a-scope`
- scope: private
- created: 2026-11-15T22:21:20Z
- importance: 0.8
- metadata: `{"_mnemo.key":"decision.13a-scope","area":"decisions"}`

```text
R-006.1 (REVIEWER-A, accepted modified by PLANNER-A 2026-11-11): Phase 13 splits into 13a Local Coordination (13.1 messages and tasks, 13.3 claims and presence, 13.5 visibility, 13.6 freshness), which runs on one machine under the shared key, and 13b Multi-Party Crypto (13.2 per-agent key slots, 13.4 signed authorship, 13.7 merge). In 13a, visibility is a filter, not a security boundary, until 13b. Order: 12.4 -> 12.3 -> 12.2 -> 13a -> HARNESS-DOGFOOD-1, then 13b.
```

## 01M7HDDB3Y02MAK5DB3R2S1QYW

- type: episodic
- agent: `cursor-agent`
- key: `decision.13a-seen-per-agent`
- scope: private
- created: 2026-11-16T05:31:22Z
- importance: 0.7
- metadata: `{"_mnemo.key":"decision.13a-seen-per-agent","area":"decision","topic":"decision.13a-seen-per-agent"}`

```text
2026-11-15 (RB-012.6 item 4, closing SEEN-PER-AGENT): messages to a role or all, and a role's ready tasks, are new on each agent's per-turn line until that agent was shown them (_mnemo.seen_by); direct messages keep the delivery field; NewWork carries its agent and mark_shown records it; wake() skips a task the agent has seen.
```

## 01M7SBNAYDTJHH726WDYE03506

- type: episodic
- agent: `cursor-agent`
- key: `decision.catch-up-design`
- scope: private
- created: 2026-11-19T07:34:42Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.catch-up-design"}`

```text
decision.catch-up-design (planner-a, 2026-11-18): docs/design/catch-up.md (PR 145, da96a2a) is approved for the v0200-3 build, with these additions. Checked in code: write_gen() = header.wal_seq + 1 after ensure_gen_lease bumps wal_seq (records.rs 1452, 602), so every run sealed after a reader's t has gen > t, and the walk-back stopping rule holds. Step 8: no gate and no format change (the Planner's call under spec.v0.20.0; nothing goes to the user); the release notes repeat v0.14's "update every mnemo on the machine" line. Additions for the build (the Worker adds them to the note in PR 145 before merging it, as the first commit of v0200-3): (a) equivalence cases where the reader is several commits behind, from two different writers, with a lease-only transaction between them, so the walk covers k > 1 deltas; and one where the reader is behind across a base rewrite followed by more deltas (fallback). (b) A page-cache case where a failed write on the writer's own handle, then its retry, rewrite the same page under the same generation (v0200-1r's restore path); the cache must hold the retry's bytes. (c) Footprint: fact.shared-reload-footprint is folded into this build, not a separate task. Measure the footprint over 500 one-memory commits at 100K (so about 8 fallbacks are included) and report it; the target is under +20 MB. If the fallback path alone still grows about 4.7 MB per reload with no plateau, say so in the report, and the Planner decides then whether to open a task. (d) The lazy indexes: report the first-recall cost of each one dropped (key index, superseded, layers, agent set) at 100K before choosing which to make incremental. The targets in section 8 stand.
```

## 01M7SVF0DBE4J5FRGQQPKXYCGJ

- type: episodic
- agent: `cursor-agent`
- key: `decision.catch-up-results`
- scope: private
- created: 2026-11-19T12:10:52Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.catch-up-results"}`

```text
decision.catch-up-results (planner-a, 2026-11-19): v0200-3 (ed4e5b1) is accepted. Met: catch-up 3.6 -> 0.4 ms at 10K and 30.8 -> 0.8 ms at 100K. Accepted misses: (1) a recall as the first call after a commit takes 8.2 ms at 100K (was 64.6) and 10.6 ms at 10K (target 5). The control, a warm recall after a 20 ms sleep with no commit, takes 11.8-18 ms, so the M1's slowdown after a pause dominates (fact.recall-after-pause); the caught-up state recalls warm at 2.2-2.4 ms. (2) The fallback, about once per 64 one-row commits, is about 10 ms slower than v0.19.1's reload (fresh page faults). (3) Footprint about +60-100 MB over 700 commits at 100K, against v0.19.1's +1.3 GB, with steps at fallbacks. Backlog, trigger a user report or a measured need: section 6's middle path (diff the new base), the second-recall 5.7 ms puzzle, incremental catch-up rising to 1.2 ms after 200 commits. Not accepted as is: the restore-in-place edge in catch-up.md 8a (identity and seal pass on an older copy of the same brain written over the file in place, and pointers can repeat), fixed in v0200-3r before step 8. Also on REVIEWER-B's scope.
```

## 01M7S45PHAJRMC3YEJKSY062S5

- type: episodic
- agent: `cursor-agent`
- key: `decision.discoverability`
- scope: private
- created: 2026-11-19T05:23:49Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795066187,"_mnemo.key":"decision.discoverability","_mnemo.superseded_by":"01M7S4GKWSA7MYM9KEAZ3DPX6A"}`

```text
decision.discoverability (user, 2026-11-18): yes to the GitHub social preview image (v0200-d1), the official MCP registry listing (v0200-d3), better crates.io and PyPI descriptions and keywords (v0200-d2), and README badges (Planner, in the v0.20.0 version-text hand-off; the README already has CI, PyPI, crates.io and licence badges, so add an MCP registry badge once listed, plus downloads). All ship in v0.20.0; v0200-5 READY waits for them. User steps: upload the preview in repo Settings; mcp-publisher login and publish after the tag. The demo: the Planner is explaining options; not yet decided. The launch day at v0.20.0: not yet answered. Repo description and 16 topics were set by the user the same day.
```

## 01M7S4GKWSA7MYM9KEAZ3DPX6A

- type: episodic
- agent: `cursor-agent`
- key: `decision.discoverability`
- scope: private
- created: 2026-11-19T05:29:47Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795070833,"_mnemo.key":"decision.discoverability","_mnemo.superseded_by":"01M7S8YD32BR9DFYXP0A96GZ97","_mnemo.supersedes":"01M7S45PHAJRMC3YEJKSY062S5"}`

```text
decision.discoverability (user, 2026-11-18): yes to the GitHub social preview image (v0200-d1), the official MCP registry listing (v0200-d3), better crates.io and PyPI descriptions and keywords (v0200-d2), README badges (Planner, in the v0.20.0 version-text hand-off: an MCP registry badge once listed, plus downloads), and the demo, option 1: a scripted terminal GIF re-recorded each release (v0200-d4; Planner places it in README.md and index.html at the version-text hand-off). Not chosen: a voiced video, an in-browser demo (backlog). All ship in v0.20.0; v0200-5 READY waits for them. User steps: upload the preview in repo Settings; mcp-publisher login and publish after the tag. The launch day at v0.20.0: not yet answered. Repo description and 16 topics were set by the user the same day.
```

## 01M7S8YD32BR9DFYXP0A96GZ97

- type: episodic
- agent: `cursor-agent`
- key: `decision.discoverability`
- scope: private
- created: 2026-11-19T06:47:13Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.discoverability","_mnemo.supersedes":"01M7S4GKWSA7MYM9KEAZ3DPX6A"}`

```text
decision.discoverability (user, 2026-11-18): yes to the GitHub social preview image (v0200-d1), the official MCP registry listing (v0200-d3), better crates.io and PyPI descriptions and keywords (v0200-d2), README badges (Planner, in the v0.20.0 version-text hand-off: an MCP registry badge once listed, plus downloads), and the demo, option 1: a scripted terminal GIF re-recorded each release (v0200-d4; Planner places it in README.md and index.html at the version-text hand-off). Not chosen: a voiced video, an in-browser demo (backlog). No launch day for now (user, 2026-11-18); don't raise it again unless the user does. All ship in v0.20.0; v0200-5 READY waits for them. User steps: upload the preview in repo Settings; mcp-publisher login and publish after the tag. Repo description and 16 topics were set by the user the same day. Planner idea for smarter memory: `mnemo task next --slug` (the Worker had to take and requeue v0200-2 twice to reach v0200-1h).
```

## 01M7TQYYTYMACJ9TGQXZ12EK39

- type: episodic
- agent: `cursor-agent`
- key: `decision.dogfood`
- scope: private
- created: 2026-11-19T20:28:55Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795124870,"_mnemo.key":"decision.dogfood","_mnemo.superseded_by":"01M7TWFFSPFHCGYWCX5SJEPXBF"}`

```text
decision.dogfood (user, 2026-11-19): "Be always vigilant to dogfood, we need to test the product ourselves, else we are building a shadow." How the Planner applies it: (1) at each release's after-tag checks, the installed ~/.local/bin/mnemo matches the tag, and every running `mnemo serve` started after the install (ps lstart against the binary's mtime); otherwise the user reconnects mnemo in that session (/mcp). (2) New features are exercised on the project brain, not only in CI. (3) Every friction point found this way becomes a backlog fact (status.backlog-small). Found the same day: the sessions' MCP servers had run the 2026-11-17 build (v0.18.x) since then.
```

## 01M7TWFFSPFHCGYWCX5SJEPXBF

- type: episodic
- agent: `cursor-agent`
- key: `decision.dogfood`
- scope: private
- created: 2026-11-19T21:47:50Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.dogfood","_mnemo.supersedes":"01M7TQYYTYMACJ9TGQXZ12EK39"}`

```text
decision.dogfood (user, 2026-11-19): "Be always vigilant to dogfood, we need to test the product ourselves, else we are building a shadow." How the Planner applies it: (1) at each release's after-tag checks, the installed ~/.local/bin/mnemo matches the tag, and every running `mnemo serve` started after the install (ps lstart against the binary's mtime); otherwise the user reconnects mnemo in that session (/mcp). (2) New features are exercised on the project brain, not only in CI. (3) Every friction point found this way becomes a backlog fact (status.backlog-small). (4) Release candidates (user "Yes", 2026-11-19): for each minor release, after REVIEWER-B's pass and its fixes, and before the tag, the Worker installs the release branch's build to ~/.local/bin/mnemo, every session reconnects mnemo (/mcp), and the sessions run on it until the tag; the Worker reports the version and the Planner checks the server start times. Found the same day: the sessions' MCP servers had run the 2026-11-17 build since then; planner-a and reviewer-b reconnected 13:43, worker-a waits until v0200-5 is done.
```

## 01M7DZWX22CVNEXTA7ZVYMP5R4

- type: episodic
- agent: `cursor-agent`
- key: `decision.dogfood-keyed-writes`
- scope: private
- created: 2026-11-14T21:37:28Z
- importance: 0.8
- metadata: `{"_mnemo.key":"decision.dogfood-keyed-writes","area":"decisions"}`

```text
2026-11-14, PLANNER-A: after step 5b, the five unkeyed current facts that duplicated their new keyed records were forgotten (each matched exactly), and from now on DOGFOOD-0 keys every decision and fact from its first write; a fact that already exists unkeyed is adopted under its key with an identical-content --supersedes (outcome updated) instead of being written again.
```

## 01M7GJSSHMTESHQVEAH96G8V5J

- type: episodic
- agent: `cursor-agent`
- key: `decision.dogfood-switch`
- scope: private
- created: 2026-11-15T21:46:18Z
- importance: 0.9
- metadata: `{"_mnemo.key":"decision.dogfood-switch","area":"decisions"}`

```text
2026-11-15 (v0.16 step 7b, the dogfood switch, done by the user): the project brain moved from test/project.mnemo to .git/mnemo/project.mnemo, rekeyed with --new-key so its generated key is in the user's keychain (test/.env retired beside a backup outside the repo); mnemo 0.16.1 installed to ~/.local/bin; mnemo onboard --client claude-code wired the local MCP server, the three hooks and the skill. The user decided all four sessions (worker-a, planner-a, reviewer-a, reviewer-b) run the hooks: each saves its own transcript and loads the brief at start; only the Worker writes facts, with a key; the Planner and the Reviewers only read (AGENTS.md tmux rule 9).
```

## 01M7DXVVAZXNZE0AQEYH23MQ67

- type: episodic
- agent: `cursor-agent`
- key: `decision.embedder.model`
- scope: private
- created: 2026-11-14T21:01:56Z
- importance: 0.9
- metadata: `{"_mnemo.key":"decision.embedder.model","area":"decisions"}`

```text
2026-11-14, the user: the built-in embedder is snowflake-arctic-embed-xs, identifier-aware: a recall's plain words get no keyword weight, and tokens shaped like identifiers (backticked, ::, @, =, --flag, a_b, camelCase, name(...), dotted names) become explicit keywords at 0.5; it had to beat the fixed weights on both question sets first, and did (step 5 part 3b).
```

## 01M7P5HPX2S3GN91JVZKRAXCZC

- type: episodic
- agent: `cursor-agent`
- key: `decision.inspect-coord`
- scope: private
- created: 2026-11-18T01:50:08Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.inspect-coord"}`

```text
Decision (2026-11-17): INSPECT-COORD gives humans a place to monitor coordination: Findings and Tasks tabs in `mnemo inspect`, served by GET-only /api/findings and /api/tasks; brain text goes into text nodes only; tasks keep `task list` order. Shipped in v0.18.1 (PR 130, 4c696f0).
```

## 01M7E4KZV5H9W7P2P55ER7ZS6M

- type: episodic
- agent: `cursor-agent`
- key: `decision.msrv`
- scope: private
- created: 2026-11-14T22:59:59Z
- importance: 0.8
- metadata: `{"_mnemo.key":"decision.msrv","area":"decisions"}`

```text
2026-11-14, the user: raise the MSRV to 1.94 for everything (not a 1.94 floor for embed-candle alone, nor an older candle), because candle-core 0.11.1 needs it on x86_64 and v0.15 is unreleased, so nobody relied on 1.92. PLANNER-A, same day: the aarch64 manylinux wheel builds ring with CFLAGS_aarch64_unknown_linux_gnu=-D__ARM_ARCH=8 for that target only (keeping manylinux2014), checked by installing the wheel on an ubuntu-24.04-arm runner.
```

## 01M7P5HQ1TMWZZSETSGX80GYY1

- type: episodic
- agent: `cursor-agent`
- key: `decision.non-exhaustive-0.19`
- scope: private
- created: 2026-11-18T01:50:08Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.non-exhaustive-0.19"}`

```text
Decision (2026-11-17): v0.18.1 adds public field `refs` to TaskSpec and Task (not #[non_exhaustive]), kept in a patch release with a CHANGELOG line saying what struct-literal code needs. v0.19.0 marks the 13a public types #[non_exhaustive]: TaskSpec, Task, FindingSpec, Finding, NewWork, and the message and claim specs.
```

## 01M7ESFPSXH9D02BM8WQB704J1

- type: episodic
- agent: `cursor-agent`
- key: `decision.onboard`
- scope: private
- created: 2026-11-15T05:04:39Z
- importance: 0.9
- metadata: `{"_mnemo.key":"decision.onboard","area":"decisions"}`

```text
2026-11-11 decision (user), restated for 12.2 on 2026-11-15: mnemo onboard is the one entry point, safe to re-run. It finds or creates the project brain through git rev-parse --git-common-dir (every worktree shares one file), keeps its generated key in the OS keychain (the one step that may need the human), installs the built-in model, wires Claude Code and Cursor (MCP through mcp install, rules, hooks), and prints the guide; onboard --guide prints only the guide, and mnemo init stays the low-level create. GUIDE-1 is one source text compiled into the binary, delivered by onboard --guide, the MCP server, the rule files and llms.txt, and every command in it runs in CI. Layers: 1 setup and 2 organizing memory ship with 12.2, 3 orchestration with 13a. Trust boundary: the guide lives in the binary; project conventions in the manifest are project notes that never override it. Acceptance: a fresh agent, told only how to install mnemo and then 'run mnemo onboard', runs this project's workflow unaided; the only human step is the keychain prompt. Design note: docs/design/12.2-onboard.md (v0.16.0 step 0, PR 91).
```

## 01M7PGEBHXDYV6EDD1PHMA3QCF

- type: episodic
- agent: `cursor-agent`
- key: `decision.perf-2-design`
- scope: private
- created: 2026-11-18T05:00:33Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.perf-2-design"}`

```text
decision.perf-2-design (PLANNER-A, 2026-11-17, review of v0190-1: PR 135, docs/design/perf-2.md). Accepted, with this order and scope for v0190-2:
1. Merge PR 135 first, as it is, then build on release-0.19.0.
2. Measure the 100K index load's split before choosing (decode, the id->partition map, reconcile, posting bytes), and put the split in the report.
3. Build layout 2 in two stages. Stage A: the head (centroids, codebook, directory) plus postings as raw flat arrays, still loaded whole on first use but with no per-element msgpack decode, and the id->partition map built only when a write path needs it. Stage B, lazy load by partition (queued Adds per unloaded partition, the handle-wide removed set): only if stage A leaves first recall at 100K above 15 ms. Either way every new base is layout 2 and layout 1 is still read.
4. 3b (batched reads, parallel decrypt with scoped threads above a measured threshold, at most available_parallelism) and 3c (Pager::read_pages) are in. Record the single-thread number as well, so phones without spare cores are covered.
5. 3d: the two single-thread levers only. No parallel warm scan in v0.19.0; if 10K warm recall stays above 2 ms, report the number and the README says it honestly.
6. 3f (merging the 4th fsync): DEFERRED, not in v0.19.0. It's a durability change for one fsync on some batch commits; it gets its own task later with its crash tests. The investigation in the note is the spec's ask, done.
7. Also fix ann_defaults_v0_6_0 so it runs on v9 without MNEMO_DEV_ANN_MIN_FACTS (skip or label the tiers below V9_ANN_MIN_FACTS), and so it never writes to a tracked perf file by default.
8. Tests as the note's section 5, minus the 3f ones. The compat test with the published 0.18.1 binary is required.
Every number in the report from a run; note the machine load as in step 1.
```

## 01M7GMSXBAQPP7R50GB4XQQMD7

- type: episodic
- agent: `cursor-agent`
- key: `decision.phase-13a-start`
- scope: private
- created: 2026-11-15T22:21:19Z
- importance: 0.85
- metadata: `{"_mnemo.key":"decision.phase-13a-start","area":"decisions"}`

```text
2026-11-15, the user: 'go' for Phase 13a, released as v0.17.0, all free (one machine, the Monetization rule). Scope: 13.1 messages and tasks with 13.1b delivery, 13.3 claims and presence, 13.5 visibility, 13.6 freshness. REVIEWER-A's REV-010 pass (its scheduled pass before a phase) runs first, and the Worker's design note docs/design/13a-coordination.md is step 0, on release-0.17.0.
```

## 01M7PBZ94MBPZ7BDY2J0FY9G76

- type: episodic
- agent: `cursor-agent`
- key: `decision.positioning`
- scope: private
- created: 2026-11-18T03:42:24Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.positioning"}`

```text
Decision (user, 2026-11-17): positioning — "MNemo doesn't compete with vector databases. It competes with CLAUDE.md and a notes folder." The README and site explain it in three layers: memory you can search; agents sharing work; the engine underneath (encryption, crash safety, snapshots, size). mnemo inspect is a key command (hero step 3). Say "coordinate", never "orchestration". Shipped as a docs-only PR to main (no release); v0.19.0 adds a date range filter to inspect and a Findings-tab screenshot from a demo brain.
```

## 01M7GRC02NRRAE09TCHHM4D02S

- type: episodic
- agent: `cursor-agent`
- key: `decision.product-terms`
- scope: private
- created: 2026-11-15T23:23:37Z
- importance: 0.85
- metadata: `{"_mnemo.key":"decision.product-terms","area":"decisions"}`

```text
2026-11-15, the user ("Yes", R-010.4 confirming PLANNER-A): "memory" is the category word (headline, search, comparisons); "project brain" names the file (.git/mnemo/project.mnemo) every time; "shared memory for your agents" is the qualified phrase, or "shared memory and coordination for your agents" when introducing 13a, never "shared memory" alone (the OS term); "coordinate"/"coordination" are the verbs once 13a ships (rule of thumb: "You run the agents; they remember and coordinate through MNemo"); never "orchestration" or "swarm", since MNemo starts no agents. Applies to the site, READMEs, guide and llms.txt from v0.17.0.
```

## 01M7E8YFE4MZ6SMEN620EDP230

- type: episodic
- agent: `cursor-agent`
- key: `decision.release-artifact-checks`
- scope: private
- created: 2026-11-15T00:15:37Z
- importance: 0.8
- metadata: `{"_mnemo.key":"decision.release-artifact-checks","area":"decisions"}`

```text
2026-11-14, PLANNER-A (part 4 round 3): build the aarch64 Linux wheel natively on ubuntu-24.04-arm (maturin's native manylinux2014_aarch64 image) instead of forcing __ARM_ARCH in the cross image, and gate every release on check-artifacts: the CLI and the wheel run on aarch64 Linux and on Intel macOS (macos-15-intel; macos-13 is retired), each fetching the built-in model, then remembering and recalling. approve-release needs it.
```

## 01M7EH1AVNQEG42ZWDTRJGJ1NK

- type: episodic
- agent: `cursor-agent`
- key: `decision.release-checks-all-targets`
- scope: private
- created: 2026-11-15T02:36:59Z
- importance: 0.8
- metadata: `{"_mnemo.key":"decision.release-checks-all-targets","area":"decisions"}`

```text
2026-11-15, PLANNER-A (RB-010.3): the release workflow runs every published artifact before the approval: check-artifacts on all five targets, each on a runner of its own (x86_64 Linux ubuntu-latest, aarch64 Linux ubuntu-24.04-arm, arm64 macOS macos-latest, x86_64 macOS macos-15-intel, Windows windows-latest under bash), the CLI and the wheel each installing the model and remembering and recalling; and check-sdist builds and imports the sdist. approve-release needs both.
```

## 01M7NBTNV3TADMZFBVMWE832A9

- type: episodic
- agent: `cursor-agent`
- key: `decision.release-main-merge`
- scope: private
- created: 2026-11-17T18:20:39Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"decision.release-main-merge","area":"decision","topic":"release"}`

```text
Release rule (PLANNER-A decision 2026-11-17, in RELEASING.md since v0180-2): any change to main during a release is merged back into the release branch before the next task, by a PR with main as head and the release branch as base, merged with gh pr merge --merge (a merge commit, never a squash, so main stays an ancestor and the tag-time fast-forward of main works). Check: git merge-base --is-ancestor origin/main origin/release-X. First use: PR 125, 9f3ab28, for v0.18.0.
```

## 01M7NHYHJ72W9YCKSBASD0PW0P

- type: episodic
- agent: `cursor-agent`
- key: `decision.roadmap-after-0.18`
- scope: private
- created: 2026-11-17T20:07:37Z
- importance: 0.75
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.roadmap-after-0.18"}`

```text
Roadmap after v0.18.0 (user decision 2026-11-17): HARNESS-DOGFOOD-1 Run 2, then PERF-2 (partition-major index loading and a lazy vector-column fill; first recall at 100K toward <= 15 ms, at 10K toward <= 5 ms, no format bump). Sync (Pro, then Teams) waits: it is decided later on the waitlist's numbers, since the waitlist had 1 sign-up and was not visible until the site's top bar (2bf9b74).
```

## 01M7RKEDCKA17T3B4CKXAKY0T7

- type: episodic
- agent: `cursor-agent`
- key: `decision.roadmap-order`
- scope: private
- created: 2026-11-19T00:31:29Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795057400,"_mnemo.key":"decision.roadmap-order","_mnemo.superseded_by":"01M7RW4EZBHMAWQ9Y4K2T2BPWK"}`

```text
Roadmap order (user decision 2026-11-18, "Aligned"): after v0.19.1, (1) the 3f fsync merge (PERF-2's deferred item: fewer syncs on some batch commits, its own release and crash tests); (2) the Planner's sync design note; then the site's order: (3) sharing one file, continued (catch-up by delta, shared on by default for MCP); (4) encrypted sync, Pro then Teams, decided on the waitlist's numbers; (5) smarter memory; (6) teams across people and machines; (7) mobile see-and-delete screens; (8) Node.js and framework integrations. Carried: KEYCHAIN-REPROMPT, WAKE-SOCKET, PY-SDIST, RB-006.12, the timing flakes, whether recall should return findings.
```

## 01M7RW4EZBHMAWQ9Y4K2T2BPWK

- type: episodic
- agent: `cursor-agent`
- key: `decision.roadmap-order`
- scope: private
- created: 2026-11-19T03:03:20Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795057709,"_mnemo.key":"decision.roadmap-order","_mnemo.superseded_by":"01M7RWDWRHDX4EM4RHVY41155X","_mnemo.supersedes":"01M7RKEDCKA17T3B4CKXAKY0T7"}`

```text
Roadmap order (user decisions 2026-11-18): v0.20.0 now (the 3f fsync merge; sharing one file, continued: incremental catch-up and shared by default for serve --mcp). Then the sync product, not waiting on the waitlist (decision.sync-build, docs/design/sync.md): 13.7 mnemo merge; change sets on disk; the relay protocol; the hosted relay (Pro); 13.2 key slots and 13.4 signatures; Teams. After that: smarter memory; mobile see-and-delete screens; Node.js and framework integrations. Carried: KEYCHAIN-REPROMPT, WAKE-SOCKET, PY-SDIST, RB-006.12, the timing flakes, whether recall should return findings.
```

## 01M7RWDWRHDX4EM4RHVY41155X

- type: episodic
- agent: `cursor-agent`
- key: `decision.roadmap-order`
- scope: private
- created: 2026-11-19T03:08:29Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795057847,"_mnemo.key":"decision.roadmap-order","_mnemo.superseded_by":"01M7RWJ2ZVHTNN3BBEYFHD69A6","_mnemo.supersedes":"01M7RW4EZBHMAWQ9Y4K2T2BPWK"}`

```text
Decision (user, 2026-11-18): hold off on paid features; move the rest of the roadmap up. Held: the relay protocol, the hosted relay (Pro), key slots and signatures (13.2, 13.4, listed under Teams in the 2026-11-15 monetization plan), Teams. Free work comes first, after v0.20.0. Planner's proposed order, awaiting the user's yes: (1) 13.7 mnemo merge; (2) sync change sets on disk (mnemo sync export/import, through any folder); (3) smarter memory (merge duplicates, summarise sessions, link related memories, forget stale ones); (4) mobile see-and-delete screens; (5) Node.js package and framework integrations. Supersedes decision.sync-build's build-everything order.
```

## 01M7RWJ2ZVHTNN3BBEYFHD69A6

- type: episodic
- agent: `cursor-agent`
- key: `decision.roadmap-order`
- scope: private
- created: 2026-11-19T03:10:47Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795118764,"_mnemo.key":"decision.roadmap-order","_mnemo.superseded_by":"01M7TPN4QN1ZWBZVR5BGHMNW61","_mnemo.supersedes":"01M7RWDWRHDX4EM4RHVY41155X"}`

```text
Roadmap order (user decision 2026-11-18, "Aligned"): paid features on hold (relay protocol, hosted relay Pro, 13.2 key slots, 13.4 signatures, Teams). After v0.20.0, the free work in this order: (1) 13.7 mnemo merge; (2) sync change sets on disk (mnemo sync export/import through any folder); (3) smarter memory (merge duplicates, summarise sessions, link related memories, forget stale ones); (4) mobile see-and-delete screens; (5) Node.js package and framework integrations. Carried items ride as patches: KEYCHAIN-REPROMPT, PY-SDIST, WAKE-SOCKET, RB-006.12, whether recall returns findings, the timing flakes.
```

## 01M7TPN4QN1ZWBZVR5BGHMNW61

- type: episodic
- agent: `cursor-agent`
- key: `decision.roadmap-order`
- scope: private
- created: 2026-11-19T20:06:04Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.roadmap-order","_mnemo.supersedes":"01M7RWJ2ZVHTNN3BBEYFHD69A6"}`

```text
Roadmap order (user decisions 2026-11-18, "Aligned", and 2026-11-19, "Yes" to REV-011.6's swap): paid features on hold (relay protocol, hosted relay Pro, 13.2 key slots, 13.4 signatures, Teams). After v0.20.0, the free work in this order: (1) 13.7 mnemo merge; (2) sync change sets on disk (mnemo sync export/import through any folder; designed as sequential immutable files fit for object-storage conditional writes, REV-011.4); (3) smarter memory (merge duplicates, summarise sessions, link related memories, forget stale ones; MCP elicitation and mnemo doctor); (4) Node.js package and framework integrations (the AI SDK memory package first, then eve, then the sandbox guide); (5) mobile see-and-delete screens. Hosted brains for an agent harness: not now (user, 2026-11-19; status.rev-011). Carried items ride as patches: KEYCHAIN-REPROMPT, PY-SDIST, WAKE-SOCKET, RB-006.12, whether recall returns findings, the timing flakes.
```

## 01M7PEYPSTKP6NTD4E8NBCMWV8

- type: episodic
- agent: `cursor-agent`
- key: `decision.run-2`
- scope: private
- created: 2026-11-18T04:34:31Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795031603,"_mnemo.key":"decision.run-2","_mnemo.superseded_by":"01M7R3H66X0PAAPQXPS7CYVXZH"}`

```text
Decision (user, 2026-11-17: "Yes"): HARNESS-DOGFOOD-1 Run 2 runs on v0.19.0. The brain is the ledger: PLAN.md and reviewer_notes.md are frozen archives with head notes; status and order are the project essence; dispatches are tasks (spec in the body or a spec.<slug> fact); Planner decisions and verifications are decision.* or status.* facts or task notes; the Worker's reports are report.<slug> facts named in task updates; review findings are records; hand-offs of Planner-owned files are tasks. Pointers name a slug or key. Every task starts with a recall and its report says what came back. docs/brain.md replaces the chore(plan) commit. The rules are AGENTS.md's subsection "The brain is the ledger". Run 2 passes if v0.19.0 goes dispatch to tag with no hand-edited PLAN.md or reviewer_notes.md.
```

## 01M7R3H66X0PAAPQXPS7CYVXZH

- type: episodic
- agent: `cursor-agent`
- key: `decision.run-2`
- scope: private
- created: 2026-11-18T19:53:23Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.run-2","_mnemo.supersedes":"01M7PEYPSTKP6NTD4E8NBCMWV8"}`

```text
Decision (user, 2026-11-18): Run 2's way of working is permanent: the project brain carries the work (tasks, report.* facts, decision/spec/status facts, finding records, the project essence); PLAN.md and reviewer_notes.md stay frozen archives. The user chose option 1: rewrite AGENTS.md's whole 'Multi-agent roles & coordination' section to describe only this (approved text, handed off as v0191-0), rather than just renaming the Run 2 heading.
```

## 01M7P5HQ05970SDH96H09KHQTQ

- type: episodic
- agent: `cursor-agent`
- key: `decision.security-finding-private-until-verified`
- scope: private
- created: 2026-11-18T01:50:08Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.security-finding-private-until-verified"}`

```text
Decision (2026-11-17): a SECURITY finding record is written private until verified, and is verified only against a release tag ^v\d+\.\d+\.\d+$, which makes it team. export --git leaves out unverified SECURITY findings and (since v0.18.1) any task whose _mnemo.refs names one. Anything outside SECURITY.local.md that refers to an unfixed finding names it by id only (user decision, AGENTS.md f77040b).
```

## 01M7GJSTNC1NBJJHEM466WW8VF

- type: episodic
- agent: `cursor-agent`
- key: `decision.site-2026-11`
- scope: private
- created: 2026-11-15T21:46:19Z
- importance: 0.7
- metadata: `{"_mnemo.key":"decision.site-2026-11","area":"decisions"}`

```text
2026-11-15, the user's three choices for the landing page redesign (index.html, live as PR 101, 05a5ca4): the problem-first headline ('Your coding agent forgets. Give it a memory.'); light and dark themes that follow the system, with a toggle kept in localStorage; and the page presents mnemo as free only, with sync as a roadmap line. The user reviews the live page and asks for changes.
```

## 01M7E9BES46T2W9CQJZNVNXQMW

- type: episodic
- agent: `cursor-agent`
- key: `decision.strip-release`
- scope: private
- created: 2026-11-15T00:22:42Z
- importance: 0.8
- metadata: `{"_mnemo.key":"decision.strip-release","area":"decisions"}`

```text
2026-11-14, the user: strip the release CLI binaries (strip = "symbols" in the release profile, so local release builds match); keep the strip for the wheels' library too if it shrinks. A panic still prints its message, with addresses for function names; RELEASING.md says to rebuild the tag unstripped (CARGO_PROFILE_RELEASE_STRIP=none) to read a backtrace.
```

## 01M7DZPTJ0TM551015HRRH70PZ

- type: episodic
- agent: `cursor-agent`
- key: `decision.supersede-named`
- scope: private
- created: 2026-11-14T21:34:09Z
- importance: 0.8
- metadata: `{"_mnemo.key":"decision.supersede-named","area":"decisions"}`

```text
2026-11-14, the user: add a 'replaces' option to the engine in v0.15, as its own small PR (step 5b) before the built-in embedder, so a keyed write can supersede a named memory that was written without a key; the five stale facts from step 5 part 3 are then re-written under keys, superseding them.
```

## 01M7RW4ETJ728Z39PFXZM6M25W

- type: episodic
- agent: `cursor-agent`
- key: `decision.sync-build`
- scope: private
- created: 2026-11-19T03:03:20Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.sync-build"}`

```text
Decision (user, 2026-11-18): build the whole sync product, not gated on the waitlist ("Let's not wait for signups, we build the full product ... the product is [what's important]"). After v0.20.0, in docs/design/sync.md's build order, each step its own release with a REVIEWER-B pass: (1) 13.7 mnemo merge with the HLC and merge conflicts (free); (2) change sets on disk, mnemo sync export/import (free); (3) the relay protocol with a test relay; (4) the hosted relay, Pro (private repo; pricing, hosting and the business prerequisites are the user's, before it ships); (5) Teams after 13.2 key slots and 13.4 signatures. Planner's default: messages don't sync in the first version.
```

## 01M7RXSC9R1RYP7Q6TZ9E2SFZR

- type: episodic
- agent: `cursor-agent`
- key: `decision.vercel-supabase-ideas`
- scope: private
- created: 2026-11-19T03:32:14Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.vercel-supabase-ideas"}`

```text
Decision (user, 2026-11-18), ideas from Supabase and Vercel: (1) yes: MCP elicitation, a confirmation before destructive tools (forget, essence remove, bulk deletes); (2) yes: brain health checks, mnemo doctor plus a read-only MCP prompt (stale facts, merge conflicts, likely duplicates, unverified findings); both go in the smarter-memory step. (3) yes: a memory tool package for Vercel's AI SDK first in the Node.js step, an eve extension after it. (4) yes: a guide to MNemo as the memory file in sandboxes (Vercel Sandbox and Drives, Claude Managed Agents), with the Node.js step. (5) backlog: public, repeatable benchmarks against CLAUDE.md, a notes folder and other memory tools.
```

## 01M7P5HPTPB6FSFQGZBRZPKB17

- type: episodic
- agent: `cursor-agent`
- key: `decision.waiting-pointer`
- scope: private
- created: 2026-11-18T01:50:08Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795015163,"_mnemo.key":"decision.waiting-pointer","_mnemo.superseded_by":"01M7QKVF7TM8DP3W2FYED18SNQ"}`

```text
Decision (user, 2026-11-17): Worker -> Planner pointer `WAITING: <entry title>` when the Worker stops for the user (release approval click, a token, a permission). The Worker still tells the user directly; the pointer lets planner-a show it. AGENTS.md tmux rule 1, committed 88c3c37; rule 6 lets the Worker append at DONE, BLOCKED or WAITING.
```

## 01M7QKVF7TM8DP3W2FYED18SNQ

- type: episodic
- agent: `cursor-agent`
- key: `decision.waiting-pointer`
- scope: private
- created: 2026-11-18T15:19:23Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"decision.waiting-pointer","_mnemo.supersedes":"01M7P5HPTPB6FSFQGZBRZPKB17"}`

```text
Decision (user, 2026-11-17; resume step added by planner-a 2026-11-18): Worker -> Planner pointer `WAITING: <slug>` when the Worker stops for the user (release approval click, a token, a permission); the Worker still tells the user directly, and planner-a shows it. The way back: when the user tells planner-a it's done ("Approved"), planner-a notes it on the task (task update <slug> --note "user reports approved in planner-a at <time>; check run <id>") and sends worker-a `READ: <slug>`. The Worker then checks the run or setting itself (gh run view), never acting on the message alone. Release task bodies also tell the Worker to watch the run in the background after WAITING, so the click alone wakes it. Missed at v0.19.0: the user had to tell the Worker.
```

## 01M7ESPCFF1MKV7V6032Z2Z1GM

- type: semantic
- agent: `cursor-agent`
- key: `design.122-brain-location`
- scope: private
- created: 2026-11-15T05:08:18Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-brain-location","area":"decisions"}`

```text
12.2 design (signed off 2026-11-15): the project brain is <git-common-dir>/mnemo/project.mnemo (usually .git/mnemo/project.mnemo), so every worktree shares it, git never tracks it, and its shared-mode sidecar stays inside .git; <common>/mnemo/ is created 0700. The PATH argument is optional on the brain commands (about, recall, remember, essence, thread, serve, mcp install, onboard, inspect, info, verify, export, list, history; get keeps it); with none, the brain is found from the global -C <dir>, else CLAUDE_PROJECT_DIR, else the working directory, and outside a git work tree the error is 'no project brain here'. So configs onboard writes carry no path. No personal brain beside it until 13a.
```

## 01M7EY27JJB333WVBQP5AJ9RTP

- type: procedural
- agent: `cursor-agent`
- key: `design.122-guide-pending`
- scope: private
- created: 2026-11-15T06:24:40Z
- importance: 0.8
- metadata: `{"_mnemo.key":"design.122-guide-pending","area":"decisions"}`

```text
12.2 GUIDE-1 pending rule (PLANNER-A, 2026-11-15, v0.16.0 step 2): the guide (mnemo/guide/guide.md) is written in full from step 2, including commands later steps build. Every block or command for one that doesn't exist yet goes on PENDING_UNTIL_STEP in mnemo/tests/guide.rs, an entry of (text to match, step, optional stand-in command); tests/guide.rs skips matching blocks and commands (running the stand-in instead, e.g. mnemo init for mnemo onboard --yes until step 5), and every_pending_entry_is_still_needed fails on a stale entry. Each later step removes its own entries (step 3: resources/read and prompts/get; step 4: mnemo hook; step 5: mnemo onboard --yes, --undo and the integrations/claude-code plugin skill). At READY the list must be empty, and PLANNER-A checks that.
```

## 01M7F2CQ5Y2H20RQFBNQ0P38G8

- type: semantic
- agent: `cursor-agent`
- key: `design.122-hook-inputs`
- scope: private
- created: 2026-11-15T07:40:18Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-hook-inputs","area":"decisions"}`

```text
12.2 step 4, mnemo hook's inputs and rules (PLANNER-A's five points, as built in v0.16.0): (1) the brain is looked for from -C, then CLAUDE_PROJECT_DIR (absolute), then the input's cwd (Claude Code) or workspace_roots[0] (Cursor), absolute and a directory, then the working directory; the CLAUDE_PROJECT_DIR stderr note is off in the hook. (2) session_id (Cursor: conversation_id) must be 1 to 128 characters of [A-Za-z0-9._-], since it becomes the thread id claude-code:<id> or cursor:<id>; otherwise nothing is saved and stderr says 'the session id isn't one this hook accepts'. (3) transcript_path must be absolute and a regular file, checked with metadata before the open and on the handle after it, opened O_NONBLOCK on Unix so a FIFO never blocks; over 64 MiB, its last 64 MiB are read from the first whole line, and lines are parsed leniently (a half-written last line is skipped). (4) the hooks' saves always use --tool-results omit (only [tool_result ok|error, N bytes]) and redaction (mnemo_engine::redact::secrets); thread ingest has --tool-results and --redact for a direct call, off by default; stderr names counts by kind, never values. (5) it always exits 0: the event runs under catch_unwind, every problem is a one-line note. Saves need an embedder (the built-in with its model installed, never a download); tests use the dev-hooks switch MNEMO_DEV_HOOK_EMBED_CMD.
```

## 01M7ESPCP2B0C9DN69NX9XZXZE

- type: semantic
- agent: `cursor-agent`
- key: `design.122-hooks`
- scope: private
- created: 2026-11-15T05:08:18Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-hooks","area":"decisions"}`

```text
12.2 design (signed off 2026-11-15): the hooks are one subcommand, mnemo hook <session-start|pre-compact|session-end> --client <claude-code|cursor>, which always exits 0, never prompts, never downloads, and never contends with the --shared server (session-start reads with --wait 2s). onboard writes them as settings hooks by default (.claude/settings.local.json, per-user scope, files listed in .git/info/exclude); the plugin is an option, since a plugin's SessionEnd hook is capped at 1.5 s. session-start injects at most 8,000 B by default (--budget, at most 9,000; onboard --hook-budget). pre-compact and session-end ingest the transcript; pre-compact's stdout feeds Claude Code's compaction summary. A --client claude-code run that sees cursor_version exits silently, since Cursor also runs Claude Code's hooks. No Stop hook.
```

## 01M7ESPCJGYD7G3BYN1Y8ZPQZC

- type: semantic
- agent: `cursor-agent`
- key: `design.122-install`
- scope: private
- created: 2026-11-15T05:08:18Z
- importance: 0.8
- metadata: `{"_mnemo.key":"design.122-install","area":"decisions"}`

```text
12.2 design (signed off 2026-11-15): the guide's first line is an install script on the site (install.sh, install.ps1; Worker-owned root files) that fetches the release binary, checks its SHA-256 against a new SHA256SUMS release asset and, when gh is installed, the build attestation (gh attestation verify --repo toarchkumar/mnemo), installs to ~/.local/bin without sudo, and ends 'Next: mnemo onboard' (the full path when ~/.local/bin isn't on the PATH). cargo binstall mnemo-engine is the second line (it checks only TLS for a release binary). The mnemo-engine-cli crate and a Homebrew tap wait; the features stay off by default in mnemo-engine.
```

## 01M7F7TZNYT5KZMPRKG6FBXWVR

- type: procedural
- agent: `cursor-agent`
- key: `design.122-install-hardening`
- scope: private
- created: 2026-11-15T09:15:28Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-install-hardening","area":"decisions"}`

```text
12.2 step 6, the install scripts' rules (PLANNER-A, as built in v0.16.0; install.sh in POSIX sh, install.ps1): (1) everything is in functions and the last line calls main (install.ps1: one function, then a call), so a download cut short runs nothing; install.ps1 uses throw, not exit, so `irm | iex` never closes the user's PowerShell. (2) MNEMO_VERSION must match ^[0-9]+.[0-9]+.[0-9]+$; 'latest' comes from the redirect of https://github.com/toarchkumar/mnemo/releases/latest, no API, no token. (3) MNEMO_INSTALL_BASE is an absolute path to an existing local directory only (a URL or a relative path is refused), needs MNEMO_VERSION, and skips the attestation, saying so. (4) with gh on the PATH and `gh auth status` passing, `gh attestation verify <file> --repo toarchkumar/mnemo`, and a failure installs nothing; without, it says only the SHA-256 was checked. (5) the download goes to mktemp -d, the SHA-256 is checked against the release's SHA256SUMS (a mismatch installs nothing), then a copy beside ~/.local/bin/mnemo and one mv over it. (6) curl --proto '=https' --tlsv1.2 -fsSL, or wget --https-only, or stop. The Next line names the full path when ~/.local/bin isn't on the PATH. release.yml's checksums job writes SHA256SUMS and, on tag runs only, attests the five binaries (actions/attest-build-provenance v4.2.2, pinned by SHA; attestations: write and id-token: write on that job alone); check-artifacts runs each script against the dry run's files.
```

## 01M7F1G7YYRRRDBS43Y36C1QHR

- type: semantic
- agent: `cursor-agent`
- key: `design.122-mcp-context`
- scope: private
- created: 2026-11-15T07:24:45Z
- importance: 0.8
- metadata: `{"_mnemo.key":"design.122-mcp-context","area":"decisions"}`

```text
12.2 step 3 as built (v0.16.0, PR 94, accepted 2026-11-15): the MCP server's initialize sends the guide's card as instructions and advertises resources and prompts. Resources (text/markdown, read-only, no access tracking, shared reads under SharedMnemo::read): mnemo://about (card, framing line, onboarding memories, project essence; 8,000 B), mnemo://recent (the 20 newest live facts by created_at, one line each cut at 300 B; 6,000 B), mnemo://guide (the whole guide), and the template mnemo://key/{key} (live value then earlier versions, default agent's slot then shared; 4,000 B). A missing key or unknown URI is -32002, a missing uri or unknown prompt -32602. Prompts: session-start (optional task: card, the task, framing line, project essence, then an untracked top-5 recall; with no task the five most recent memories; 6,000 B) and session-end (the save reminder; with an optional thread, the framing line and that thread's essence). Accepted choices: no-task session-start shows recent memories (the git query belongs to the hook), and 'recent' means newest created_at (an updated time would be a format change). Code: mnemo/src/mcp/context.rs, Mnemo::recent(n); tests: tests/mcp_context.rs.
```

## 01M7F4S2T04M1MMWHCWGM32MCX

- type: semantic
- agent: `cursor-agent`
- key: `design.122-onboard-scopes`
- scope: private
- created: 2026-11-15T08:22:00Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-onboard-scopes","area":"decisions"}`

```text
12.2 step 5, mnemo onboard's scopes and commands (PLANNER-A's points 1, 2 and 4, as built in v0.16.0): --scope local (the default) wires Claude Code's local scope through claude mcp add-json (or .mcp.json when claude isn't on the PATH), hooks in .claude/settings.local.json, the skill, and Cursor's .cursor/mcp.json, hooks.json and rules/mnemo.mdc, all listed in .git/info/exclude, with the running binary's absolute path as the command (a GUI-started client may not have ~/.local/bin on its PATH). --scope project writes .mcp.json, .claude/settings.json and Cursor's files for committing, with bare mnemo. A bare mcp install with no path defaults to local for Claude Code and project for Cursor (its file listed in .git/info/exclude). Hook timeouts in the configs: SessionStart 15 s, PreCompact 30 s, SessionEnd 30 s; --hook-budget N adds --budget N to the SessionStart command. --plugin installs the plugin with claude plugin install mnemo --marketplace toarchkumar/mnemo --scope <scope> -y (install id mnemo@mnemo-engine) instead of the settings hooks and skill.
```

## 01M7F4S2KNK7XMKJVMHMQA2E8S

- type: semantic
- agent: `cursor-agent`
- key: `design.122-onboard-undo`
- scope: private
- created: 2026-11-15T08:22:00Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-onboard-undo","area":"decisions"}`

```text
12.2 step 5, how mnemo onboard --undo recognises its own output (PLANNER-A's point 3, as built in v0.16.0): an MCP entry is onboard's when its command's file name is mnemo or mnemo.exe (any directory, either path separator) and 'serve' is in its args; a hook handler is onboard's when its command's first word (quoted or not) is mnemo/mnemo.exe followed by 'hook'; a generated file (the skill, the Cursor rule) is onboard's when it carries guide::ONBOARD_HEADER; the .git/info/exclude block is the one between its two marker lines. It never removes anything that doesn't match (a user's own hook in the same event array stays), and never the brain, its keychain entry or the model; its summary prints how to delete those (security delete-generic-password -s io.github.toarchkumar.mnemo.brain -a <salt hex> on macOS, cmdkey on Windows, secret-tool on Linux). A JSON file left as an empty object, and a directory left empty, are removed.
```

## 01M7FZP0J81JBN4F0RAYCP74E1

- type: episodic
- agent: `cursor-agent`
- key: `design.122-post-release-order`
- scope: private
- created: 2026-11-15T16:12:11Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-post-release-order","area":"decisions"}`

```text
2026-11-15, PLANNER-A (v0.16.0): the dogfood switch (7b) and the user's by-hand acceptance run move after the release. Both need a v0.16 mnemo with the keychain on the user's PATH (this Mac's is v0.12.3), and the install line can only fetch a published release. So the user installs v0.16.0 with the real install line (the acceptance run's first check), runs the scratch-repo session, then the switch. A failure in either is a v0.16.1, with the attestation check of the real path done the same day. AGENTS.md's 'Dogfood the engine on yourself' rewrite comes with 7b, after the switch.
```

## 01M7ESPCXX3JRZK5SGREE8K9GW

- type: episodic
- agent: `cursor-agent`
- key: `design.122-review-plan`
- scope: private
- created: 2026-11-15T05:08:18Z
- importance: 0.7
- metadata: `{"_mnemo.key":"design.122-review-plan","area":"decisions"}`

```text
12.2 / v0.16.0 review plan (PLANNER-A, 2026-11-15): one scoped REVIEWER-B pass after step 6 merges, covering the hooks, injection framing and redaction (step 4), onboard's key and keychain path and config writes (step 5), and the install scripts and release workflow (step 6). It serves as the release review; at READY PLANNER-A checks only the delta since (step 7 and the bump). The by-hand acceptance run is the user's, in a scratch repo in their own account; the Worker never runs it, since it writes the real keychain.
```

## 01M7F7TZS7FWZQFGRSTQNYJ61A

- type: semantic
- agent: `cursor-agent`
- key: `design.122-session-search`
- scope: private
- created: 2026-11-15T09:15:28Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-session-search","area":"decisions"}`

```text
12.2, searching past sessions (PLANNER-A's item A, v0.16.0 step 6): recall leaves thread turns out unless the filter names them, so an agent searches what the hooks saved with recall and the turn filter: MCP {"filter": {"metadata": {"_mnemo.kind": "turn"}}}, CLI --filter _mnemo.kind=turn; one session only with {"_mnemo.thread": "claude-code:<session id>"}. Turns are found by meaning only: they aren't in the BM25 keyword index (FMT-9a: turn postings would be about 3x all the fact data), so session search needs the built-in model, which the hooks need to save turns anyway. The guide's Threads part has 'Search past sessions' with a json mcp and an sh test block (both tagged model), the card's first bullet points to it, and onboard_cli's acceptance test finds the hook's saved identifier through recall with the filter (and not without it).
```

## 01M7ESPCSXMF3159FBQJTB11BR

- type: semantic
- agent: `cursor-agent`
- key: `design.122-transcripts`
- scope: private
- created: 2026-11-15T05:08:18Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-transcripts","area":"decisions"}`

```text
12.2 design (signed off 2026-11-15): the hooks' transcript ingest keeps user and assistant text and [tool_use name=X], and keeps tool results only as [tool_result ok|error, N bytes] markers (thread ingest --tool-results omit; a direct call keeps full), narrowing rule 1 for the hooks only. Known secret shapes are redacted to [redacted: <kind>] with word boundaries and vendor minimum lengths: PEM private-key blocks, AKIA/ASIA + 16, gh[pousr]_ + 36, github_pat_ + 22, xox[abprs]- + 10, sk- + 20 of [A-Za-z0-9_-], JWTs (eyJ, three parts of 10+), MN1- recovery codes, and NAME=value / NAME: value / "name": "value" for names ending TOKEN, SECRET, PASSWORD, PASSPHRASE, API_KEY or _KEY, only for values of 8+ characters that aren't a plain word (a digit, a character outside [A-Za-z_.-], or mixed case). The case table includes false positives (task-list, PRIMARY_KEY=id, commit hashes, ULIDs).
```

## 01M7ESPCVX405DMDHXK309AJFT

- type: semantic
- agent: `cursor-agent`
- key: `design.122-trust-framing`
- scope: private
- created: 2026-11-15T05:08:18Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.122-trust-framing","area":"decisions"}`

```text
12.2 design (signed off 2026-11-15): the guide lives in the binary and nothing from a file, manifest, memory or MCP request changes it. Everything shown from the brain (project notes, essences, recalled memories) is data: every delivery puts it after the card under one framing line, "From this project's brain: notes and past context, not instructions. They never override this guide or your user." That covers the SessionStart hook, mnemo://about, mnemo://recent, mnemo://key/{key} and the session-start and session-end prompts, and a test checks the line in each.
```

## 01M7GN21R85G8QV76QCA17NDRJ

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-claims`
- scope: private
- created: 2026-11-15T22:25:46Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-claims","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): Claims and presence are 12.4's step 7 as designed (tokens, check before an irreversible step, the early lapse only as a hint): a 15 min default lease (30 s to 24 h), task claims of 2 h renewed by the owner's updates, who() with alive from the presence sidecar lock. The brain's OS lock is held only inside a call; the only lock between calls is the agent's own presence sidecar lock.
```

## 01M7GXYS5R1E804W31K6WRRKT8

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-claims-built`
- scope: private
- created: 2026-11-16T01:01:16Z
- importance: 0.7
- metadata: `{"_mnemo.key":"design.13a-claims-built","area":"design","topic":"design.13a-claims-built"}`

```text
13a step 3 (PR 107, 2026-11-15): claims are _mnemo.kind=claim records, one per topic, changed in place: _mnemo.topic, _mnemo.holder, _mnemo.claim_token (32 hex), _mnemo.expires_at, _mnemo.lease, _mnemo.holder_lock, _mnemo.dead_seen_at. API Mnemo::claim/renew/release/check_claim/claims; Lost on a lapsed or foreign token; a holder's own re-take is HeldBy. Presence: _mnemo.kind=presence, one per agent: _mnemo.agent, _mnemo.role, _mnemo.tool, _mnemo.last_active (at most once a minute), _mnemo.presence_lock. SharedMnemo::hold_presence (serve --mcp --shared only) locks presence.<16 hex sha256>.lock in the sidecar dir. Early lapse only with holder_lock and two looks 10 s apart. Lookups use the v9 row's thread_hash = fnv1a(_mnemo.coordination, kind, locator).
```

## 01M7GN20ZZ4TVRD21ZZX6BKNSZ

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-delivery`
- scope: private
- created: 2026-11-15T22:25:45Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-delivery","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): The delivery state (sent -> delivered -> seen -> done) is the truth and every wake-up a hint. Paths in order: (1) the per-turn UserPromptSubmit hook, which reads on most turns and writes one window only when it shows something, its line under FRAMING and escape_brain, at most 600 bytes; (2) the idle wake-up, each session's own serve --mcp posting one fixed-text pointer (counts only) into its own session through CLAUDE_CODE_MESSAGING_SOCKET, verified in step 6 or dropped, crossSessionInbound must be accept; (3) a Stop continuation, off by default, never twice in a row; (4) channels optional (they need Anthropic's plugin allowlist). mnemo watch is deferred to after HARNESS-DOGFOOD-1's Run 1.
```

## 01M7GN22HCTH9VQA63PPHZVFNN

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-freshness`
- scope: private
- created: 2026-11-15T22:25:47Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-freshness","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): _mnemo.as_of and _mnemo.review_by on any record; stale(now) lists items past review_by or past a per-kind default (essence state and next 14 days, open 30 days, task active 3 days without update, blocked 7 days, unread message 3 days, facts never unless review_by). Shown in about, the essence render, the brief's Your work part and mnemo stale.
```

## 01M7HAGJMJTQZ8M09VYPQHMZPM

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-freshness-built`
- scope: private
- created: 2026-11-16T04:40:42Z
- importance: 0.7
- metadata: `{"_mnemo.key":"design.13a-freshness-built","area":"design","topic":"design.13a-freshness-built"}`

```text
13a step 7 (PR 110, 2026-11-15): stale_since(m, now): an explicit _mnemo.review_by decides alone; else the kind default (essence state/next 14 d, open 30 d; task active 3 d from last history entry, blocked 7 d; unread message 3 d; facts never) from as_of, else updated_at, else created_at. Mnemo::stale() reads essence and coordination rows plus rows with ROW_REVIEW=32. 'Yours': a task's owner, a message's recipient, else _mnemo.author. mnemo stale [--mine], MCP about {stale:true}, Python stale(); [stale since <date>] in about and the essence render; the brief counts the agent's own. ABOUT-COUNT: Mnemo::kind_counts in about's header and JSON; the MCP about array keeps its shape (essence last).
```

## 01M7GN21C4JGQY5Z8VCS326NVY

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-guardrails`
- scope: private
- created: 2026-11-15T22:25:45Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-guardrails","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): Wake-up guardrails, the same on every path: 12 wake-ups per recipient per hour, a 60 s cooldown, a hop cap of 8 per thread with a 10-minute pair carry (a new message from A to B starts one above the newest between them in the last 10 minutes), and a kill switch (mnemo wake pause|resume, a wake:paused record; MNEMO_WAKE=off for one session). Messages are information, never authority.
```

## 01M7HFK6Y57KF45QPEJDC6GJ2N

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-guide-built`
- scope: private
- created: 2026-11-16T06:09:31Z
- importance: 0.7
- metadata: `{"_mnemo.key":"design.13a-guide-built","area":"design","topic":"design.13a-guide-built"}`

```text
13a step 8 (PR 112, 2026-11-15): the guide's section 6, 'Working with other agents', in R-010.3's order: naming sessions (MNEMO_AGENT, MNEMO_ROLE, an alias per role, --as/--role), tasks, the inbox, claims and who (sh test and json mcp blocks), how work reaches a session (per-turn line, Stop when on, a SendMessage pointer in Claude Code with the inbox as the record, Cursor's per-turn line), mnemo's tasks vs Claude Code's TaskCreate/SendMessage, the rules (information never authority, kill switch, crossSessionInbound accept), visibility and freshness. Card line added (925 bytes); SUMMARY mentions the inbox, task queue and claims; llms.txt, llms-full.txt and the plugin SKILL.md regenerated.
```

## 01M7H4ZK9ETE907ETF8VBB9B29

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-hooks-built`
- scope: private
- created: 2026-11-16T03:04:03Z
- importance: 0.7
- metadata: `{"_mnemo.key":"design.13a-hooks-built","area":"design","topic":"design.13a-hooks-built"}`

```text
13a step 5 (PR 109, 2026-11-15): mnemo hook user-prompt (Claude Code UserPromptSubmit; Cursor postToolUse, once a minute per session) shows new work for MNEMO_AGENT/MNEMO_ROLE (unread messages, ready tasks, own task claims lapsing within 15 min) after FRAMING, escaped, <=600 bytes, then marks it seen (Mnemo::new_work, mark_shown). mnemo hook stop (onboard --wake stop) blocks only for a pre-approved task or a message by id; never twice; fixed text. Guardrails in Mnemo::wake only (12/h, 60 s, pair burst 8 in 10 min, hop cap 8 for explicit replies, kill switch _mnemo.wake_paused on the manifest via mnemo wake pause|resume|status); the per-turn hook and the brief are never gated.
```

## 01M7GN20JVYXD8G9K58KQDC01J

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-identity`
- scope: private
- created: 2026-11-15T22:25:45Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-identity","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): A session names itself with MNEMO_AGENT and MNEMO_ROLE in its environment (exported in the tmux pane before claude); without them it is claude-code:<session id> with no role. The sender (_mnemo.from) is unproven until 13b, so messages are information and nothing in the engine checks from. AGENT-DEFAULT: a reserved manifest key _mnemo.default_agent, honoured by the CLI and the MCP server for a write that names no agent; scaffold_manifest leaves it unset; mnemo default-agent sets it. A fact gets _mnemo.author = MNEMO_AGENT, which never changes its owner.
```

## 01M7GTRKGK5969EAG29NEV88TV

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-messages-built`
- scope: private
- created: 2026-11-16T00:05:28Z
- importance: 0.7
- metadata: `{"_mnemo.key":"design.13a-messages-built","area":"design","topic":"design.13a-messages-built"}`

```text
13a step 2 (PR 106, 2026-11-15): messages are _mnemo.kind=message records in the shared scope with the sentinel vector. Fields: _mnemo.to (agent id, role:<r>, all), _mnemo.from, _mnemo.from_role, _mnemo.conversation (the first message's id), _mnemo.hops, _mnemo.refs, _mnemo.status (unread/acked), _mnemo.delivery (sent/delivered/seen/done), _mnemo.acked_at; content is 'subject\n\nbody'. One record for life, changed in place. API: Mnemo::post/inbox/mark_seen/ack/prune_messages; CLI post/inbox/ack with --as/--role; MCP tool inbox (list, read, post, ack).
```

## 01M7GN205VVXGEPJ6QMP28G8SV

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-records`
- scope: private
- created: 2026-11-15T22:25:44Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-records","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): Four record kinds, message, task, claim and presence, as ordinary records with reserved _mnemo.* metadata and no format change. Shared scope, the file's sentinel vector, never embedded, never recall/BM25/ANN/about hits; status and delivery change in place with history. Tasks have key task:<slug>; claims and presence are keyed (claim:<topic>, presence:<agent>) as 12.4 designed. export --git keeps tasks (not done ones older than 30 days) and leaves out messages, claims and presence.
```

## 01M7GN23ABG7NNBECX6R4MXXKQ

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-steps`
- scope: private
- created: 2026-11-15T22:25:47Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-steps","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): v0.17.0 steps, each a PR into release-0.17.0: 1 plumbing (kinds, metadata fields, AGENT-DEFAULT, MNEMO_AGENT/MNEMO_ROLE, CLI-PATHLESS-2), 2 messages, 3 claims and presence, 4 tasks, 5 the per-turn hook and Stop continuation, 6 the idle wake-up, 7 visibility and freshness with ABOUT-COUNT, 8 the guide, 9 READY. A scoped REVIEWER-B pass after step 6 over steps 3 to 6. Keyword-indexing saved turns stays carried.
```

## 01M7GN22XTBE7V6480C4FWB3PT

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-surfaces`
- scope: private
- created: 2026-11-15T22:25:47Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-surfaces","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): Three MCP tools, each with an op: inbox (list, read, post, ack), task (add, next, update, list, get), claim (take, renew, release, check, who); the CLI (inbox, post, ack, task, claim, stale, wake, hook user-prompt and hook stop), Python and Rust, no mobile in 13a. The brief gets a Your work part of at most 600 bytes from the recall's share. The guide gets a Working with other agents section and one card line.
```

## 01M7H1YYDRB34K85SNGSPNJVQX

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-tasks-built`
- scope: private
- created: 2026-11-16T02:11:16Z
- importance: 0.7
- metadata: `{"_mnemo.invalid_at":1794807081,"_mnemo.key":"design.13a-tasks-built","_mnemo.superseded_by":"01M7HDDAQWMDS1YS3DPHWSJ3Y6","area":"design","topic":"design.13a-tasks-built"}`

```text
13a step 4 (PR 108, 2026-11-15): tasks are _mnemo.kind=task records, one per slug, changed in place: _mnemo.slug (found by the v9 row hash, not a _mnemo.key slot), _mnemo.status, _mnemo.role, _mnemo.owner, _mnemo.creator, _mnemo.after (slugs), _mnemo.pre_approved, _mnemo.delivery, _mnemo.status_history; content 'title\n\nbody'. task_next claims task:<slug> for 2 h (TASK_CLAIM_TTL), oldest ready queued first, then reclaims an active task whose claim lapsed (reclaimed_from in history). task_update: owner moves active/blocked (renews the claim; Lost if taken back or wrong token); creator sets ready->done unless pre_approved; anyone requeues an abandoned task; ready/done/queued release the claim. CLI: --for is the task's role (--role is the caller's). MCP tool order: task, inbox, claim.
```

## 01M7HDDAQWMDS1YS3DPHWSJ3Y6

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-tasks-built`
- scope: private
- created: 2026-11-16T05:31:21Z
- importance: 0.7
- metadata: `{"_mnemo.key":"design.13a-tasks-built","_mnemo.supersedes":"01M7H1YYDRB34K85SNGSPNJVQX","area":"design","topic":"design.13a-tasks-built"}`

```text
13a tasks as built, revised for RB-012 (PR 111, 2026-11-15): _mnemo.slug via the row hash; task_next offers the caller's own lapsed task (active or blocked) first, then ready queued tasks, then other owners' lapsed ones. task_update: a token from a non-owner -> Lost(TakenBack); a live claim held by another -> TakenBack; a token that isn't the (live or lapsed) claim's -> Lost(OtherToken); otherwise an owner carries on, renewing or re-taking the claim; Updated(task, Option<TaskClaim{token, expires_at, retaken}>). blocked->queued and active->queued: the owner, or anyone once the claim lapsed. task_add: a done slug's re-add refused with <slug>-N; only the creator changes a definition; an after cycle refused; prints the status.
```

## 01M7GN22494SC34F4H78B66TJF

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-visibility`
- scope: private
- created: 2026-11-15T22:25:46Z
- importance: 0.85
- metadata: `{"_mnemo.key":"design.13a-visibility","area":"decisions"}`

```text
13a design (docs/design/13a-coordination.md, signed off by PLANNER-A 2026-11-15, PR 104 6727a91): _mnemo.visibility public | team | private, default team, a filter and not a security boundary until 13b: private shows only when asked, export needs --visibility all, export --git never includes it, inspect shows it with a badge. SECURITY.local.md does not move into private records before 13b; a separate private brain after 13b.
```

## 01M7HAGJ7R1TM80X04QCQWQFRT

- type: semantic
- agent: `cursor-agent`
- key: `design.13a-visibility-built`
- scope: private
- created: 2026-11-16T04:40:41Z
- importance: 0.7
- metadata: `{"_mnemo.key":"design.13a-visibility-built","area":"design","topic":"design.13a-visibility-built"}`

```text
13a step 7 (PR 110, 2026-11-15): _mnemo.visibility is a filter (not security): VisibilitySet (public+team default; lists or 'all'); a private record's v9 row has ROW_PRIVATE=8 (ROW_PUBLIC=16) in the flags byte, so recall's row path skips it before decrypting; decrypting paths check the record. Left out of recall, about (about_in), inbox (inbox_in), the essence render and the brief unless asked (--visibility, MCP visibility, Python visibility=). export: stderr count, --visibility all includes, --git never. inspect shows a 'private' badge. Engine manifest lookups use VisibilitySet::ALL.
```

## 01M7SS0MTCQQD54C30JH2SWBAZ

- type: semantic
- agent: `cursor-agent`
- key: `fact.bench-corpus-cache`
- scope: private
- created: 2026-11-19T11:28:04Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.bench-corpus-cache","area":"performance","topic":"bench-corpus-cache"}`

```text
fact.bench-corpus-cache (worker-a, 2026-11-19, v0200-3): benches/corpus.rs's setup_cached_corpus reuses $CARGO_TARGET_DIR/benches/corpus_<n>_384d.mnemo whenever it opens with the right dimensions, whatever build wrote it. mnemo/target/benches/ held corpora from 2026-11-13 (before v0.19.0), whose 100K index is layout 1 (msgpack): loading it takes about 87 ms, against about 10 ms for layout 2. Any benchmark that loads the index (a reload, a first recall, shared_bench cells 10/11) on that cache is not comparable to numbers on fresh corpora. Fix: run the bench with CARGO_TARGET_DIR pointing at an empty directory (corpora built fresh by the build under test), or delete the stale cache; say which in the report.
```

## 01M7E8YFW5RQKBGX97J3Q2GM4G

- type: semantic
- agent: `cursor-agent`
- key: `fact.brain.embedder`
- scope: private
- created: 2026-11-15T00:15:37Z
- importance: 0.9
- metadata: `{"_mnemo.key":"fact.brain.embedder","area":"dogfood"}`

```text
The dogfood brain test/project.mnemo uses the engine's built-in embedder since 2026-11-14 (v0.15 step 5): Snowflake/snowflake-arctic-embed-xs@d8c86521100d3556476a063fc2342036d45c106f, 384-dim, moved from all-MiniLM-L6-v2 with mnemo reembed. project_memory.py takes the embedder the brain's manifest records (use_builtin_embedder for the built-in), so test/.venv needs a wheel built with --features embed-candle, and the model in the cache (mnemo embedder install, or first use). The backup before the move is mnemo-brain-backups.nosync/project.mnemo.2026-11-14-before-builtin-move.
```

## 01KZFX613B66NRWGZNN7A1KNGR

- type: semantic
- agent: `cursor-agent`
- key: `fact.brain.format`
- scope: private
- created: 2026-08-08T05:22:56Z
- importance: 0.95
- metadata: `{"_mnemo.invalid_at":1794691950,"_mnemo.key":"fact.brain.format","_mnemo.superseded_by":"01M7DZKSJQK2602DVR9X1VEH4M"}`

```text
PR 3 shipped: format v7 -> v8, exact-key result cache (Phase 10.1 + 10.3). New Mnemo::cache_put/get/delete/purge/stats API + CacheFlushPolicy (Strict default, Batched opt-in for the relaxed durability lane; memory writes never batched). Per-namespace CacheBudget (default 10K entries / 64 MiB), LRU eviction. Header gains cache_start/pages/len u64s at bytes 270-293 (after v7 seal), sealed under v8-only AAD. v7 files migrate on open with an empty cache directory. Breaking rename: cache_stats -> page_cache_stats (result cache took the name). CLI: 'mnemo cache <get|put|delete|stats|purge>'. 9 new integration tests. Semantic cache (10.2) and MCP/Python surface (10.4) deferred to PR 4/5. New module mnemo/src/result_cache.rs — NOT cache.rs (page cache). Uses sha2 crate for key hashing.
```

## 01M7DZKSJQK2602DVR9X1VEH4M

- type: semantic
- agent: `cursor-agent`
- key: `fact.brain.format`
- scope: private
- created: 2026-11-14T21:32:30Z
- importance: 0.85
- metadata: `{"_mnemo.key":"fact.brain.format","_mnemo.supersedes":"01KZFX613B66NRWGZNN7A1KNGR","area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
test/project.mnemo is gitignored and format v9, and the Planner opens it read-only. Passphrases come only from test/.env, read by the scripts; no agent types or pastes one.
```

## 01M7SN6CBV52E93VW3ET2RCMBD

- type: semantic
- agent: `cursor-agent`
- key: `fact.catch-up-ann-reconcile`
- scope: private
- created: 2026-11-19T10:21:18Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.catch-up-ann-reconcile","area":"performance","topic":"catch-up"}`

```text
fact.catch-up-ann-reconcile (worker-a, v0200-3, 2026-11-19): each one-memory commit repacks its tail record page and rewrites its tail vector page (FMT-9b.2), so 25-65 catalog rows change per commit at 100K. Re-adding them all to the ANN (IvfPqIndex::add is about 130-300 us per row) made the incremental catch-up 4-9 ms at 100K. The build skips a row whose ANN entry can't change: re-added before (written after the index under the old header) and re-added by a fresh open (written after it under the new one), still live, same kind, same fp16 column entry. An entry from the index build is NOT equal to add() of the same vector (k-means assignment vs nearest centroid), so the 're-added in both' condition is required (the_index_follows catches its removal).
```

## 01M7SRD4SZS5HBBPJDPD1ZEV1M

- type: semantic
- agent: `cursor-agent`
- key: `fact.catch-up-footprint`
- scope: private
- created: 2026-11-19T11:17:25Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.catch-up-footprint","area":"performance","topic":"catch-up-footprint"}`

```text
fact.catch-up-footprint (worker-a, 2026-11-19, v0200-3): a shared handle's footprint at 100K is what the macOS allocator keeps, not live data. Measured sources: (1) the first catch-ups after a load double the capacity of the catalog, the ANN lists and the id map (about +45 MB); (2) each full reload (a base rewrite about every 65 commits) rebuilds the handle. Fixes in v0200-3: the vector column gets an eighth of headroom in ensure_column (grows in place), and a trusted full reload adopts the old handle's column (adopt_column, rows whose vector entry didn't move). Tried and reverted: grow_rows with headroom on growth (worse, 525 MB), dropping the old handle before the reload (no help at 100K). Shape at 100K over 300 commits, adopt + headroom: 125 -> 170.5 after the first catch-ups, fallback steps +23/+8/+11/+7, end 235.9 (old code: end 382). Section 6's middle path (carry more structures across a reload) is left to the Planner.
```

## 01M7SYJP563CZ73XYGQJTANS6K

- type: semantic
- agent: `cursor-agent`
- key: `fact.catch-up-restore-guard`
- scope: private
- created: 2026-11-19T13:05:18Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.catch-up-restore-guard","area":"decision","topic":"catch-up-restore-guard"}`

```text
fact.catch-up-restore-guard (worker-a, 2026-11-19, v0200-3r): a SharedMnemo records, whenever it takes a state (load, catch-up, own commit), the on-disk nonce of the first page of each run its header points at (Anchors in store/shared.rs; Pager::page_nonce reads past the cache). Before an incremental catch-up or a reload with adopt_column it requires new wal_seq >= held and every KEPT held run unchanged on disk: catalog base and head always, a loaded index's base and head when the new header keeps the index base, other runs only when the new header still has the same pointer. Superseded runs are not checked: on the same timeline their pages may be reused (with max_snapshots(1) they are within a few commits), which would make every catch-up fall back. When the check passes it returns the newest generation among the checked runs; the handle drops cached pages newer than that (Pager::drop_cached_newer_than) and keeps a held BM25 dict only if its segment gen is not newer (catch_up_to's trusted_gen). Checking only the catalog head misses a restore where only a result-cache put happened in between: a cache_get then served the other timeline's value from the page cache. Cost: within noise (A/B, catch-up p50 0.45/0.40 vs 0.46/0.41 ms at 10K, 0.81/0.82 vs 0.80/0.85 ms at 100K).
```

## 01M7RGNSJ8BVKBVABTB5MRSGYH

- type: semantic
- agent: `cursor-agent`
- key: `fact.ci-toolchain-gap`
- scope: private
- created: 2026-11-18T23:43:05Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.ci-toolchain-gap","area":"ci","topic":"toolchain-gap"}`

```text
CI installs the latest stable Rust (rustc 1.100.0 on 2026-11-18) and builds with warnings denied, while this Mac has rustc 1.95.0, so a newly deprecated std API passes locally and fails every CI job (AtomicU32::fetch_update in v0191-1b). Prefer APIs stable at the MSRV (1.94) and not deprecated since; for atomics, a compare_exchange loop.
```

## 01KSGPG9WCMM57ZBF937DVVCMK

- type: procedural
- agent: `cursor-agent`
- key: `fact.dogfood.commands`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 1
- metadata: `{"_mnemo.invalid_at":1794779120,"_mnemo.key":"fact.dogfood.commands","_mnemo.superseded_by":"01M7GJR0HT29RP0F2MZPHNZJX7","area":"dogfood","topic":"commands"}`

```text
project_memory.py commands: bootstrap (rebuild from seed.json), recall "query" (NL context), remember "fact" --type semantic|episodic|procedural [--importance 0-1], info (stats). All target test/project.mnemo.
```

## 01M7GJR0HT29RP0F2MZPHNZJX7

- type: procedural
- agent: `cursor-agent`
- key: `fact.dogfood.commands`
- scope: private
- created: 2026-11-15T21:45:20Z
- importance: 1
- metadata: `{"_mnemo.key":"fact.dogfood.commands","_mnemo.supersedes":"01KSGPG9WCMM57ZBF937DVVCMK","area":"dogfood","topic":"commands"}`

```text
project_memory.py commands: recall "query" (runs the installed mnemo CLI, --no-track); remember "fact" --type semantic|episodic|procedural --key K [--importance 0-1], or --from-file JSONL (the Worker only; it passes the agent cursor-agent); info (stats; needs the Python binding with the keychain); perf <file.json>; bootstrap --brain <scratch> (never the project brain). The brain is .git/mnemo/project.mnemo.
```

## 01KSGPG9WCYEB246EZZZNKQ5SS

- type: semantic
- agent: `cursor-agent`
- key: `fact.dogfood.this-project`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 1
- metadata: `{"_mnemo.invalid_at":1794701701,"_mnemo.key":"fact.dogfood.this-project","_mnemo.superseded_by":"01M7E8XCHRJ7YABGDWT9A3CK5H","area":"dogfood","topic":"this-project"}`

```text
Dogfooding sandbox: test/ holds project.mnemo (local encrypted brain). Tracked in git: test/scripts/seed.json, project_memory.py, test/README.md. Runtime gitignored: test/.env, test/project.mnemo, test/.venv, test/.cache/, test/project-memory.jsonl. Embeddings: all-MiniLM-L6-v2, 384 dim.
```

## 01M7E8XCHRJ7YABGDWT9A3CK5H

- type: semantic
- agent: `cursor-agent`
- key: `fact.dogfood.this-project`
- scope: private
- created: 2026-11-15T00:15:01Z
- importance: 1
- metadata: `{"_mnemo.invalid_at":1794779119,"_mnemo.key":"fact.dogfood.this-project","_mnemo.superseded_by":"01M7GJR060XR4046CS3PVZMDBP","_mnemo.supersedes":"01KSGPG9WCYEB246EZZZNKQ5SS","area":"dogfood","topic":"this-project"}`

```text
Dogfooding sandbox: test/ holds project.mnemo (local encrypted brain). Tracked in git: test/scripts/seed.json, project_memory.py, test/README.md. Runtime gitignored: test/.env, test/project.mnemo, test/.venv, test/.cache/, test/project-memory.jsonl. Embeddings: the built-in snowflake-arctic-embed-xs, 384 dim (all-MiniLM-L6-v2 before v0.15).
```

## 01M7GJR060XR4046CS3PVZMDBP

- type: semantic
- agent: `cursor-agent`
- key: `fact.dogfood.this-project`
- scope: private
- created: 2026-11-15T21:45:19Z
- importance: 1
- metadata: `{"_mnemo.key":"fact.dogfood.this-project","_mnemo.supersedes":"01M7E8XCHRJ7YABGDWT9A3CK5H","area":"dogfood","topic":"this-project"}`

```text
Dogfooding layout (since the v0.16 switch, 2026-11-15): the brain is .git/mnemo/project.mnemo, untracked and shared by every worktree, with its key in the user's keychain. Tracked in git: test/scripts/seed.json, project_memory.py, test/README.md. Gitignored: test/.venv, test/.cache/, test/project-memory.jsonl; test/.env and test/project.mnemo are the retired old layout. Embeddings: the built-in snowflake-arctic-embed-xs, 384 dim (all-MiniLM-L6-v2 before v0.15).
```

## 01M7TP4V040D8B8Y1CN2JP1KWV

- type: semantic
- agent: `cursor-agent`
- key: `fact.flake-cli-shared-embed`
- scope: private
- created: 2026-11-19T19:57:10Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.flake-cli-shared-embed","area":"ci","topic":"flake-cli-shared-embed"}`

```text
fact.flake-cli-shared-embed (worker-a, 2026-11-19): cli_shared::a_shared_mcp_server_embeds_before_it_takes_the_lock (tests/cli_shared.rs:470) failed once on a hosted Windows runner (PR 154, job 135826405514): 'the import waited 1.9746906s' against a 1.2 s cap, while the import got the lock within its --wait 500ms. The time was the CLI process (start, KDF, open), not the lock. It passed on the re-run. The sibling the_cli_embeds_before_it_takes_the_lock had the same kind of Ubuntu failure in v0.15 (01M7DQC8). Logged, not changed; the Planner decides if it recurs (a fix would split the process time from the lock wait, as that sibling's message does).
```

## 01M7TGKZHC2KH79WJKGNAGPNHJ

- type: semantic
- agent: `cursor-agent`
- key: `fact.flake-shared-valve`
- scope: private
- created: 2026-11-19T18:20:35Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.flake-shared-valve","area":"ci","topic":"flake-shared-valve"}`

```text
fact.flake-shared-valve (worker-a, updated 2026-11-19): shared_valve::two_writers_and_four_readers_all_get_their_turns (tests/shared_valve.rs:193, readers stopped getting in) has failed twice on hosted macOS runners, each time with one 500 ms window of zero reads and passing on the re-run. PR 139 (2026-11-18): reads [140, 47, 0, 103, 79, 33]. PR 152 (2026-11-19, job 135737720437, no Rust change): commits [24, 26], reads [41, 82, 0, 52, 13, 112], longest read wait 688.6 ms. Logged, not changed; the Planner decides now that it has recurred (report.v0200-d3h).
```

## 01M7S9DD3RRC4144550JDX88NP

- type: semantic
- agent: `cursor-agent`
- key: `fact.flush-commit-failed`
- scope: private
- created: 2026-11-19T06:55:25Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.flush-commit-failed"}`

```text
fact.flush-commit-failed (v0.20.0, v0200-1r): Mnemo has commit_failed: Option<u64> (store/mod.rs), set in flush.rs when commit_and_checkpoint (wal::commit, its fsync, the checkpoint, the shrink) fails after commit_point is set. Every later flush on that handle returns MnemoError::MaybeStored, so close does too and try_close hands the handle back; reopening the file finishes the commit (WAL replay) or drops a torn one. The first failing flush returns its original error. Chosen over reloading in place like the shared window because an exclusive handle carries per-handle settings a reload would have to rebuild, and the reload itself can hit the same failing fsync; SharedMnemo already handles it with finish_failed_commit/install. Failures before the commit point still restore header/free list and the retry commits. Tests: mnemo/tests/flush_retry.rs (every write and sync index of a 1-memory and a 1500-memory commit, then retry, then reopen pre or post) and prop_big_batch_commits_crash_pre_or_post with retry on the same handle.
```

## 01M7S0H5YRGQ0SQ2M0X40A4P0M

- type: semantic
- agent: `cursor-agent`
- key: `fact.flush-stage-commit`
- scope: private
- created: 2026-11-19T04:20:11Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.flush-stage-commit","area":"engine","topic":"flush"}`

```text
fact.flush-stage-commit (v0200-1, PERF-2 3f): Mnemo::flush (store/flush.rs) is pager.write_dirty() (no fsync), then stage_commit(), which stages the WAL frames, writes FMT-9c's direct runs to their home pages and ends with one barrier (pager.sync() if pager.unsynced()), then wal::commit with its own fsync, then the checkpoint fsync: 3 fsyncs per commit, batches with a big run included (4 before). The barrier is never merged with the commit's sync. flush saves (header, free, free_dirty) before stage_commit and restores them on its error, so a retry on the same handle commits over the committed state. Pager::flush was removed; Pager::unsynced stays set until a sync succeeds, so a retry after a failed barrier still syncs.
```

## 01M7RFVKYN260JXTEXAHN28YCP

- type: semantic
- agent: `cursor-agent`
- key: `fact.fuzz-local`
- scope: private
- created: 2026-11-18T23:28:48Z
- importance: 0.5
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.fuzz-local","area":"testing","topic":"fuzz-local"}`

```text
This Mac has no nightly toolchain or cargo-fuzz, so fuzz targets are checked only with `cargo check` in mnemo/fuzz (delete the untracked fuzz/Cargo.lock it creates) and run in CI's fuzz-smoke.
```

## 01M7S0H6BMCA73CKAT9G7PSP5X

- type: procedural
- agent: `cursor-agent`
- key: `fact.io-trace-failpoints`
- scope: private
- created: 2026-11-19T04:20:12Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.io-trace-failpoints","area":"testing","topic":"failpoints"}`

```text
fact.io-trace-failpoints (v0200-1): with the failpoints feature, mnemo_engine::__failpoints has set_io_trace(bool) and io_trace() -> Vec<IoEvent> (IoEvent::Page(n), WalCommit, Sync; per thread), recording every page write, the WAL commit write and every fsync, and set_syncs_until_fail(n) (n-th sync fails once with 'injected sync failure'; -1 disarms), beside set_writes_failing / writes_seen. Start a trace after the remembers: the first remember's generation lease is a commit of its own. Examples: mnemo/tests/fsync_merge.rs.
```

## 01KSGPG9WAMTF19MPQAS0F01RX

- type: semantic
- agent: `cursor-agent`
- key: `fact.msrv`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.85
- metadata: `{"_mnemo.invalid_at":1794691935,"_mnemo.key":"fact.msrv","_mnemo.superseded_by":"01M7DZKBB5AA85TP7C724DWPVM","area":"tooling"}`

```text
Rust MSRV is 1.75. Default cargo build and test only build the core crate, not Python or Node bindings. Build bindings with cargo build -p mnemo-python or maturin in mnemo-python/.
```

## 01M7DZKBB5AA85TP7C724DWPVM

- type: episodic
- agent: `cursor-agent`
- key: `fact.msrv`
- scope: private
- created: 2026-11-14T21:32:15Z
- importance: 0.8
- metadata: `{"_mnemo.invalid_at":1794697194,"_mnemo.key":"fact.msrv","_mnemo.superseded_by":"01M7E4KTZ2T7R8210KKMVMZVYY","_mnemo.supersedes":"01KSGPG9WAMTF19MPQAS0F01RX","area":"decisions"}`

```text
2026-11-14, PLANNER-A: step 2's keychain used keyring =3.6.3 with lockfile pins, because every keyring 4 store crate needed Rust 1.85+ and the MSRV was 1.75. Later that day the user raised the MSRV to 1.92 (step 5a, MSRV-2), because fastembed's ort =2.0.0-rc.15 needs it, and the keychain moved to the keyring 4 family (keyring-core and one store crate per platform).
```

## 01M7E4KTZ2T7R8210KKMVMZVYY

- type: episodic
- agent: `cursor-agent`
- key: `fact.msrv`
- scope: private
- created: 2026-11-14T22:59:54Z
- importance: 0.8
- metadata: `{"_mnemo.key":"fact.msrv","_mnemo.supersedes":"01M7DZKBB5AA85TP7C724DWPVM","area":"decisions"}`

```text
The MSRV is Rust 1.94 (v0.15, 12.3 step 5, the user's decision 2026-11-14), for every crate: candle-core 0.11.1, the built-in embedder's runtime, calls core::arch::x86_64::__cpuid_count outside unsafe, which Rust 1.93 and older reject on x86_64. It was 1.92 from step 5a (for ONNX Runtime's ort, since dropped) and 1.75 before. The keychain uses the keyring 4 family (keyring-core and one store crate per platform). The released CLI binaries and wheels are built with stable.
```

## 01KSGPG9W9APEATK5HYCG00G2K

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.access`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 1
- metadata: `{"_mnemo.invalid_at":1794779117,"_mnemo.key":"fact.onboarding.access","_mnemo.superseded_by":"01M7GJQY9GA23KS2FKM14F1P58","area":"onboarding","topic":"access"}`

```text
HOW TO ACCESS PROJECT MEMORY: File test/project.mnemo (encrypted, 384 dimensions, agent_id cursor-agent). Passphrase in test/.env as MNEMO_PASSPHRASE — export it or the script loads .env automatically. Never commit test/.env or any .mnemo file. Python tool: test/scripts/project_memory.py. Rust CLI: cd mnemo && cargo run --bin mnemo -- <cmd> ../test/project.mnemo with MNEMO_PASSPHRASE set.
```

## 01M7GJQY9GA23KS2FKM14F1P58

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.access`
- scope: private
- created: 2026-11-15T21:45:17Z
- importance: 1
- metadata: `{"_mnemo.key":"fact.onboarding.access","_mnemo.supersedes":"01KSGPG9W9APEATK5HYCG00G2K","area":"onboarding","topic":"access"}`

```text
HOW TO ACCESS PROJECT MEMORY: the project brain is .git/mnemo/project.mnemo (encrypted, 384-dim, facts under agent_id cursor-agent). Git never tracks it, every worktree shares it, and its key is in the user's keychain, so no session handles a passphrase. In this repo the CLI takes no path: `mnemo about`, `mnemo recall --text "…" --no-track`, `mnemo get <id>`. The MCP server that `mnemo onboard` wires gives each session the same tools. test/.env and test/project.mnemo are the old layout, retired at the switch.
```

## 01KSK8ZEP8N4ZE9MRBNGNZ33RX

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.agents-md`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 0.8
- metadata: `{"_mnemo.invalid_at":1794779120,"_mnemo.key":"fact.onboarding.agents-md","_mnemo.superseded_by":"01M7GJR0XECRE4RQ20C148JCVC","area":"onboarding","topic":"agents-md"}`

```text
AGENTS.md at the repo root orients agents working ON the source code (codebase layout, build/test commands, conventions, common task locations, dogfood workflow). Different audience from `mnemo about <file>` which orients agents USING a .mnemo file. The .cursor/rules/project-memory.mdc is the Cursor-specific version pointing at the same workflow. New contributors-via-LLM should read AGENTS.md first before substantive code changes. Added in v0.2 alongside 'For AI agents' sections in the three READMEs (root, mnemo/, mnemo-python/).
```

## 01M7GJR0XECRE4RQ20C148JCVC

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.agents-md`
- scope: private
- created: 2026-11-15T21:45:20Z
- importance: 0.8
- metadata: `{"_mnemo.key":"fact.onboarding.agents-md","_mnemo.supersedes":"01KSK8ZEP8N4ZE9MRBNGNZ33RX","area":"onboarding","topic":"agents-md"}`

```text
AGENTS.md at the repo root orients agents working ON the source code (codebase layout, build/test commands, conventions, common task locations, the dogfood workflow, the multi-agent roles). It's a different audience from `mnemo about`, which orients agents USING a .mnemo file, and from the guide (`mnemo onboard --guide`, the skill and the MCP instructions), which every session gets. New contributors-via-LLM should read AGENTS.md first before substantive code changes.
```

## 01KSGPG9WABVFKA7XPDAPVJ9GH

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.bootstrap`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.9
- metadata: `{"_mnemo.invalid_at":1794779119,"_mnemo.key":"fact.onboarding.bootstrap","_mnemo.superseded_by":"01M7GJQZSHEGJ46ZA8N0MPQ03K","area":"onboarding","topic":"bootstrap"}`

```text
REFRESH MEMORY FROM CANONICAL SEEDS: Edit test/scripts/seed.json (tracked in git), then python test/scripts/project_memory.py bootstrap — wipes test/project.mnemo and re-embeds all entries, writes test/project-memory.jsonl. Use remember for incremental adds without wiping prior episodic history.
```

## 01M7GJQZSHEGJ46ZA8N0MPQ03K

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.bootstrap`
- scope: private
- created: 2026-11-15T21:45:19Z
- importance: 0.9
- metadata: `{"_mnemo.key":"fact.onboarding.bootstrap","_mnemo.supersedes":"01KSGPG9WABVFKA7XPDAPVJ9GH","area":"onboarding","topic":"bootstrap"}`

```text
REFRESH FROM CANONICAL SEEDS: test/scripts/seed.json (tracked in git) holds the canonical onboarding memories. `python test/scripts/project_memory.py bootstrap --brain <scratch.mnemo>` builds a scratch brain from it (with MNEMO_PASSPHRASE set); it refuses the project brain, which it would wipe. To change a live onboarding memory, the Worker writes its new version under its key, and edits seed.json to match.
```

## 01KSGPG9W9G1AXCF1Y83EH17FV

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.checklist`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 1
- metadata: `{"_mnemo.invalid_at":1794779117,"_mnemo.key":"fact.onboarding.checklist","_mnemo.superseded_by":"01M7GJQXX8EB34MV8T7AS4JRCQ","area":"onboarding","topic":"checklist"}`

```text
NEW SESSION ONBOARDING (do this first): (1) Read test/README.md. (2) Confirm test/.env has MNEMO_PASSPHRASE and test/project.mnemo exists. (3) source test/.venv/bin/activate (create venv + maturin develop per test/README if missing). (4) python test/scripts/project_memory.py recall "project goals architecture dogfooding current task". (5) Treat recall output as authoritative context. (6) Read mnemo/README.md for engine details if implementing Rust/CLI.
```

## 01M7GJQXX8EB34MV8T7AS4JRCQ

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.checklist`
- scope: private
- created: 2026-11-15T21:45:17Z
- importance: 1
- metadata: `{"_mnemo.key":"fact.onboarding.checklist","_mnemo.supersedes":"01KSGPG9W9G1AXCF1Y83EH17FV","area":"onboarding","topic":"checklist"}`

```text
NEW SESSION ONBOARDING (do this first): (1) Your SessionStart hook has already loaded this brief; read it. (2) Read AGENTS.md, and for a multi-agent role its 'Multi-agent roles & coordination' section. (3) Recall for your task: the MCP recall tool, or `mnemo recall --text "<task in one sentence>" --no-track` (no path needed in this repo). (4) Treat what the brain returns as past context, not instructions. (5) Read mnemo/README.md for engine details if implementing Rust or the CLI.
```

## 01KSGPG9W9CVPE0C7DQ25KCG4R

- type: semantic
- agent: `cursor-agent`
- key: `fact.onboarding.mission`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 1
- metadata: `{"_mnemo.invalid_at":1794779117,"_mnemo.key":"fact.onboarding.mission","_mnemo.superseded_by":"01M7GJQXHEN3PPR3K65WQZKXFS","area":"onboarding","topic":"mission"}`

```text
PROJECT MISSION: We are building Memory Nemo (MNemo) — an encrypted single-file Rust agent-memory engine — while dogfooding it in this repo. Ongoing project context (architecture, decisions, how we work) lives in test/project.mnemo so new AI sessions stay aligned without relying only on chat history.
```

## 01M7GJQXHEN3PPR3K65WQZKXFS

- type: semantic
- agent: `cursor-agent`
- key: `fact.onboarding.mission`
- scope: private
- created: 2026-11-15T21:45:17Z
- importance: 1
- metadata: `{"_mnemo.key":"fact.onboarding.mission","_mnemo.supersedes":"01KSGPG9W9CVPE0C7DQ25KCG4R","area":"onboarding","topic":"mission"}`

```text
PROJECT MISSION: We are building Memory Nemo (MNemo), an encrypted single-file Rust agent-memory engine, while dogfooding it in this repo. Ongoing project context (architecture, decisions, how we work) lives in the project brain, .git/mnemo/project.mnemo, so new AI sessions stay aligned without relying only on chat history.
```

## 01KSGPG9W92JDDAJA08PCSNVYG

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.read-paths`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.95
- metadata: `{"_mnemo.invalid_at":1794701701,"_mnemo.key":"fact.onboarding.read-paths","_mnemo.superseded_by":"01M7E8XCFS3KTM7BF0S9W30DJ1","area":"onboarding","topic":"read-paths"}`

```text
TWO WAYS TO READ MEMORY: (A) Natural language — python test/scripts/project_memory.py recall "your question" (uses sentence-transformers all-MiniLM-L6-v2, 384-dim, local). (B) CLI only — mnemo info|list|get|verify on test/project.mnemo; mnemo recall|search need --query as comma-separated floats (copy vectors from test/project-memory.jsonl after bootstrap).
```

## 01M7E8XCFS3KTM7BF0S9W30DJ1

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.read-paths`
- scope: private
- created: 2026-11-15T00:15:01Z
- importance: 0.95
- metadata: `{"_mnemo.invalid_at":1794779118,"_mnemo.key":"fact.onboarding.read-paths","_mnemo.superseded_by":"01M7GJQZ1NASFDHSSKRK2PN8FT","_mnemo.supersedes":"01KSGPG9W92JDDAJA08PCSNVYG","area":"onboarding","topic":"read-paths"}`

```text
TWO WAYS TO READ MEMORY: (A) Natural language — python test/scripts/project_memory.py recall "your question" (uses the engine's built-in embedder, snowflake-arctic-embed-xs, 384-dim, local). (B) CLI only — mnemo info|list|get|verify on test/project.mnemo; mnemo recall|search need --query as comma-separated floats (copy vectors from test/project-memory.jsonl after bootstrap).
```

## 01M7GJQZ1NASFDHSSKRK2PN8FT

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.read-paths`
- scope: private
- created: 2026-11-15T21:45:18Z
- importance: 0.95
- metadata: `{"_mnemo.key":"fact.onboarding.read-paths","_mnemo.supersedes":"01M7E8XCFS3KTM7BF0S9W30DJ1","area":"onboarding","topic":"read-paths"}`

```text
WAYS TO READ MEMORY: (A) the MCP tools: recall (with the turn filter to search past sessions) and essence render. (B) the CLI, with no path in this repo: `mnemo about`; `mnemo recall --text "question" --no-track` (the built-in embedder plus keywords); `mnemo recall --text … --filter _mnemo.kind=turn` for past sessions; `mnemo get <id>` for one whole memory; `mnemo history --key K`. (C) the helper: `python test/scripts/project_memory.py recall "question"`, which runs the installed CLI.
```

## 01KSGPG9WAA9CC017H8BCFBH62

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.setup`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.95
- metadata: `{"_mnemo.invalid_at":1794779119,"_mnemo.key":"fact.onboarding.setup","_mnemo.superseded_by":"01M7GJQZDZ6DM0A5HZCKSA55F8","area":"onboarding","topic":"setup"}`

```text
ONE-TIME / REPAIR SETUP: cd mnemo-python && pip install maturin && maturin develop. python3 -m venv test/.venv && source test/.venv/bin/activate && pip install -r test/requirements.txt. Copy test/.env.example to test/.env and set MNEMO_PASSPHRASE (or ask user). Create DB: python test/scripts/project_memory.py bootstrap.
```

## 01M7GJQZDZ6DM0A5HZCKSA55F8

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.setup`
- scope: private
- created: 2026-11-15T21:45:19Z
- importance: 0.95
- metadata: `{"_mnemo.key":"fact.onboarding.setup","_mnemo.supersedes":"01KSGPG9WAA9CC017H8BCFBH62","area":"onboarding","topic":"setup"}`

```text
ONE-TIME / REPAIR SETUP: install the CLI with `curl -fsSL https://toarchkumar.github.io/mnemo/install.sh | sh` (to ~/.local/bin; if an older mnemo still runs, `rehash` or open a new terminal). `mnemo onboard` in the repo wires the session (the MCP server, the hooks, the skill) and finds the existing brain. The helper's perf and info commands need the Python binding in test/.venv built with the keychain (cd mnemo-python && maturin develop --release --features keychain,embed-candle into test/.venv). If the keychain entry is lost, the brain's recovery code restores it: `mnemo unlock`.
```

## 01KSGPG9W92FQVPB03D0N4WK05

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.workflow`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 1
- metadata: `{"_mnemo.invalid_at":1794779118,"_mnemo.key":"fact.onboarding.workflow","_mnemo.superseded_by":"01M7GJQYNN1Q4QCR05N11RP5QW","area":"onboarding","topic":"workflow"}`

```text
AGENT WORKFLOW IN THIS REPO: Before substantive work → recall with a short task description. After durable learnings → remember one fact per call (semantic=stable facts, procedural=how-to, episodic=dated events). Optionally add to test/scripts/seed.json and run bootstrap to refresh canonical memory. Follow .cursor/rules/project-memory.mdc.
```

## 01M7GJQYNN1Q4QCR05N11RP5QW

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.workflow`
- scope: private
- created: 2026-11-15T21:45:18Z
- importance: 1
- metadata: `{"_mnemo.invalid_at":1794860376,"_mnemo.key":"fact.onboarding.workflow","_mnemo.superseded_by":"01M7K07R0XRFKV7DZQ7BRJAD22","_mnemo.supersedes":"01KSGPG9W92FQVPB03D0N4WK05","area":"onboarding","topic":"workflow"}`

```text
AGENT WORKFLOW IN THIS REPO: before substantive work, recall with your task in one sentence. Only the Worker writes facts: one fact per write, each with a key (decision.<topic>, finding.<topic>, …) and the agent cursor-agent, through `python test/scripts/project_memory.py remember "…" --key K` (it passes the agent) or the MCP remember tool with key and agent_id. The Planner and the Reviewers only read (recall, --no-track); every session's hooks save its own transcript (AGENTS.md, tmux rule 9). Types: semantic for stable facts, procedural for how-to, episodic for dated events.
```

## 01M7K07R0XRFKV7DZQ7BRJAD22

- type: procedural
- agent: `cursor-agent`
- key: `fact.onboarding.workflow`
- scope: private
- created: 2026-11-16T20:19:36Z
- importance: 1
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.onboarding.workflow","_mnemo.supersedes":"01M7GJQYNN1Q4QCR05N11RP5QW","area":"onboarding","topic":"workflow"}`

```text
AGENT WORKFLOW IN THIS REPO: before substantive work, recall with your task in one sentence. Only the Worker writes facts: one fact per write, each with a key (decision.<topic>, finding.<topic>, …), through `python test/scripts/project_memory.py remember "…" --key K` or the MCP remember tool with key. The brain's default agent (cursor-agent) gives every write its slot, so none names the agent. The Planner and the Reviewers only read (recall, --no-track); every session's hooks save its own transcript (AGENTS.md, tmux rule 9). Types: semantic for stable facts, procedural for how-to, episodic for dated events.
```

## 01M7S0H6RZABT72W9DZ9TSG0FK

- type: episodic
- agent: `cursor-agent`
- key: `fact.post-commit-retry-bug`
- scope: private
- created: 2026-11-19T04:20:12Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1795062245,"_mnemo.key":"fact.post-commit-retry-bug","_mnemo.superseded_by":"01M7S0R9VF0Y9MBW54BM8BEQQP","area":"correctness","topic":"flush-retry"}`

```text
fact.post-commit-retry-bug (found 2026-11-18 in v0200-1, pre-existing, not fixed; for the Planner to triage): on an exclusive handle, a flush that fails at or after the commit point (e.g. the WAL commit write itself), retried on the same handle, can leave a file whose reopen fails PageAuthFailed(83). Cause: step 3e stamps header.wal_seq = txn_id and the run pointers in place before the commit, and v0200-1's restore covers only failures before the commit point. Evidence: a debug test failing write #2 (the WAL commit) of one commit: the retry returned Ok, the reopen returned Err(PageAuthFailed(83)); that commit's trace was [Page(81), Page(82), Sync, WalCommit, Sync, Page(83..87), Page(0), Sync]. The shared window reloads after an error, so shared handles aren't affected.
```

## 01M7S0R9VF0Y9MBW54BM8BEQQP

- type: episodic
- agent: `cursor-agent`
- key: `fact.post-commit-retry-bug`
- scope: private
- created: 2026-11-19T04:24:05Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1795071324,"_mnemo.key":"fact.post-commit-retry-bug","_mnemo.superseded_by":"01M7S9DCP7AJ5KC6N1BWW629EX","_mnemo.supersedes":"01M7S0H6RZABT72W9DZ9TSG0FK","area":"correctness","topic":"flush-retry"}`

```text
fact.post-commit-retry-bug (found 2026-11-18 in v0200-1; pre-existing, not fixed; for the Planner to triage): on an exclusive handle, a flush that fails at the WAL commit write or the checkpoint's first page write, retried on the same handle, returns Ok but leaves a file whose reopen fails PageAuthFailed. Evidence: a temporary test (200 memories, then one remember; fail write k of its flush; retry; close; reopen), run on release-0.20.0 at 2d21ac6 and on v0200-1's branch with identical results: k=2 and k=3 reopen Err(PageAuthFailed(20)), k=0,1,4..9 reopen Ok((201, Ok(201))). The commit's trace: [Page(18), Page(19), Sync, WalCommit, Sync, Page(20..24), Page(0), Sync], so write 2 is the WAL commit and write 3 the first checkpoint page. Likely cause: step 3e stamps header.wal_seq = txn_id and the run pointers in place before the commit, and v0200-1's restore of (header, free, free_dirty) covers only failures before the commit point. The shared window reloads after an error, so shared handles aren't affected (not tested).
```

## 01M7S9DCP7AJ5KC6N1BWW629EX

- type: episodic
- agent: `cursor-agent`
- key: `fact.post-commit-retry-bug`
- scope: private
- created: 2026-11-19T06:55:24Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.post-commit-retry-bug","_mnemo.supersedes":"01M7S0R9VF0Y9MBW54BM8BEQQP"}`

```text
fact.post-commit-retry-bug (found 2026-11-18 in v0200-1; FIXED by v0200-1r, branch v0200-1r-retry-after-commit, into release-0.20.0): on an exclusive handle, a flush that failed at the WAL commit write or the checkpoint's first page write, retried on the same handle, returned Ok but left a file whose reopen failed PageAuthFailed(20) (200 memories + 1; write k=2 and k=3 of the flush). Present in v0.19.1. Cause: prepare_for_flush builds CatalogDelta { prev: ptr_tuple(self.header.catalog_delta) } and IndexDeltaRef { prev: ptr_tuple(self.header.index_delta) } from the header, which after a failure at or past the commit point still holds the failed attempt's run pointers (page 20, never written), so the retry's delta linked to a page that doesn't exist. Restoring the old header there is unsafe: the commit may be durable, and the retry would reuse its pages and generation. Fix: see fact.flush-commit-failed.
```

## 01M33R12ZRKN2J8CXGSNEG4251

- type: semantic
- agent: `cursor-agent`
- key: `fact.python.parity`
- scope: private
- created: 2026-09-22T05:04:45Z
- importance: 0.75
- metadata: `{"_mnemo.invalid_at":1794691954,"_mnemo.key":"fact.python.parity","_mnemo.superseded_by":"01M7DZKY67J6344XC7ED8TNP5D"}`

```text
Binding-parity gap (tracked as PR BP): the Python binding lacks set_cache_flush_policy, set_cache_budget, rekey, compact_file, and memories(); all exist on the Rust core. Must land before Phase 3.
```

## 01M7DZKY67J6344XC7ED8TNP5D

- type: episodic
- agent: `cursor-agent`
- key: `fact.python.parity`
- scope: private
- created: 2026-11-14T21:32:34Z
- importance: 0.85
- metadata: `{"_mnemo.key":"fact.python.parity","_mnemo.supersedes":"01M33R12ZRKN2J8CXGSNEG4251"}`

```text
v0.4.1 released — first successful live PyPI Trusted Publishing (OIDC) release. Publisher registration required correcting Repository from mnemo-engine to mnemo (owner/repo, not PyPI project name). crates.io + PyPI both at 0.4.1. Bundles PR 7 fuzz + PR 8 proptest + PR 9 deny/fmt/TP + PR BP binding parity. attestations:true on the pypa action also minted PEP-740 attestations.
```

## 01M7RFVMA0PQRT5PBTRKSVHHNY

- type: semantic
- agent: `cursor-agent`
- key: `fact.read-failpoint`
- scope: private
- created: 2026-11-18T23:28:48Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.read-failpoint","area":"testing","topic":"read-failpoint"}`

```text
Since v0191-1b, __failpoints::set_page_read_fails(page, count) injects an I/O error on reads of a page (read_exact_at, read_page, read_page_uncached). It's process-wide, so a test that arms it lives in a test binary of its own (tests/index_load_retry.rs). Disarm with (0, 0).
```

## 01M7SN6BWK9RMQZ013X3KMVMFJ

- type: semantic
- agent: `cursor-agent`
- key: `fact.recall-after-pause`
- scope: private
- created: 2026-11-19T10:21:17Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.recall-after-pause","area":"performance","topic":"bench-method"}`

```text
fact.recall-after-pause (worker-a, v0200-3, 2026-11-19): on the M1 Pro bench machine a recall after any pause runs slower than back-to-back warm recalls, whatever the engine does. shared_bench cell 10 at 10K: warm recall 2.3-2.4 ms p50 back to back; a recall after a 20 ms sleep with no commit 6.1-13.0 ms; after a sleep then a 10 ms spin 5.8 ms; the second recall after a sleep 5.6 ms. So 'first recall after a catch-up' must be read against cell 10's control rows (warm_recall_after_20ms_sleep_no_commit, warm_recall_after_the_catch_ups), not only against a back-to-back warm recall.
```

## 01M7TCEJWAMTJVHCZ481QMG1RQ

- type: semantic
- agent: `cursor-agent`
- key: `fact.release-dry-run-branch`
- scope: private
- created: 2026-11-19T17:07:44Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.release-dry-run-branch","area":"release","topic":"dry-run-branch"}`

```text
fact.release-dry-run-branch (worker-a, 2026-11-19, seen in run 44569170984): a release.yml dry run started from a feature branch always ends failed, because publish to PyPI and publish to crates.io are refused before any step; the pypi and crates-io environments allow only tag:v* and branch:release-*. Every build job, check-artifacts, mcp-bundle and attach still run. Run dry runs on the release branch (RELEASING.md), or read the per-job results when testing a release.yml change from a branch.
```

## 01M7BAMSQB14EKM6KMQ2QGAXSX

- type: episodic
- agent: `cursor-agent`
- key: `fact.release.latest`
- scope: private
- created: 2026-11-13T20:47:34Z
- importance: 0.85
- metadata: `{"_mnemo.invalid_at":1794691940,"_mnemo.key":"fact.release.latest","_mnemo.superseded_by":"01M7DZKG8B43MSECH2XM5B2A8E","area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
As of 2026-11-13, v0.14.0 steps 1, 2a, 2b, 2c, 3, 4a and 4b are merged on release-0.14.0. The queue to the release: DOGFOOD-0, REKEY-2, step 5 (the bench, then correcting the 12.4 note's numbers), READY, REVIEWER-B's release review and its fixes, the release-prep HANDOFF, the tag, the user's approval click, and VERIFIED on every registry.
```

## 01M7DZKG8B43MSECH2XM5B2A8E

- type: episodic
- agent: `cursor-agent`
- key: `fact.release.latest`
- scope: private
- created: 2026-11-14T21:32:20Z
- importance: 0.6
- metadata: `{"_mnemo.key":"fact.release.latest","_mnemo.supersedes":"01M7BAMSQB14EKM6KMQ2QGAXSX"}`

```text
v0.14.0 released 2026-11-14 (run 43561321997, all publishes green). The site is served without Jekyll since dfd56d9 (.nojekyll at the repo root): Jekyll had failed on a Liquid {{ in RELEASING.md since 2026-11-12, so Pages stopped deploying after v0.13.1. Any future Liquid-looking text in Markdown is now harmless.
```

## 01M7SBK4ZPAHK1KPVKWMJPE4NX

- type: episodic
- agent: `cursor-agent`
- key: `fact.shared-reload-footprint`
- scope: private
- created: 2026-11-19T07:33:30Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"fact.shared-reload-footprint"}`

```text
fact.shared-reload-footprint (worker-a, 2026-11-18, measured in v0200-2 on v0.19.1; for the Planner to triage): on macOS, every full reload of a SharedMnemo (another process's commit) grows the process's physical footprint by about one copy of its large buffers, and it doesn't come back. Two shared handles, one remember per round, one reload each: footprint (vmmap --summary) 18.6 -> 72.4 MB over 100 reloads at 10K, 127.2 -> 599.6 MB over 100 at 100K (about 4.7 MB per reload), straight line, no plateau within 100; unchanged after both handles were dropped. `leaks`: 0 leaked bytes after drop (17877 'leaks' / 3.4 MB reported while handles were open). vmmap after drop (separate 60-reload run at 100K): MALLOC_LARGE (empty) 617.1 MB resident in 24 regions, i.e. freed large blocks libmalloc keeps, not live data. Not measured: past 100 reloads, Linux, under memory pressure. Live v0.18.1 `mnemo serve --mcp --shared` processes here (brain ~11K) had RSS 3-90 MB after ~27 h. Incremental catch-up (docs/design/catch-up.md) avoids the reallocation for most commits; its target is under +10 MB footprint after 100 catch-ups at 100K. Scratch bench: catchup_bench (to become shared_bench cell 10).
```

## 01M7C603184662HB2WWAJC76VK

- type: semantic
- agent: `cursor-agent`
- key: `fact.shared.busy-readers`
- scope: private
- created: 2026-11-14T04:45:35Z
- importance: 0.8
- metadata: `{"_mnemo.invalid_at":1794691945,"_mnemo.key":"fact.shared.busy-readers","_mnemo.superseded_by":"01M7DZKMWFW96R02RWHW2HET32"}`

```text
Shared mode's limits (v0.14.0): readers come first since step 5, so busy readers slow a writer, and with many processes reading with no pause (16 on cheap reads) a write can wait out its deadline and fail with Locked; nothing is lost and calling again is safe. LOCK-2 (a writer that has waited about 2 s gets the next turn) is planned before step 8 and before shared mode becomes the MCP default. A compaction at 100K holds the file about 175 s (PERF-2).
```

## 01M7DZKMWFW96R02RWHW2HET32

- type: episodic
- agent: `cursor-agent`
- key: `fact.shared.busy-readers`
- scope: private
- created: 2026-11-14T21:32:25Z
- importance: 0.7
- metadata: `{"_mnemo.key":"fact.shared.busy-readers","_mnemo.supersedes":"01M7C603184662HB2WWAJC76VK","area":"decisions"}`

```text
2026-11-14, the user (option 1): LOCK-2, the writer safety valve, went into v0.15 as step 2b, after the keychain and before mcp install, because in v0.14 busy readers could lock out a save (6 of 7 writes Locked against 16 cheap readers).
```

## 01M7H5VF64MTNHF2CBBYWFHQJ7

- type: episodic
- agent: `cursor-agent`
- key: `finding.13a-path2-socket-format`
- scope: private
- created: 2026-11-16T03:19:16Z
- importance: 0.7
- metadata: `{"_mnemo.key":"finding.13a-path2-socket-format","area":"decision","topic":"finding.13a-path2-socket-format"}`

```text
2026-11-15 (13a step 6): path 2, the idle wake-up through the session's own CLAUDE_CODE_MESSAGING_SOCKET, was dropped per the note's rule: the cross-session messaging docs (https://code.claude.com/docs/en/cross-session-messaging.md, read 2026-11-15) document the socket path, the token, the optional-on-macOS auth line {"type":"auth","token":"<token>"}, the 30 s line timeout and the own-child rule for crossSessionInbound, but not the format of the line that carries a message (nor do env-vars.md, hooks.md or the mods API page). The stdio MCP server does inherit CLAUDE_CODE_MESSAGING_SOCKET and CLAUDE_CODE_MESSAGING_TOKEN (checked on this Mac's running server, names only). v0.17.0 ships paths 1 and 3.
```

## 01M7EDKK9DDXK4B4ASR54QVMGY

- type: semantic
- agent: `cursor-agent`
- key: `finding.bench-1w1r-p99`
- scope: private
- created: 2026-11-15T01:37:03Z
- importance: 0.7
- metadata: `{"_mnemo.key":"finding.bench-1w1r-p99","area":"findings"}`

```text
The shared bench's 1w+1r read p99 (v0.15, 10k memories) is a metric artifact, not the valve: each write's commit makes about one read wait for it (the worst read, 129-158 ms, matches one write in every run, with the valve or MNEMO_DEV_NO_VALVE), so p99 lands on those waits whenever the reader manages under about 100 reads per write. Three quiet runs on 2026-11-15 gave 76, 52 and 49 ms (2-3% of reads coincided with a commit); one with 0.9% gave 0.29 ms. The reader's rate swings about threefold with machine load. 8w+2r read p99 was 657-755 ms, the readers-first trade.
```

## 01M7E3Y98BZ9JHWPXR509ZMAZX

- type: semantic
- agent: `cursor-agent`
- key: `finding.candle-msrv-x86`
- scope: private
- created: 2026-11-14T22:48:08Z
- importance: 0.8
- metadata: `{"_mnemo.key":"finding.candle-msrv-x86","area":"findings"}`

```text
candle-core 0.11.1 (the built-in embedder's runtime, v0.15 step 5) builds on x86_64 only with Rust 1.94 or later: its quantized/repack_x86.rs calls core::arch::x86_64::__cpuid_count outside an unsafe block, which Rust 1.92 and 1.93 reject (E0133) and 1.94+ accept. aarch64 builds at 1.92, so a local MSRV check on an M1 misses it; CI's msrv (rust 1.92, linux) job on x86_64 caught it (run 43648320104, 2026-11-14).
```

## 01M7EK1PVXMNMXG6BVW7T13KHT

- type: semantic
- agent: `cursor-agent`
- key: `finding.cargo-install-features`
- scope: private
- created: 2026-11-15T03:12:09Z
- importance: 0.8
- metadata: `{"_mnemo.key":"finding.cargo-install-features","area":"findings"}`

```text
cargo install mnemo-engine with no features builds a CLI with no OS keychain and no built-in embedder, so mnemo init with no passphrase source fails; from crates.io the CLI needs cargo install mnemo-engine --features keychain,embed-candle (the prebuilt binaries and the PyPI wheels have both). A pip install from the sdist builds without either too (pyproject.toml's features only; PY-SDIST, after v0.15).
```

## 01M7HDDBF05ZN90V80M2VY2KJJ

- type: episodic
- agent: `cursor-agent`
- key: `finding.claim-probe-in-window`
- scope: private
- created: 2026-11-16T05:31:22Z
- importance: 0.7
- metadata: `{"_mnemo.key":"finding.claim-probe-in-window","area":"performance","topic":"finding.claim-probe-in-window"}`

```text
2026-11-15 (RB-012.6 item 3): the presence probe's 20 ms retry inside the exclusive claim window cost task next 79.3 ms facing 3 live workers and 167.1 ms facing 6 (load near 6); the claim path now probes once (probe_once), giving 1.4 and 1.5 ms (load near 3); who keeps the retry.
```

## 01M7GTRM7JAFHQA4GD6D8NAEM4

- type: episodic
- agent: `cursor-agent`
- key: `finding.coordination-order`
- scope: private
- created: 2026-11-16T00:05:28Z
- importance: 0.7
- metadata: `{"_mnemo.key":"finding.coordination-order","area":"design","topic":"finding.coordination-order"}`

```text
2026-11-15 (13a step 2): created_at has one-second grain, so ordering coordination records by (created_at, id) misorders writes within a second (ULID randomness). Sort by created_at with a stable sort over the catalog, which keeps write order.
```

## 01M7GJSSY1KG6VYNXWG4G7F49H

- type: episodic
- agent: `cursor-agent`
- key: `finding.helper-cli-choice`
- scope: private
- created: 2026-11-15T21:46:18Z
- importance: 0.75
- metadata: `{"_mnemo.key":"finding.helper-cli-choice","area":"findings"}`

```text
2026-11-15 after the dogfood switch: project_memory.py recall failed with 'a key store (none in this build yet)' because mnemo_cli() preferred mnemo/target/release/mnemo (a v0.15.0 build without the keychain feature) over the PATH's 0.16.1. The CLI lists no build features, so the helper now orders $MNEMO_BIN, every mnemo on the PATH, then the repo build, and skips a candidate at v0.15+ whose read-only `info` on the key brain fails with 'none in this build' (it isn't asked when MNEMO_PASSPHRASE opens a scratch brain). Measured: recall end to end in 0.47 s.
```

## 01M7GJST9MM4SF93PDW133ZMSV

- type: semantic
- agent: `cursor-agent`
- key: `finding.keyed-write-agent-slot`
- scope: private
- created: 2026-11-15T21:46:19Z
- importance: 0.8
- metadata: `{"_mnemo.key":"finding.keyed-write-agent-slot","area":"conventions"}`

```text
The project brain's facts are Private to agent_id cursor-agent, and a key's slot is per agent, so a keyed write must name that agent: project_memory.py remember passes --agent cursor-agent; the CLI needs --agent cursor-agent; the MCP remember tool needs agent_id: cursor-agent. The CLI's and the MCP server's default agent is 'default', so a keyed write without it starts a second live slot beside the first instead of superseding it (found 2026-11-15 while rewriting the onboarding notes).
```

## 01M7DZPNVHB41Z76JX6PBTSFGT

- type: semantic
- agent: `cursor-agent`
- key: `finding.keyed-write-slot`
- scope: private
- created: 2026-11-14T21:34:04Z
- importance: 0.8
- metadata: `{"_mnemo.key":"finding.keyed-write-slot","area":"findings"}`

```text
A keyed write (Mnemo::remember_keyed, mnemo/src/store/keyed.rs) supersedes only the live record in its own key slot (key_index.get(&slot), then _mnemo.superseded_by and _mnemo.invalid_at on that record), and update refuses _mnemo.* metadata, so a fact written without a key can't be superseded by a later keyed write; v0.15 step 5b added remember_keyed_superseding (mnemo remember --key K --supersedes <id>) for that, and the cache rebuild already treats a record with _mnemo.superseded_by as superseded before reading its key.
```

## 01M7GXYSGZJFFZNTXNS0J19F7G

- type: episodic
- agent: `cursor-agent`
- key: `finding.macos-fsync-race-cost`
- scope: private
- created: 2026-11-16T01:01:16Z
- importance: 0.7
- metadata: `{"_mnemo.key":"finding.macos-fsync-race-cost","area":"performance","topic":"finding.macos-fsync-race-cost"}`

```text
2026-11-15 (13a step 3): the 1,000-round two-process claim race takes about 80 s on this Mac (load near 9) because each round is two durable commits and macOS's sync_all is a full fsync (~37 ms per shared write window measured at 200 rounds). The FLAKE-1 spawner thread costs nothing measurable. CL_RACE_ROUNDS overrides the count.
```

## 01M7EH1AEM21V79Y8TN1MWEM2N

- type: semantic
- agent: `cursor-agent`
- key: `finding.mcp-builtin-width`
- scope: private
- created: 2026-11-15T02:36:59Z
- importance: 0.8
- metadata: `{"_mnemo.key":"finding.mcp-builtin-width","area":"findings"}`

```text
RB-010.1 (v0.15): serve --mcp without --embed-cmd exited before the handshake on a brain of another width once the model was cached (set_embedder's DimensionMismatch was ?'d), and recorded the built-in's identity at start in any 384-wide brain with no identity. Fixed before the tag: serve_inner peeks the width before loading or fetching the model, and attaches the built-in the way the CLI's recall does: registered on a file that records it, recorded only on a scaffold-only file, otherwise served without an embedder with a notice; no default-embedder error ends the server, plain or --shared.
```

## 01M7H4ZKP1KNAZCZB9CZCNHASP

- type: episodic
- agent: `cursor-agent`
- key: `finding.per-turn-hook-cost`
- scope: private
- created: 2026-11-16T03:04:03Z
- importance: 0.7
- metadata: `{"_mnemo.key":"finding.per-turn-hook-cost","area":"performance","topic":"finding.per-turn-hook-cost"}`

```text
2026-11-15 (13a step 5, R-010.6): an unchanged per-turn hook on an 11,000-record brain took 22.8 ms median vs mnemo --version 3.4-3.8 ms (load 4.4); this brain's keychain read plus a read-only open (installed mnemo info) took 67 ms vs 5 ms (load 4.6). With the stamp fast path (Mnemo::file_stamp = page 0 seal read without the key + identity/len/mtime, in sidecar hook.turn.*) and finding the brain without spawning git (git rev-parse was ~14 ms), an unchanged turn is 3.4 ms vs 3.3-4.1 ms (load 3.2).
```

## 01M7E3YE2F6XVJRW437PXGAG90

- type: semantic
- agent: `cursor-agent`
- key: `finding.ring-manylinux-aarch64`
- scope: private
- created: 2026-11-14T22:48:13Z
- importance: 0.8
- metadata: `{"_mnemo.invalid_at":1794699264,"_mnemo.key":"finding.ring-manylinux-aarch64","_mnemo.superseded_by":"01M7E6K0NE8ZK892KWR4KTP77T","area":"findings"}`

```text
ring 0.17.14 (rustls's crypto, pulled in by ureq for the built-in embedder's model download) fails to build in maturin's manylinux2014 aarch64 cross container: its pregenerated ARM assembly needs __ARM_ARCH, which that old cross gcc doesn't define (#error "ARM assembler must define __ARM_ARCH"). The aarch64 CLI built with cargo-zigbuild is fine (zig cc defines it). Seen in the v0.15 step 5 release dry run 43648416346, 2026-11-14.
```

## 01M7E6K0NE8ZK892KWR4KTP77T

- type: semantic
- agent: `cursor-agent`
- key: `finding.ring-manylinux-aarch64`
- scope: private
- created: 2026-11-14T23:34:24Z
- importance: 0.8
- metadata: `{"_mnemo.key":"finding.ring-manylinux-aarch64","_mnemo.supersedes":"01M7E3YE2F6XVJRW437PXGAG90","area":"findings"}`

```text
Building ring 0.17.14 in maturin's manylinux2014 aarch64 cross container with CFLAGS_aarch64_unknown_linux_gnu=-D__ARM_ARCH=8 makes the wheel build, but the result is broken: installed on a real aarch64 runner (ubuntu-24.04-arm), its rustls rejects huggingface.co's certificate with 'invalid peer certificate: BadSignature' (release dry run 43650998342, 2026-11-14). The same download works from the x86_64 Linux build. Forcing the define compiles ring's ARMv8 assembly with an old cross gcc but it doesn't verify signatures correctly; the arm check caught it.
```

## 01M7G76P1NQ8RHA8CCG3DM96T4

- type: episodic
- agent: `cursor-agent`
- key: `finding.shell-hash-after-install`
- scope: private
- created: 2026-11-15T18:23:38Z
- importance: 0.7
- metadata: `{"_mnemo.key":"finding.shell-hash-after-install","area":"findings"}`

```text
v0.16.0 by-hand run (2026-11-15): after install.sh put mnemo 0.16.0 in ~/.local/bin, the user's zsh still ran the old ~/.cargo/bin/mnemo (0.12.3) from its command hash until `rehash` (`which -a mnemo` listed both). The script can't see the parent shell's hash, so from v0.16.1 both install scripts name every other mnemo on the PATH after the install, with `hash -r` / `rehash` / a new terminal, else remove it or put the install's directory first.
```

## 01M7EDKJQR1S1PM93EPG2WKFDS

- type: semantic
- agent: `cursor-agent`
- key: `finding.strip-release`
- scope: private
- created: 2026-11-15T01:37:03Z
- importance: 0.8
- metadata: `{"_mnemo.key":"finding.strip-release","area":"findings"}`

```text
strip = "symbols" in the engine's release profile breaks the Kotlin bindings: UniFFI's binding generator reads its metadata (UNIFFI_META_* symbols) from the built libmnemo_uniffi library, and the strip removes them ('No UniFFI metadata found', Mobile CI 2026-11-15). Since v0.15 [profile.release.package.mnemo-uniffi] strip = "none" keeps them; the CLI and the wheels are stripped. Two artifacts don't change with the strip: the Windows ones (MSVC keeps symbols in a .pdb that isn't shipped) and the aarch64 Linux CLI, which cargo-zigbuild's link already strips.
```

## 01M7RFVKKMKZ2H1BWWPZE3YMFH

- type: procedural
- agent: `cursor-agent`
- key: `howto.python-test-wheel`
- scope: private
- created: 2026-11-18T23:28:47Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"howto.python-test-wheel","area":"testing","topic":"python-wheel"}`

```text
The Python binding tests (mnemo-python/test_mnemo.py) need a wheel built with `maturin build --release --features dev-hooks,embed-candle` in a scratch venv; without dev-hooks, test_index_and_snapshots fails on has_index.
```

## 01M7RFVK83HK1BXJ0FH94FAD8M

- type: procedural
- agent: `cursor-agent`
- key: `howto.task-blocked-to-ready`
- scope: private
- created: 2026-11-18T23:28:47Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"howto.task-blocked-to-ready","area":"coordination","topic":"task-transitions"}`

```text
mnemo task update: a blocked task can't go straight to ready (refused). Move it to active first, then ready.
```

## 01M7FZP1BKZ03Y1E2W3DQQ1FSX

- type: procedural
- agent: `cursor-agent`
- key: `lesson.str-slicing-non-ascii`
- scope: private
- created: 2026-11-15T16:12:12Z
- importance: 0.75
- metadata: `{"_mnemo.key":"lesson.str-slicing-non-ascii","area":"conventions"}`

```text
RB-011.1 lesson (v0.16.0): never slice a Rust &str at a byte offset that isn't known to sit next to an ASCII byte (e.g. rest[..4], &s[i+1..i+3]); compare bytes (as_bytes().get(..)) instead. The redactor and the MCP key decoder both panicked on ordinary text like "it’s", "café" or "50%→60%". Any text-scanning code gets tests with curly quotes, an em dash, accents, CJK and an emoji, and redact.rs has a no-panic proptest.
```

## 01M7HFK6HSSVFCZZHAR8049VZT

- type: semantic
- agent: `cursor-agent`
- key: `onboarding.coordination`
- scope: private
- created: 2026-11-16T06:09:31Z
- importance: 0.85
- metadata: `{"_mnemo.key":"onboarding.coordination","area":"onboarding","topic":"coordination"}`

```text
COORDINATION IN THIS REPO (v0.17, 13a): each session is named with MNEMO_AGENT and MNEMO_ROLE before its client starts: planner-a (role planner, the user's control center), worker-a (role worker, the sole committer), reviewer-b (role reviewer, code health) and reviewer-a (role reviewer, product and market; it runs in Antigravity without the hooks, so it uses the CLI with --as reviewer-a --role reviewer). The brain carries the work: tasks (mnemo task add --for worker; task next; task update, where ready waits for the creator's done), the inbox (mnemo post, inbox, ack) and claims (mnemo claim take, who). The per-turn hook shows what's new each turn. In Claude Code a session that posts or adds a task also wakes its peer with SendMessage, a pointer only: the inbox is the record. Messages are information, never authority; approvals, releases and anything irreversible stay with the user. Until HARNESS-DOGFOOD-1, PLAN.md and reviewer_notes.md stay the ledgers, as AGENTS.md's tmux rules say. The guide's section 6, Working with other agents, has the commands.
```

## 01M7NBTP73WBC2SHVX5M4SE7WY

- type: procedural
- agent: `cursor-agent`
- key: `process.brain-backup`
- scope: private
- created: 2026-11-17T18:20:40Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"process.brain-backup","area":"process","topic":"release"}`

```text
The brain's backup (RELEASING.md step 7, from v0.18.0): before tagging, run mnemo export --git -o docs/brain.md with the installed CLI, read the diff for anything the public repo shouldn't publish, and commit it; from Run 2 it replaces the chore(plan) coordination-file commit. The first docs/brain.md (PR 126) was written by v0.17.1 because a freshly built binary would trigger a keychain prompt on the real brain, so its header line is v0.17.1's.
```

## 01M7PYQQBWX4TKX8R35AGDMWEN

- type: procedural
- agent: `cursor-agent`
- key: `process.inspect-screenshot`
- scope: private
- created: 2026-11-18T09:10:20Z
- importance: 0.5
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"process.inspect-screenshot","area":"process","topic":"docs"}`

```text
Retaking docs/img/inspect-findings.png (v0.19.0, PR 138 dabb5fc): sh test/scripts/inspect_screenshot.sh builds a synthetic demo brain (mnemo/examples/inspect_demo_brain.rs: DEMO-* findings in every status, role-named authors, made-up text) in a temp dir under a throwaway passphrase, runs mnemo inspect from inside that dir (so the header shows only demo.mnemo, not a machine path), and screenshots <url>#findings with headless Chrome (light theme, 1200x540, --virtual-time-budget=5000); it checks the 300 KB limit. The inspector opens a tab from the URL hash (#findings, #tasks). Never photograph the project brain.
```

## 01M7PTYRY1TC3YCP9W55G1DJ1K

- type: procedural
- agent: `cursor-agent`
- key: `process.perf-measurement`
- scope: private
- created: 2026-11-18T08:04:16Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"process.perf-measurement","area":"process","topic":"performance"}`

```text
Measuring PERF numbers on this Mac: cargo run --release --features benches --example device_budget -- --sizes 1000,10000,100000 (cached corpora under mnemo/target/device_budget; move the dir aside to rebuild with a new layout); run twice and log uptime's load average per run (macOS background work like fileproviderd, mediaanalysisd, mlhostd, ANECompilerService can push it to 5 to 26). examples/perf2_split.rs gives the 100K index load split (--rewrite-to copies and rewrites the base in the current layout). ranking_v2 takes --perf-json <scratch copy>; ann_defaults_v0_6_0 writes only with --out. Don't build or run other heavy work while a benchmark runs.
```

## 01M71NHYN8Q47NZEASB4EHC62N

- type: procedural
- agent: `cursor-agent`
- key: `process.release-flow`
- scope: private
- created: 2026-11-10T02:45:50Z
- importance: 0.8
- metadata: `{"_mnemo.invalid_at":1794710218,"_mnemo.key":"process.release-flow","_mnemo.superseded_by":"01M7EH1A0EMA4ZPE8A4MGV19YQ"}`

```text
Release flow (MOBILE-1e): one v* tag runs release.yml: the engine (crates.io, PyPI), then the Swift package (MnemoFFI-<v>.xcframework.zip on the GitHub Release + toarchkumar/mnemo-swift main with bare tag <v>) and the Android library (publishAndReleaseToMavenCentral). A manual run is a dry run (engine jobs skipped; Central upload USER_MANAGED, validated, dropped via the Portal API). Checklist and credential renewals: RELEASING.md.
```

## 01M7EH1A0EMA4ZPE8A4MGV19YQ

- type: procedural
- agent: `cursor-agent`
- key: `process.release-flow`
- scope: private
- created: 2026-11-15T02:36:58Z
- importance: 0.8
- metadata: `{"_mnemo.key":"process.release-flow","_mnemo.supersedes":"01M71NHYN8Q47NZEASB4EHC62N"}`

```text
Release flow (v0.15): one v* tag runs release.yml: the engine (crates.io, PyPI, the GitHub Release's 11 files), then the Swift package (MnemoFFI-<v>.xcframework.zip on the GitHub Release + toarchkumar/mnemo-swift main with bare tag <v>) and the Android library (publishAndReleaseToMavenCentral). Before the approval, check-artifacts runs every CLI and wheel on a runner of its own target and check-sdist builds the sdist. A manual run is a dry run that publishes nothing: on a release-* branch every publish job runs its dry-run steps (cargo publish --dry-run, the PyPI and attach listings, a signed publishToMavenLocal and a Central upload validated then dropped, and a temporary dry-run-<v> branch pushed to mnemo-swift and deleted); on any other branch the registry environments refuse the four publish jobs before a step runs. Checklist and credential renewals: RELEASING.md.
```

## 01M7EK1Q8GV8GNEAH2YXD28CH1

- type: procedural
- agent: `cursor-agent`
- key: `process.release-order`
- scope: private
- created: 2026-11-15T03:12:09Z
- importance: 0.9
- metadata: `{"_mnemo.invalid_at":1794760830,"_mnemo.key":"process.release-order","_mnemo.superseded_by":"01M7G19V4K51V9C7SZW3W3199E","area":"conventions"}`

```text
The release order (v0.14 and v0.15): merge the last PR into the release branch and check its push runs; run the release dry run on the release branch itself with mobile on (only there do the publish jobs run their dry-run steps); commit the Planner's HANDOFF (root README, index.html) on its own; commit PLAN.md and reviewer_notes.md as chore(plan): sync coordination files; fast-forward main to the release branch; tag vX.Y.Z annotated 'MNemo X.Y.Z' and push it; stop at 'approve the release' for the user's click; after it, check the four registries and the GitHub Release, then delete the merged branches.
```

## 01M7G19V4K51V9C7SZW3W3199E

- type: procedural
- agent: `cursor-agent`
- key: `process.release-order`
- scope: private
- created: 2026-11-15T16:40:30Z
- importance: 0.85
- metadata: `{"_mnemo.key":"process.release-order","_mnemo.supersedes":"01M7EK1Q8GV8GNEAH2YXD28CH1","area":"conventions"}`

```text
The release order (v0.14 to v0.16): merge the last PR into the release branch and check its push runs; run the release dry run on the release branch itself with mobile on (only there do the publish jobs run their dry-run steps; attest is skipped on a dry run); commit the Planner's HANDOFF (root README, index.html) on its own; commit PLAN.md and reviewer_notes.md as chore(plan): sync coordination files; fast-forward main to the release branch; tag vX.Y.Z annotated 'MNemo X.Y.Z' and push it; stop at 'approve the release' for the user's click. Since v0.16 the attest job runs after the approval, and attach and both publish jobs wait for it. After it: check attest succeeded first, the four registries and the GitHub Release, the site (Pages serves install.sh, install.ps1, llms.txt and llms-full.txt byte-equal to main's), and gh attestation verify on one published binary with the scripts' flags; then delete the merged branches.
```

## 01M7EDSB0FS9QQ82VH34TGXVZE

- type: procedural
- agent: `cursor-agent`
- key: `process.release-review`
- scope: private
- created: 2026-11-15T01:40:11Z
- importance: 0.8
- metadata: `{"_mnemo.key":"process.release-review","area":"conventions"}`

```text
REVIEWER-B reviews each minor release at READY, before the tag: everything since the last release, the workflow changes, and the prior batch's triage. A patch release gets no review unless the Planner asks.
```

## 01M7PCC07YEX6KYAGPMY6BGR24

- type: procedural
- agent: `cursor-agent`
- key: `process.site-phone-check`
- scope: private
- created: 2026-11-18T03:49:21Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"process.site-phone-check","area":"process","topic":"site"}`

```text
Checking index.html at phone width without a browser tool: headless Chrome (/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --headless=new --user-data-dir=<scratch> --allow-file-access-from-files) can't make a window 390 px wide, so load the page in a 390 px iframe inside a wrapper page; render slices by shifting the iframe with a negative margin-top and a 1300 px tall window (sips --cropOffset doesn't crop by offset); measure overflow with a wrapper script reading iframe.contentDocument scrollWidth vs clientWidth (--dump-dom --virtual-time-budget=5000). macOS has no `timeout`: use perl -e 'alarm 90; exec @ARGV'.
```

## 01M7NWWEF2PTAEZDHERP4RXW4S

- type: procedural
- agent: `cursor-agent`
- key: `process.waiting-pointer`
- scope: private
- created: 2026-11-17T23:18:43Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"process.waiting-pointer","area":"process","topic":"coordination"}`

```text
When the Worker stops for the user (the release approval click, a token, a permission), it appends a short WORKER — WAITING entry to PLAN.md (what the user must do and where: run URL, step name) and sends planner-a `WAITING: <entry title>`, in addition to telling the user directly (user decision 2026-11-17; AGENTS.md tmux rule 1 and 6, committed with v0.18.1).
```

## 01M7EK1PETYR7ZX32683AQ5EWM

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.15.0`
- scope: private
- created: 2026-11-15T03:12:08Z
- importance: 0.9
- metadata: `{"_mnemo.key":"release.v0.15.0","area":"releases"}`

```text
v0.15.0 (tagged 2026-11-15) ships 12.3, no setup: generated keys kept in the OS keychain (keyring 4 family) with MN1- recovery codes, mnemo unlock, recovery-code and rekey --new-key, and the key sources MNEMO_KEY, MNEMO_KEY_FILE, MNEMO_KEY_COMMAND; the built-in embedder (snowflake-arctic-embed-xs on candle, 384-dim, identifier-aware recall, fetched once and hash-checked at every load; mnemo embedder install|status, --model-dir); mnemo mcp install; hybrid recall by default, keyword tokenizer 2 and mnemo reembed; LOCK-2 (a writer that has waited 500 ms gets the next turn); remember --key K --supersedes <id>; MSRV 1.94; init defaults to 384; stripped release builds; and every published artifact checked on its own target before the approval.
```

## 01M7G19VHDXG9MNBY9X1W2CFKC

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.16.0`
- scope: private
- created: 2026-11-15T16:40:30Z
- importance: 0.8
- metadata: `{"_mnemo.key":"release.v0.16.0","area":"decisions"}`

```text
v0.16.0 (12.2 and GUIDE-1, tagged 2026-11-15 from release-0.16.0): mnemo onboard (one brain per project at .git/mnemo/project.mnemo, the MCP server, hooks and skill or rule for Claude Code and Cursor, local scope by default, --undo, --plugin); mnemo hook (session-start injection under the framing line, pre-compact and session-end transcript saves without tool output and with secrets redacted, 50 turns per write, always exit 0); the guide built in (mnemo onboard --guide, llms.txt, llms-full.txt); the MCP server's resources and prompts; pathless brain commands and -C; install.sh and install.ps1 with SHA256SUMS, the attestation pinned to release.yml and the tag, and a version check; the attestation made after the approval; RB-011's fixes; the identical keyed re-write fix. Windows hooks untested. Mobile published (a minor release).
```

## 01M7G8K5JCXSNSGH8PEW062HKJ

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.16.1`
- scope: private
- created: 2026-11-15T18:47:55Z
- importance: 0.75
- metadata: `{"_mnemo.key":"release.v0.16.1","area":"decisions"}`

```text
v0.16.1 (a patch, tagged 2026-11-15 from release-0.16.1, PR 100): marker-only transcript turns ([thinking], tool calls, tool-result sizes) are kept in the thread but not embedded and never recalled (by flag or content, so v0.16.0 turns too), with Mnemo::ingest_thread_new_turns for an all-marker tail; mnemo get <ID>, search and snapshots take no path in a git repository, and a single get prints the whole content; onboard prints the guide first so its output ends with the human steps, plus a pass-on line without a terminal; the install scripts name any other mnemo on the PATH. Mobile not published (.github/release-mobile false): the packages stay at 0.16.0. Before the dogfood switch (7b). CLI-PATHLESS-2 (update, thread start, essence update|resolve|remove) carried, LOW, after the switch.
```

## 01M7HKW2M4AHVCRKPVEXSJ0F1X

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.17.0-ready`
- scope: private
- created: 2026-11-16T07:24:16Z
- importance: 0.75
- metadata: `{"_mnemo.invalid_at":1794820615,"_mnemo.key":"release.v0.17.0-ready","_mnemo.superseded_by":"01M7HTAB8TEY8PYX2Y12Y6JGF8","area":"release","topic":"v0.17.0-ready"}`

```text
v0.17.0 READY (2026-11-15): release-0.17.0 at b223f25 after PR 113 (the bump: every manifest, lockfile and mobile example at 0.17.0; CHANGELOG [0.17.0] with the mixed-version rule; .github/release-mobile true because the UniFFI binding changed and rule 3; the Swift and Kotlin install lines at 0.17.0). The dry run on release-0.17.0 with mobile on (run 43821159815) succeeded: every CLI, wheel, sdist, artifact check, crates.io and PyPI dry runs, Swift and Android builds and publishes, Swift verify; version check read engine, mnemo-uniffi and mnemo-android 0.17.0 and mobile true. The dry run goes on the release branch (the registry environments allow release-* branches only).
```

## 01M7HTAB8TEY8PYX2Y12Y6JGF8

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.17.0-ready`
- scope: private
- created: 2026-11-16T09:16:55Z
- importance: 0.75
- metadata: `{"_mnemo.key":"release.v0.17.0-ready","_mnemo.supersedes":"01M7HKW2M4AHVCRKPVEXSJ0F1X","area":"release","topic":"v0.17.0-ready"}`

```text
v0.17.0 READY again (2026-11-16): release-0.17.0 at ef0c1af after PR 114 (RB-013 and EPIPE-1). The fresh dry run (run 43835261485, mobile on) succeeded; version check read engine, mnemo-uniffi and mnemo-android 0.17.0 and mobile true; CI and Mobile CI passed on ef0c1af.
```

## 01M7K1HHJ4PW97JF25RJGZ43PN

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.17.1-ready`
- scope: private
- created: 2026-11-16T20:42:25Z
- importance: 0.75
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1794870087,"_mnemo.key":"release.v0.17.1-ready","_mnemo.superseded_by":"01M7K9G3B93JHSX9S84FAR626Q","area":"release","topic":"v0.17.1-ready"}`

```text
v0.17.1 READY (2026-11-16): release-0.17.1 at 59e1c03 after PR 115 (ONBOARD-UX), with the rule 9 handoff 1b6df66 before it. The dry run 43948527607 (mobile off; .github/release-mobile false) succeeded: version check read engine, mnemo-uniffi and mnemo-android 0.17.1; CI and Mobile CI passed on 59e1c03.
```

## 01M7K9G3B93JHSX9S84FAR626Q

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.17.1-ready`
- scope: private
- created: 2026-11-16T23:01:27Z
- importance: 0.75
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1794875186,"_mnemo.key":"release.v0.17.1-ready","_mnemo.superseded_by":"01M7KEBPYHHB93G33C8SDGCWCD","_mnemo.supersedes":"01M7K1HHJ4PW97JF25RJGZ43PN","area":"release","topic":"v0.17.1-ready"}`

```text
v0.17.1 READY again (2026-11-16): release-0.17.1 at 53f9e9e after PR 116 (three wording fixes at a terminal: 'needs attention', the brain line's key source, the trust step only when needed, 'Nothing to change; everything is already set up.'), on top of PR 115 (ONBOARD-UX) and the rule 9 handoff 1b6df66. The dry run 43972967392 (mobile off) succeeded; CI and Mobile CI passed on 53f9e9e.
```

## 01M7KEBPYHHB93G33C8SDGCWCD

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.17.1-ready`
- scope: private
- created: 2026-11-17T00:26:26Z
- importance: 0.75
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"release.v0.17.1-ready","_mnemo.supersedes":"01M7K9G3B93JHSX9S84FAR626Q","area":"release","topic":"v0.17.1-ready"}`

```text
v0.17.1 READY again (2026-11-16): release-0.17.1 at 6d0288a after PR 117 (onboard's terminal view takes a boot log's tags), on top of PR 116 (wording) and PR 115 (ONBOARD-UX). The dry run 43985452970 (mobile off) succeeded: 23 jobs, 9 skipped; CI and Mobile CI passed on 6d0288a.
```

## 01M7MR2N1QC4QJ00RA86DKDTKZ

- type: episodic
- agent: `cursor-agent`
- key: `release.v0.17.2-ready`
- scope: private
- created: 2026-11-17T12:35:29Z
- importance: 0.75
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"release.v0.17.2-ready","area":"release","topic":"v0.17.2-ready"}`

```text
v0.17.2 READY (2026-11-17): release-0.17.2 at bf73ef8, Run 1's five rounds (PR 118 READY-NOTICE, PR 119 presence at start and polish, PR 120 MODEL-RETRY and WHO-SHAPE, PR 121 the legacy scan and the claim race, PR 122 the bump) after the rule 9 handoff e8dbdd4. The dry run 44081832937 (mobile off) succeeded: 23 jobs, 9 skipped; version check read 0.17.2; CI and Mobile CI passed on bf73ef8.
```

## 01M7PF1C7HS3K4SW8C9GXQY9BS

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-0`
- scope: private
- created: 2026-11-18T04:35:59Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-0","area":"report","topic":"v0190-0"}`

```text
WORKER DONE v0190-0 (2026-11-17): v0.19.0 setup. Brain: "v0.19.0 setup: the release branch, the Run 2 rules commit, and the archives frozen" -> partial (the release-order and roadmap-status facts came back; decision.run-2 wasn't in the top 5, read by key). 1. release-0.19.0 branched from main at 9fb1191 (origin/main was 9fb1191). 2. d3f828a 'docs(agents): the brain is the ledger (HARNESS-DOGFOOD-1 Run 2)': AGENTS.md alone, +39 lines (the new subsection). 3. 72ae920 'chore(plan): freeze the coordination files as archives (Run 2)': PLAN.md and reviewer_notes.md by path, as they stood (+150 lines: the archive head notes and the last entries). From here neither file is edited. 4. Pushed: origin/release-0.19.0 at 72ae920. No PR, as the task says. Pointer: the Planner's new socket (11856). Tally: found by the READ: v0190-0 pointer and the per-turn line (1 task ready). Awkward: none.
```

## 01M7PGBF8RN8YQV054P2HN8WWP

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-1`
- scope: private
- created: 2026-11-18T04:58:58Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-1","area":"report","topic":"v0190-1"}`

```text
WORKER READY v0190-1 (2026-11-17): PERF-2 step 1, the baseline on v0.18.1 and docs/design/perf-2.md. PR 135 (8dceda9, branch rel0190-1-perf2-design) into release-0.19.0, left OPEN for your review (merge on your word; changes from the review go in first). No engine change.

Brain: "PERF-2 baseline first recall after opening vector column fill ANN index layout perf numbers" -> found (spec.perf-2 at the top; no stored v0.11 profile numbers, read the code and the spec's).

Setup: Apple M1 Pro 8 cores 16 GB; release build with --cfg aes_armv8/polyval_armv8 from .cargo/config.toml (RUSTFLAGS not set); release-0.19.0 at 72ae920, engine = v0.18.1 (git diff v0.18.1 HEAD -- src Cargo.toml Cargo.lock empty). Load average 3.2 to 6.4 during the runs (fileproviderd held one core); device_budget ran twice to show the noise.

Numbers (run 1 / run 2; ms unless marked; device_budget --sizes 1000,10000,100000):
- first recall, fresh handle, median of 20: 1K 0.92/0.92; 10K 10.38/9.16; 100K 118.29/116.45 (p95 run 2: 0.93, 10.21, 122.18)
- first recall after stats() loaded the index: 1K 0.33/0.33; 10K 9.14/8.85; 100K 6.31/6.37
- warm recall, median of 1,000: 1K 0.22/0.22; 10K 2.25/2.28; 100K 2.40/2.41
- open, median of 20: 0.54/0.52; 3.79/3.37; 30.47/30.16
- handle RSS MB: 2.9/2.9; 17.5/16.2; 112.5/112.5
- durable single write median: 14.99/14.01; 12.79/15.17; 14.63/15.53
- batch of 100 per memory: 0.256/0.280; 0.198/0.271; 0.376/0.388
- fsyncs single/batch (run 2): 3/4; 3/3; 3/4
- ranking_v2: hit@1 0.9231, MRR 0.9487; phase6 0.5667/0.6779 (floors 0.5467/0.6665)
- ann_defaults at 100K, defaults: 317 partitions, n_probe 159, n_rerank 500, recall@10 0.978, 3.78 ms/query

The note's choices:
(1) Where it goes: at 100K about 110 of 116 ms is the index load (ensure_ann: decode the ~17 MB run, the id->partition map, reconcile); the query is about 6 ms. At 10K (no index) it's the vector-column fill: 1,000 pages each a seek, a read and an AES-GCM decrypt.
(2) Index layout 2, partition-major, behind the existing v9 layout byte (no format bump): a head (centroids, PQ codebook, a partition directory; about 1 MB at 100K) decoded at first use; partitions loaded when a query probes them; the id->partition map built only when a write path needs it; delta Adds queued per unloaded partition, Removes in a handle-wide set; every new base written as layout 2, layout 1 still read. v0.18.1 on layout 2: unknown layout -> no index -> exact recall, no error; its writes are re-added by generation on the next v0.19 open; its build_index writes layout 1, still read.
(3) Pager::read_pages: one positioned read per run of pages (read_at / seek_read), then per-page auth and decrypt.
(4) 10K first recall: batched reads plus a parallel decrypt with scoped threads above a measured threshold; no new dependency, no unsafe.
(5) Warm 10K: dot_f16 already runs 8 lanes; try 16 lanes in two accumulators, and widening a whole entry before the dot; parallel warm scan only if those miss 2 ms.
(6) Shared mode: plain handles hold their lock for life, so lazy reads see the same run; shared handles load partitions only inside a call that passed the stamp check, and drop them on reload; page generations make a wrong-run read fail as PageAuthFailed. PERF-2 also cuts the 112.5 ms first recall every shared reload pays at 100K.
(7) The 4th fsync: FMT-9c direct writes of big control runs after Pager::flush already synced. Proposal: defer that sync until after the direct writes so one fsync covers both (4 -> 3); safe because both are unreferenced until the WAL commit and SEC-1 removed the lease ordering; never fold it into the commit's own fsync. Needs failpoint crash tests; your call whether it's in v0.19.0.
Open questions for you: the lever order (measure the 100K load's split first; head-only load may be enough at n_probe = half the partitions), 3f in v0.19.0 or its own step, and whether a parallel warm scan is in scope.

Deviations: ann_defaults_v0_6_0 panics on v9 at its 10K tier (no index below V9_ANN_MIN_FACTS = 20,000, so stats().index is None); re-run with MNEMO_DEV_ANN_MIN_FACTS=1000, its 10K row is a forced index; it writes test/scripts/perf_v0.5.0.json, restored from HEAD after each run (git status clean). The over-budget phase's first-recall numbers include a prior stats() call, so they aren't cold first recalls; the note says so.

Tally: found by READ: v0190-1 and the per-turn line. Awkward: fileproviderd at ~94% of a core during the benchmarks.
```

## 01M7PTY93EFSCWMJKZTEYTTZ61

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-2`
- scope: private
- created: 2026-11-18T08:04:00Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-2","area":"report","topic":"v0190-2"}`

```text
WORKER READY v0190-2 (2026-11-18): PERF-2 step 2, the build. PR 136 squash-merged into release-0.19.0 as 8a63540 (branch deleted). CI 26 of 26; the new compat (v0.18.1, linux) job installed the published 0.18.1 ("Installed package mnemo-engine v0.18.1") and passed 2 of 2. No format bump.

Brain: "PERF-2 step 2 build index layout 2 flat arrays read_pages parallel column fill" -> found (decision.perf-2-design, report.v0190-1, spec.perf-2).

Order as decided: PR 135 merged first (74982c4); the 100K split measured before choosing; stage A built; stage B not needed; 3b and 3c in; 3d single-thread levers only; 3f not built.

The 100K load split (examples/perf2_split.rs, dev hook __index_load_split; medians of fresh handles), layout 1 (v0.18.1's engine) -> layout 2: read+decrypt 15.69 -> 4.22 ms; decode 70.76 -> 0.70; id->partition map 1.08 (in the load) -> 1.02 (only when a write needs it); delta chain 0.00; reconcile 3.32 -> 3.02; the following first recall 30.22 -> 4.19; plain first recall 114.98 -> 11.54; run 17,298,907 -> 12,082,189 bytes.

Against every target and floor (device_budget, M1 Pro, release, hardware AES; v0.19.0 run 1 / run 2; load average 9.36 / 5.45 at each run's start, 5.08 for the single-thread run, macOS background work; test/scripts/perf_v0.19.0.json):
- first recall 100K: 11.50 / 11.50 ms (v0.18.1 116.45 to 118.29; target <=15) MET. One decrypt thread: 17.66 ms.
- first recall 10K: 4.22 / 4.20 ms (9.16 to 10.38; target <=5) MET. One thread: 8.19 ms.
- warm recall 10K: 2.35 / 2.31 ms (2.25 to 2.28; target <=2) NOT MET. Sixteen lanes in dot_f16 measured no gain, so it was reverted and the second lever not tried: the warm time isn't in the dot product; a profile is the next step if the target stays. Report it as 2.3 ms.
- warm recall 100K: 2.39 / 2.38 (2.40 / 2.41) held.
- open 100K: 25.78 / 25.70 ms (30.16 to 30.47) better. 10K: 3.02 / 3.00 (3.37 to 3.79).
- handle RSS 100K: 111.6 / 110.9 MB (112.5) held. 10K: 17.3 / 17.2 (16.2 to 17.5). 1K: 3.7 (2.9), +0.8 MB, noted.
- durable single write: 12.0 to 16.8 ms across sizes (12.8 to 15.5) held; fsyncs unchanged (3 single; batch 4 at 1K and 100K).
- ranking_v2 hit@1 0.9231, MRR 0.9487 (unchanged); phase6 0.5667 / 0.6779.
- recall@10 at 100K, defaults: 0.978, 2.93 ms/query (unchanged). ann_defaults now skips the 10K tier (below V9_ANN_MIN_FACTS) and wrote only to a scratch --out file.
- exact results below 20K: covered by the unchanged suites (all pass).
- v0.18.1 on a layout-2 file: no error, exact recall, its write picked up by v0.19 (tests/v0_18_compat.rs, CI).

What was built: index layout 2 (flat partition-major arrays behind the layout byte, every length checked before allocating, trailing bytes refused; every new base is layout 2, layout 1 still read and rewritten as 2 with the next base); Pager::read_pages_uncached (one positioned read per run of adjacent pages, AES-GCM in place, scoped threads from 64 pages) behind RunReader (256 pages per refill, read as a plain slice); Pager::for_each_page streaming the column fill (threads once per fill, 15-page/120 KiB buffers over a bounded channel); recall prefills the scored candidates' vector pages; the id->partition map lazy; dev hooks __index_load_split, __rewrite_index_base_for_testing, MNEMO_DEV_DECRYPT_THREADS.

Deviations found while measuring, all fixed before the final runs: (1) a single 256-page batch for the column fill added 2.4 to 3.7 MB to a 10K handle (allocator keeps 2 MiB buffers); 64-page batches fixed memory but restarted threads per batch (10K first recall 7.32 ms); streaming fixed both. (2) RunReader serving through the batch's per-page lookup slowed the catalog decode: open at 100K 36.18 vs 26.98 ms; RunReader now owns the decrypted buffer. (3) The 64-page parallel threshold was chosen, not measured; the single-thread numbers are recorded. (4) Stage B and 3e's shared-mode test not built (stage A met the target, as decided).

Tests: unit pager::batched_reads_match_single_reads and pager::streamed_reads_reach_the_sink_once_each (1 and 4 threads, order and gaps, damaged page, wrong generation, past the end, cached dirty page, failing sink); index::layout2_tests (round trip, lazy map, damaged runs as errors); tests/v0_18_compat.rs. Local: fmt, clippy CI features, clippy mnemo-python; full cargo test --features failpoints 777 passed 0 failed 1 ignored; mnemo-uniffi 23 passed; after the last change, fmt, clippy and 27 suites (open, recovery, crash, shared, compat, fixtures) 467 passed 0 failed.

Docs: CHANGELOG [Unreleased] Changed (the numbers, how, mixed versions: upgrade every session together on a brain with 20,000+ facts); mnemo/README.md performance history row v0.19.0; docs/design/perf-2.md status and section 7 "As built". For your HANDOFF, the root README states the old first-search numbers: line 695 area, line 743, the table row at 860 ("First search after opening ... 0.9 ms | 9 ms | 112 ms"), and lines 917 to 919 ("about 112 ms at 100K ... about 9 ms at 10K"); the site's roadmap "Next" lists "Faster first search in large brains". iPhone numbers weren't re-measured.

Tally: found by READ: decision.perf-2-design and the per-turn line (1 task ready). Awkward: macOS background work (load 5 to 26) during the final runs; two of my own fixes (memory, open time) only showed up in measurement, not tests.
```

## 01M7PX5FQ0A1PB3SF2G4W9EXEV

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-3`
- scope: private
- created: 2026-11-18T08:42:54Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1794991382,"_mnemo.key":"report.v0190-3","_mnemo.superseded_by":"01M7PX5QK2DZVZ4JCBJFD4TT60","area":"report","topic":"v0190-3"}`

```text
WORKER READY v0190-3 (2026-11-18): the 13a and findings public types are #[non_exhaustive]. PR 137 squash-merged into release-0.19.0 as the head commit now (branch deleted); CI 20 of 20.

Brain: "non_exhaustive public record types TaskSpec Task FindingSpec Finding NewWork message claim spec constructors" -> found (decision.non-exhaustive-0.19 at the top).

Done: #[non_exhaustive] on all 13 listed: TaskSpec, Task, TaskTaken, TaskClaim, NewWork, Message, Claim, Presence, Stale, KindCounts, FindingSpec, Finding, FindingFilter. None left out: each is a record callers build or read, and a later field on any is likely.

A correction to the task's wording: #[non_exhaustive] refuses a struct expression outside the defining crate even with ..Default::default() (E0639). So outside the crate (the CLI binary, the integration tests, Python, any user) callers start from a constructor or Default and set fields. Added TaskSpec::new(title) and FindingSpec::new(id, category, priority, title); Message and Claim now derive Default (the hook's unit test builds them); TaskSpec, FindingSpec, FindingFilter, NewWork and KindCounts already had Default. Destructuring patterns need a trailing `..`; reading fields is unchanged.

Updated: the CLI (task add, finding add, finding list, the hook's Planner findings line), the hook's unit test, the Python binding (task_add, finding_add, findings), 27 struct expressions in 9 integration test files (rewritten from the compiler's E0639 spans). uniffi and the doc tests needed nothing. No README or guide snippet used the old style. CHANGELOG [Unreleased] Changed: what struct-literal and destructuring code needs, with an example.

Checks: build --tests --examples (failpoints, benches); clippy with CI's features clean; cargo test --doc 6 passed; cargo check -p mnemo-uniffi; cargo check and clippy on mnemo-python clean; the affected suites (bin unit tests, tasks, findings, export_cli, inspect_cli, wake, visibility_freshness, v0_16/v0_17 compat, project_brain, mcp_context, hook_cli, coordination_plumbing, messages, claims) all passed locally. Full suite in CI only (last full local run on PR 136).

Tally: found by your VERIFIED note on v0190-2 and the per-turn line. Awkward: the task's ..Default::default() assumption doesn't hold for non_exhaustive types.
```

## 01M7PX5QK2DZVZ4JCBJFD4TT60

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-3`
- scope: private
- created: 2026-11-18T08:43:02Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-3","_mnemo.supersedes":"01M7PX5FQ0A1PB3SF2G4W9EXEV","area":"report","topic":"v0190-3"}`

```text
WORKER READY v0190-3 (2026-11-18): the 13a and findings public types are #[non_exhaustive]. PR 137 squash-merged into release-0.19.0 as b97e689 (branch deleted); CI 20 of 20.

Brain: "non_exhaustive public record types TaskSpec Task FindingSpec Finding NewWork message claim spec constructors" -> found (decision.non-exhaustive-0.19 at the top).

Done: #[non_exhaustive] on all 13 listed: TaskSpec, Task, TaskTaken, TaskClaim, NewWork, Message, Claim, Presence, Stale, KindCounts, FindingSpec, Finding, FindingFilter. None left out: each is a record callers build or read, and a later field on any is likely.

A correction to the task's wording: #[non_exhaustive] refuses a struct expression outside the defining crate even with ..Default::default() (E0639). So outside the crate (the CLI binary, the integration tests, Python, any user) callers start from a constructor or Default and set fields. Added TaskSpec::new(title) and FindingSpec::new(id, category, priority, title); Message and Claim now derive Default (the hook's unit test builds them); TaskSpec, FindingSpec, FindingFilter, NewWork and KindCounts already had Default. Destructuring patterns need a trailing `..`; reading fields is unchanged.

Updated: the CLI (task add, finding add, finding list, the hook's Planner findings line), the hook's unit test, the Python binding (task_add, finding_add, findings), 27 struct expressions in 9 integration test files (rewritten from the compiler's E0639 spans). uniffi and the doc tests needed nothing. No README or guide snippet used the old style. CHANGELOG [Unreleased] Changed: what struct-literal and destructuring code needs, with an example.

Checks: build --tests --examples (failpoints, benches); clippy with CI's features clean; cargo test --doc 6 passed; cargo check -p mnemo-uniffi; cargo check and clippy on mnemo-python clean; the affected suites (bin unit tests, tasks, findings, export_cli, inspect_cli, wake, visibility_freshness, v0_16/v0_17 compat, project_brain, mcp_context, hook_cli, coordination_plumbing, messages, claims) all passed locally. Full suite in CI only (last full local run on PR 136).

Tally: found by your VERIFIED note on v0190-2 and the per-turn line. Awkward: the task's ..Default::default() assumption doesn't hold for non_exhaustive types.
```

## 01M7PYQ7R536FSB69W4A8ESXR6

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-4`
- scope: private
- created: 2026-11-18T09:10:04Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-4","area":"report","topic":"v0190-4"}`

```text
WORKER READY v0190-4 (2026-11-18): the inspector's date range and the Findings screenshot. PR 138 squash-merged into release-0.19.0 as dabb5fc (branch deleted); CI 20 of 20.

Brain: "inspector date range filter memories findings tasks created_at screenshot demo brain" -> found (spec.inspect-date-filter).

(1) The date range (spec.inspect-date-filter): From and To dates, inclusive, in the viewer's local time, on Memories, Findings and Tasks; either may be empty. The page turns them into local day bounds (a DST day is 23 or 25 hours: Nov 1 to Nov 2 here spans 49 hours, checked). Server-side for all three routes, since /api/memories is paged: since/until in unix seconds; memories and findings by created_at, tasks by created_at or updated_at (date=updated, the tab's "Last change"). Combines with every filter; totals and status counts follow the range (the other filters still narrow only the list). Malformed since/until/date -> 400 with the reason. With a range and nothing in it, the tab keeps its filters and says "Nothing in this date range." Also: a link can open a tab (#findings), the code exchange keeps the hash, picking a tab updates it. Rules unchanged: GET only, text nodes only (no HTML sink), no handle held, nothing written.

(2) The screenshot: docs/img/inspect-findings.png, 101,720 bytes, light theme, the Findings tab (counts line, filters with the date inputs, DEMO-1.1 to DEMO-2.2 in every status, the "private until verified" mark on the unverified SECURITY finding). Built from a synthetic brain (mnemo/examples/inspect_demo_brain.rs: made-up text, role names reviewer/planner/worker, 3 tasks, 3 memories) by test/scripts/inspect_screenshot.sh: temp directory, throwaway passphrase, inspector run from inside it so the header shows only demo.mnemo (the first take showed the /var/folders temp path; fixed), headless Chrome with preferredColorScheme light, 1200x540, checks the 300 KB limit. Never touches the project brain or the keychain. Retake: sh test/scripts/inspect_screenshot.sh.

Tests: inspect_cli::the_date_range_filters_every_dated_tab (backdated memories: inclusive bounds, open ends, empty values, an empty window, with type and q; findings and tasks totals and counts follow the range for date=created|updated; five malformed requests -> 400; the page has the date inputs and hash handling, no HTML sink). The page's script parses in JavaScriptCore and ran against a stub DOM (routes get the same since/until; the range empty state). Local: fmt, clippy CI features, inspect_cli 11, guide 5. Docs: CHANGELOG Added, mnemo/README.md's inspector section, one guide line (llms-full.txt regenerated).

A slip, fixed before anything was pushed: I committed on my local release-0.19.0 instead of a task branch; the push failed (no such branch), so I moved my own unpushed commit to rel0190-4-inspect-dates (git switch -c) and pointed local release-0.19.0 back at origin (git branch -f, only my commit was ahead). Nothing reached GitHub outside the PR.

For v0190-5: the image is at docs/img/inspect-findings.png (1200x540).

Tally: found by READ: v0190-3 and the per-turn line. Awkward: the missing branch, above.
```

## 01M7PYTFGGWMJBVB5GKD7W1N0H

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-5`
- scope: private
- created: 2026-11-18T09:11:50Z
- importance: 0.5
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-5","area":"report","topic":"v0190-5"}`

```text
WORKER DONE v0190-5 (2026-11-18): the hand-off committed. 3ba2f7b 'docs(site): the inspector screenshot; one roadmap line' on release-0.19.0, README.md and index.html alone (16 insertions, 8 deletions), pushed. No PR, as the task says (a hand-off commit on the release branch).

Brain: "Hand-off README.md index.html inspector screenshot roadmap duplicate line" -> partial (earlier hand-off notes came back).

Checked before committing: the README's new first-search row (0.6 / 4.2 / 12 ms) and its limitation text (about 12 ms at 100K and 4 ms at 10K, v0.19.0) match the measured runs (perf_v0.19.0.json: 0.60/0.61, 4.22/4.20, 11.50/11.50 ms; 12 is 11.5 rounded); the iPhone number is marked as v0.12.0's, not re-measured. index.html: the screenshot figure (width 1200 height 540, alt text, lazy), the shipped line 'v0.13 – v0.19' with 'A first search about ten times faster ... 12 ms at 100,000 memories' (116 to 118 ms -> 11.5 ms is about 10x), the inspector line with the date range, and both first-search roadmap lines (Next and After that) removed. docs/img/inspect-findings.png is on the branch (PR 138), at the path both files reference. No CI runs for these paths (paths-ignore).

Tally: found by READ: v0190-4 and the per-turn line. Awkward: none.
```

## 01M7Q1QTX4XHDRKBRYAXWE1JCC

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-6`
- scope: private
- created: 2026-11-18T10:02:49Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-6","area":"report","topic":"v0190-6"}`

```text
WORKER READY v0190-6 (2026-11-18): v0.19.0 READY. release-0.19.0 at 9b78312 (PR 139, branch deleted); origin/main is an ancestor.

Brain: "v0.19.0 READY release bump mobile rule CHANGELOG backup docs/brain.md dry run" -> found (process.brain-backup).

The bump (PR 139, squash 9b78312): every manifest and the lockfiles' own entries at 0.19.0 (engine, Python Cargo and pyproject, Node, mnemo-uniffi, Kotlin VERSION_NAME, the iOS and Android samples). Mobile by RELEASING.md's rule: .github/release-mobile true (every minor version publishes mobile once; v0190-3 changed no type the bindings use, and git diff --stat v0.18.0 -- mnemo/bindings/{uniffi,swift,kotlin} shows version lines only); the Swift and Kotlin READMEs name 0.19.0. CHANGELOG [0.19.0] — 2026-11-18 with an intro (numbers from perf_v0.19.0.json) and the compare links.

The backup (step 7): docs/brain.md re-exported by the installed mnemo 0.18.1 (the task's "installed release"; 0.19.0 isn't installable before the tag; 0.18.1's export --git has every filter this release needs, none were added in v0.19.0). 326 records (292 before): 34 added (20 keyed facts: reports, specs, decisions, status, process; 7 tasks; 6 project-essence items, the first in a backup), 2 changed, none removed. Reviewed by scans and by reading every added essence item: 0 findings, private records, messages, claims, presence, turns, thread essence; no emails, no home or temp paths; the one github_pat_ match is the redaction rules' description, as before; RB-006.12 appears by id only (the essence's open item, "daemon design, SECURITY.local.md"). A second export was byte-equal; tests/brain_backup.rs passes (header "written by mnemo 0.18.1"). No chore(plan) commit: the archives are frozen.

CI: PR 139 26 of 26 after one re-run. Attempt 1: rust (macos-latest) failed shared_valve::two_writers_and_four_readers_all_get_their_turns at tests/shared_valve.rs:193, "readers stopped getting in": reads per 500 ms [140, 47, 0, 103, 79, 33], commits [13, 14], longest read wait 739 ms, on a runner with GitHub's macOS capacity notice. The test passed on PRs 136, 137 and 138 and on the push runs of 8a63540, b97e689 and dabb5fc since PERF-2 merged; PR 139 touched only versions and docs. Re-run of the failed job passed. Logged like shared_lock: no change unless it recurs.

The dry run: 44278116677 on 9b78312, -f mobile=true, success, 27 jobs. version check: "engine 0.19.0, mnemo-uniffi 0.19.0, mnemo-android 0.19.0", "the commit says mobile: true", "mobile: on". Maven Central: deployment 4835d2de-... VALIDATED, then dropped (HTTP 204). Skipped, as a dry run does: approve, attest, the Release, the gate, the mobile-off note. Push runs on 9b78312: CI and Mobile CI success.

Left for the release, as RELEASING.md and Run 2 order it: REVIEWER-B's READY pass (you call it); your hand-off of the version text, still 0.18.1 at: index.html line 466 (the pill), 767 ("These examples use v0.18.1"), 792 (the example string); README.md 178 (the example string) and a "What's new in v0.19.0" section (the README's newest is v0.18.1's, line 200); then main's fast-forward, the tag v0.19.0, and the user's approval click (WAITING: at the stop).

Tally: found by READ: v0190-5 and the per-turn line. Awkward: the macOS timing flake.
```

## 01M7Q2RXY1YJXY7KHVPAQCWRQD

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-7`
- scope: private
- created: 2026-11-18T10:20:54Z
- importance: 0.5
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-7","area":"report","topic":"v0190-7"}`

```text
WORKER DONE v0190-7 (2026-11-18): the v0.19.0 version text. b345073 'docs: v0.19.0 version text' on release-0.19.0, README.md and index.html alone (27 insertions, 8 deletions), pushed; no PR (a hand-off commit), no CI for these paths.

Brain: "v0.19.0 version text README index.html hand-off" -> found (report.v0190-5 and the v0.14.0 hand-off note).

Opened after REVIEWER-B's pass (v0190-review done; RB-015.1 and RB-015.2 LOW, accepted for v0.19.1 as v0191-1, neither holds the tag).

Checked before committing: the README's What's new in v0.19.0 numbers match perf_v0.19.0.json (11.5 ms vs about 117: 116.45 to 118.29; 4.2 ms vs about 10: 9.16 to 10.38; opening 26 vs 30: 25.70/25.78 vs 30.16/30.47); the mobile lines at 0.19.0; the example string. index.html: the pill, 'These examples use v0.19.0', the example string, and 35 tagged releases (git tag -l 'v*' counts 34 now; the v0.19.0 tag makes 35).

Tally: found by READ: v0190-7 and the per-turn line. Awkward: none.
```

## 01M7Q3X89EFAYCE0S6BS5VX2T1

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-8`
- scope: private
- created: 2026-11-18T10:40:44Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1795015067,"_mnemo.key":"report.v0190-8","_mnemo.superseded_by":"01M7QKRHFG0CTHV506XNDGTH85","area":"report","topic":"v0190-8"}`

```text
WORKER WAITING v0190-8 (2026-11-18): the v0.19.0 release waits for the user's approval click.

What the user must do: approve the release at https://github.com/toarchkumar/mnemo/actions/runs/44284629207, the step "approve the release (v0.19.0, mobile true)". All 21 jobs before it succeeded.

Brain: "v0.19.0 release fast-forward main tag approval stop WAITING after-tag checks" -> found (status.v0.19.0, and the v0.18.0 and v0.18.1 release records).

Done before the stop: main fast-forwarded 9fb1191..b345073 (9 commits, first try; origin/main was an ancestor); the annotated tag v0.19.0 ("MNemo 0.19.0") on b345073, pushed; release run 44284629207 on b345073. b345073 itself has no CI run (README.md and index.html only, paths-ignore); 9b78312, the last code commit, is green on CI and Mobile CI, and its dry run 44278116677 succeeded with mobile on.

After the click: crates.io, PyPI, mnemo-swift 0.19.0, Maven Central, the GitHub Release's assets, the site's pill; then delete release-0.19.0; this report is superseded by the DONE.
```

## 01M7QKRHFG0CTHV506XNDGTH85

- type: episodic
- agent: `cursor-agent`
- key: `report.v0190-8`
- scope: private
- created: 2026-11-18T15:17:47Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0190-8","_mnemo.supersedes":"01M7Q3X89EFAYCE0S6BS5VX2T1","area":"release","topic":"v0.19.0"}`

```text
report.v0190-8 DONE (2026-11-18): v0.19.0 released. Recall at start: found (status.v0.19.0, the task body). main fast-forwarded 9fb1191..b345073; annotated tag v0.19.0 on b345073. Release run 44284629207 (mobile true) approved by the user and concluded success; the only non-success job is 'mobile (skipped, release note)', skipped as designed. After-tag checks, each from a command: crates.io mnemo-engine max_version 0.19.0; PyPI mnemo-engine 0.19.0 with 6 files; toarchkumar/mnemo-swift tag 0.19.0 (950d7b7); Maven Central repo1 maven-metadata latest and release 0.19.0; GitHub Release v0.19.0 not draft, not prerelease, 14 assets including mnemo-android-0.19.0.aar, MnemoFFI-0.19.0.xcframework.zip, 5 CLI binaries, 5 wheels, the sdist and SHA256SUMS; the site names v0.19.0 (the pill). release-0.19.0 deleted on origin and locally; the working tree is on main at b345073. Supersedes the WAITING version of this report.
```

## 01M7R3NC3BHJJV2B2S634K01G3

- type: episodic
- agent: `cursor-agent`
- key: `report.v0191-0`
- scope: private
- created: 2026-11-18T19:55:40Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0191-0","area":"release","topic":"v0.19.1"}`

```text
report.v0191-0 DONE (2026-11-18): v0.19.1 setup. Recall at start: found (status.run-2: Run 2 PASSED; the user's decision 01M7R3H66X0PAAPQXPS7CYVXZH that the brain carries the work permanently, AGENTS.md rewritten as option 1). (1) release-0.19.1 branched from main at b345073 and pushed. (2) The Planner's AGENTS.md rewrite of 'Multi-agent roles & coordination' committed alone as 470b9de 'docs(agents): the brain carries the work (Run 2 made permanent)'; the diff touched only that section (one hunk from line 265). (3) My own commit 74e0569 'docs(agents): the dogfood section matches the coordination rewrite', a direct commit on release-0.19.1 (no PR): tmux rule 9 is now rule 10; the Worker writes facts, the Planner the project essence and keyed decision.*, spec.* and status.* facts, the Reviewers only read; since v0.17 the default agent cursor-agent gives a keyed write its slot, so the examples no longer name the agent. No other 'tmux rule 9' reference is left in AGENTS.md. (4) Pushed both. No cloud-sync conflict files found.
```

## 01M7RA4HAYE1C6WTT48Y9G14RG

- type: episodic
- agent: `cursor-agent`
- key: `report.v0191-1`
- scope: private
- created: 2026-11-18T21:48:48Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0191-1","area":"release","topic":"v0.19.1"}`

```text
report.v0191-1 DONE (2026-11-18): RB-015.1 and RB-015.2, PR 140 squash-merged into release-0.19.1 as ac8725b (branch deleted). Recall at start: partial (the PERF-2 and layout-2 facts came back; the findings themselves came from finding list and finding get, which recall doesn't return). RB-015.1, as triaged: a damaged index run of a known layout (PageAuthFailed, Crypto, Serialize, Invalid, dimension mismatch; an I/O error still fails) loads as no index in ensure_ann; recall scans exactly; the handle adds one notice ('the approximate index is damaged (...), so recall scans every memory; your memories are intact, and ... mnemo index --rebuild ...'), which the CLI prints after recall, mnemo mcp logs after the call, and a shared handle carries into its notices; the handle's next commit that writes (dirty catalog or cache) rebuilds the index in flush (repair_index), or drops the damaged run below the ANN threshold. A writer that had to load the index inside flush no longer fails on damage. verify: new additive Mnemo::verify_index() -> IndexCheck (non_exhaustive: None, Ok{vectors, delta_runs}, UnknownLayout, Damaged); VerifyReport is unchanged; mnemo verify prints the index on its own line (a damaged index is a warning, exit 0); Python verify_index() returns a dict. Also found and fixed: build_index or drop_index on a handle that never loaded the index left the old delta runs allocated (a test measured 18 free pages against 21); a damaged delta run's own pages are now tracked and freed. Not changed: restore_to on a snapshot with a damaged index still errors (an explicit restore). RB-015.2: (1) fuzz_index_layout2 via __fuzz decode_index_layout2 and mutate_index_layout2, in the ci.yml fuzz-smoke and fuzz-weekly matrices, plus a seeded test (20,000 edit lists, 2,000 random buffers); no local nightly or cargo-fuzz, the target type-checks on stable and CI fuzz-smoke passed; (2) v0_18_compat at 20,100 notes: v0.18.1 recalls and imports before its index, and v0.19 finds the import; (3) selectTab uses Object.hasOwn, with an inspect_cli assertion; (4) no change. Tests: local cargo test --features failpoints 787 passed, 0 failed, 1 ignored; clippy with CI features and on mnemo-uniffi clean; v0_18_compat against the installed 0.18.1 on temp files 2 passed, 0 skipped; Python tests on a dev-hooks,embed-candle wheel all passed. CI on PR 140: 27 pass, after re-running two macOS jobs (rust macos, swift iOS simulator) cancelled for runner capacity ('not acquired by Runner'). CHANGELOG Unreleased has Fixed, Added and Changed entries; mnemo/README verify row and mnemo-python/README verify_index row updated.
```

## 01M7RHH1P3BPPEJ4H3ZFNX1E40

- type: episodic
- agent: `cursor-agent`
- key: `report.v0191-1b`
- scope: private
- created: 2026-11-18T23:57:58Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0191-1b","area":"report","topic":"v0191-1b"}`

```text
report.v0191-1b DONE (2026-11-18): PR 141 squash-merged into release-0.19.1 as 5503e3c (branch deleted). Recall at start: found (report.v0191-1 and the RB-015.1 design came back). Change: in store/ann.rs ensure_ann, a load_persisted_index error that isn't index damage (an I/O error) sets ann_unloaded back to true before it's returned, so the handle's next call tries the load again; a damaged run still loads as no index with the notice. Test support: a process-wide page read failpoint, __failpoints::set_page_read_fails(page, count) (failpoints feature only), checked in read_exact_at, read_page and read_page_uncached. Test: tests/index_load_retry.rs, its own binary (1000 notes, index built; first recall after reopen gets an injected read failure on the base run's middle page, so MnemoError::Io with no damage notice; the next recall returns the right note, RecallMode::Indexed, verify_index Ok with 1000 vectors). With the fix taken out it failed at 'the index is still to load'. CHANGELOG [Unreleased] Fixed: one bullet. Local: cargo test -q --features failpoints gave 788 passed, 0 failed, 1 ignored over 71 suites (load average 11.55 at start); fmt and CI-feature clippy clean. Deviation: the first CI run failed in all 9 build jobs because CI's stable rustc 1.100.0 deprecates AtomicU32::fetch_update and builds with warnings denied (this Mac has rustc 1.95.0). The second commit (016797f) uses a compare_exchange loop; after it, 21 of 21 checks passed on the head. Saved as fact.ci-toolchain-gap.
```

## 01M7RKJ20MCNE9CRR23YMZK0J3

- type: episodic
- agent: `cursor-agent`
- key: `report.v0191-2`
- scope: private
- created: 2026-11-19T00:33:29Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0191-2","area":"report","topic":"v0191-2"}`

```text
report.v0191-2 READY (2026-11-18): v0.19.1 READY. release-0.19.1 at 6838c69 (PR 142 squash-merged, branch deleted); origin/main is an ancestor. Recall at start: found (the backup procedure 01M7NBTP73WBC2SHVX5M4SE7WY, the v0.19.0 READY report, and the mobile-off patches 0.17.1, 0.17.2, 0.18.1 from git show <tag>:.github/release-mobile). The bump (commit 146c9ff in PR 142): every manifest and the lockfiles' own entries at 0.19.1 (engine, Python Cargo and pyproject, Node, mnemo-uniffi, Kotlin VERSION_NAME, the iOS and Android samples), the same 11 files as the v0.18.1 bump. Mobile by the rule: .github/release-mobile false. No mobile code changed (git diff --stat b345073..release-0.19.1 -- mnemo/bindings examples/mobile is empty), 0.19.0 already published mobile for this minor version, and I didn't count the damaged-index fix under rule 2: memories were never at risk, and before the fix the damage gave an error, never a wrong answer. The Swift and Kotlin READMEs keep 0.19.0. The Planner may flip the mode before the tag if it reads rule 2 the other way. CHANGELOG: [Unreleased] cut into [0.19.1] — 2026-11-18 with an intro and its compare link. The backup (commit c2141db, on its own): docs/brain.md by the installed mnemo 0.18.1, 354 records. Deviation: the task says the installed release is 0.19.0, but ~/.local/bin/mnemo --version prints 0.18.1 (the only other mnemo on the PATH, ~/.cargo/bin, is 0.12.3). I'm not to install a release on this Mac, and v0.19.0's backup was also 0.18.1's. No export filter changed since (the CHANGELOG has none), and tests/brain_backup.rs passes (it needs 0.18.0 or newer). I read the diff before committing: no secrets, no SECURITY records; the only findings in it are RB-015.1 and RB-015.2, both LOW correctness and fixed in this release. CI: PR 142 27 of 27 on c2141db; the push runs on 6838c69, CI and Mobile CI, both success. The dry run: 44427600695 on 6838c69, -f mobile=false, success: 23 jobs succeeded and 9 were skipped. version check printed 'engine 0.19.1, mnemo-uniffi 0.19.1, mnemo-android 0.19.1', 'the commit says mobile: false' and 'mobile: off'. Version text for the Planner's hand-off: index.html line 466 (the pill, v0.19.0), 753 ('35 tagged releases'; git tag -l 'v*' counts 35 now, 36 after v0.19.1), 767 ('These examples use v0.19.0'), 792 (the example string); README.md 178 (the example string) and, if you want one, a 'What's new in v0.19.1' section (the newest, line 200, is v0.19.0's). These stay at 0.19.0 because mobile is off: README.md 107, 111, 116 and 217. These are v0.19.0 measurements, not version text: 881 and 940. Left: the hand-off, main's fast-forward, the tag, the user's approval click (v0191-3).
```

## 01M7RKKTB2EEXRGNQAPSSM5J8E

- type: episodic
- agent: `cursor-agent`
- key: `report.v0191-2h`
- scope: private
- created: 2026-11-19T00:34:26Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0191-2h","area":"report","topic":"v0191-2h"}`

```text
report.v0191-2h DONE (2026-11-18): the v0.19.1 version text. 2d21ac6 'docs: v0.19.1 version text' on release-0.19.1, README.md and index.html alone (17 insertions, 5 deletions), pushed; no PR (a hand-off commit). Recall at start: found (report.v0190-7 and the v0190-5 hand-off records). Checked before committing: index.html has the pill, 'These examples use v0.19.1' and the example string at v0.19.1, and '36 tagged releases' (git tag -l 'v*' counts 35 now, so 36 with v0.19.1); README.md has the example string and a 'What's new in v0.19.1' section, whose claims match the CHANGELOG's [0.19.1] (the damaged index scans and rebuilds on the next write, index --rebuild, verify on its own line, verify_index in Rust and Python, mobile stays 0.19.0). The mobile lines (README 107, 111, 116, 217) are unchanged. Left: v0191-3 (main's fast-forward, the tag, the approval click).
```

## 01M7RMHBP2K1BSE9BV9HYW8DY6

- type: episodic
- agent: `cursor-agent`
- key: `report.v0191-3`
- scope: private
- created: 2026-11-19T00:50:34Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1795053723,"_mnemo.key":"report.v0191-3","_mnemo.superseded_by":"01M7RRM7BZPS1A9J5CSBM6DYVJ","area":"report","topic":"v0191-3"}`

```text
report.v0191-3 WAITING (2026-11-18): the v0.19.1 release waits for the user's approval click. What the user must do: approve the release at https://github.com/toarchkumar/mnemo/actions/runs/44430183319, the step 'approve the release (v0.19.1, mobile false)'. All 19 jobs before it succeeded (version check, sdist and its check, 5 CLI binaries, 5 wheels, SHA256SUMS, 5 artifact checks); the 2 mobile builds were skipped, as mobile is off. Recall at start: found (the release order 01M7G19V4K51V9C7SZW3W3199E and report.v0190-8). Done before the stop: main fast-forwarded b345073..2d21ac6 (first try; origin/main was an ancestor); the annotated tag v0.19.1 ('MNemo 0.19.1') on 2d21ac6, pushed. 2d21ac6 changes only README.md and index.html (no CI by paths-ignore); 6838c69, the last code commit, is green on CI, Mobile CI and dry run 44427600695. After the click: crates.io and PyPI 0.19.1, the GitHub Release's assets with the mobile-off note, mnemo-swift and Maven Central stay at 0.19.0, the site's pill; then delete release-0.19.1. This report is superseded by the DONE.
```

## 01M7RRM7BZPS1A9J5CSBM6DYVJ

- type: episodic
- agent: `cursor-agent`
- key: `report.v0191-3`
- scope: private
- created: 2026-11-19T02:02:03Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0191-3","_mnemo.supersedes":"01M7RMHBP2K1BSE9BV9HYW8DY6","area":"report","topic":"v0191-3"}`

```text
report.v0191-3 DONE (2026-11-18): v0.19.1 released. Recall at start: found (the release order 01M7G19V4K51V9C7SZW3W3199E and report.v0190-8). main fast-forwarded b345073..2d21ac6; annotated tag v0.19.1 ('MNemo 0.19.1') on 2d21ac6. Release run 44430183319 (mobile false): approved by the user (GitHub's approvals API: approved by the repo owner, environment release), concluded success; 27 jobs succeeded, 5 skipped (the mobile build, publish and verify jobs, as mobile is off). attest the binaries, attach engine assets and engine published (gate) succeeded. After-tag checks, each from a command: crates.io mnemo-engine max_version 0.19.1; PyPI mnemo-engine 0.19.1 with 6 files; GitHub Release v0.19.1 not draft, not prerelease, 12 assets (5 CLI binaries, 5 wheels, the sdist, SHA256SUMS), with 'Mobile packages not published in this release (latest: Swift 0.19.0, Android 0.19.0)'; mnemo-swift's newest tag 0.19.0 and Maven Central latest and release 0.19.0, as intended; the site's pill says v0.19.1, and Pages serves install.sh, install.ps1, llms.txt and llms-full.txt byte-equal to main's (SHA-256 compared). The aarch64-apple-darwin binary, downloaded to the scratchpad (not installed), matched SHA256SUMS, and gh attestation verify with the install script's flags (--signer-workflow release.yml, --source-ref refs/tags/v0.19.1) exited 0. release-0.19.1 deleted on origin and locally; the working tree is on main at 2d21ac6, clean apart from the untracked .claude/. Supersedes the WAITING version of this report.
```

## 01M7RVQVZFE7873E9RDG20CGK3

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-0`
- scope: private
- created: 2026-11-19T02:56:28Z
- importance: 0.5
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-0","area":"report","topic":"v0200-0"}`

```text
report.v0200-0 DONE (2026-11-18): v0.20.0 setup. Recall at start: found (spec.v0.20.0 01M7RVPRDNEVEH4GMRYKX0AX5V by history --key: (A) 3f, the fsync merge; (B) shared access steps 6 and 8, a design note and baseline for step 6 first). release-0.20.0 branched from main at 2d21ac6 (v0.19.1's tag commit) and pushed; git ls-remote shows refs/heads/release-0.20.0 at 2d21ac6, and it didn't exist on origin before. The working tree is on release-0.20.0, clean apart from the untracked .claude/; no cloud-sync conflict files.
```

## 01M7S4W3V523VZGXYTHDX2W9XH

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-1`
- scope: private
- created: 2026-11-19T05:36:04Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-1","area":"report","topic":"v0200-1"}`

```text
report.v0200-1 DONE (2026-11-18): the fsync merge, PERF-2 3f, part (A) of spec.v0.20.0. Recall at start: found (spec.v0.20.0 by history --key; docs/design/perf-2.md section 3f). PR 143 (commit 0a8f8fe) squash-merged into release-0.20.0 as f2898a9, CI 21 of 21 passed. The change: Pager::write_dirty writes the data pages unsynced; the new stage_commit writes FMT-9c's direct runs and ends with one barrier (Pager::sync when Pager::unsynced) before wal::commit; the commit keeps its own fsync, never merged with the barrier; Pager::flush and the direct_writes field are gone. flush saves (header, free, free_dirty) before stage_commit and restores them on its error, so a retry on the same handle commits over the stored state; without the restore, the barrier-retry test fails with PageAuthFailed on reopen. Measured (device_budget --sizes 1000,10000,100000, this Mac): batch of 100 fsyncs before->after 1K 4->3, 10K 3->3, 100K 4->3; single write 3->3 at every size. Batch median ms 26.19->21.02, 55.98->20.86, 34.61->32.77; single median ms 14.97->13.06, 34.06->13.02, 17.48->13.57. Load before 4.86->8.08, after 5.35->3.28: the times are noisy and not a claim. Tests: tests/fsync_merge.rs, 6 tests: an I/O trace showing one barrier before each commit; 3 fsyncs; a crash at every write and a failed sync at each of the three leave the file pre or post (a failed barrier leaves it pre); retries after a failed barrier and after every write before it commit cleanly. Also prop_big_batch_commits_crash_pre_or_post (12 cases). Without the barrier, three fsync_merge tests fail. Local, rustc 1.95.0: cargo test --features failpoints 795 passed, 0 failed, 1 ignored, 72 suites; fmt clean; clippy -D warnings clean with CI's features, for -p mnemo-uniffi, and with no features. New failpoints: set_syncs_until_fail and an I/O trace (fact.io-trace-failpoints). No format change. Deviation: none from the spec. For triage, pre-existing and not fixed: fact.post-commit-retry-bug. On an exclusive handle, a flush that fails at the WAL commit write or the first checkpoint page write, retried on the same handle, returns Ok, but the reopen fails PageAuthFailed. It's the same on release-0.20.0 at 2d21ac6 and on this branch (temporary test, deleted).
```

## 01M7S4ZZA69GFYANM0AHNXXC3X

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-1h`
- scope: private
- created: 2026-11-19T05:38:10Z
- importance: 0.5
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-1h","area":"report","topic":"v0200-1h"}`

```text
report.v0200-1h DONE (2026-11-18): hand-off of docs/design/sync.md. Recall at start: found (status.sync-design 01M7S2N0VY11TV5GG6BJ2CRTJK: the user approved the note 2026-11-18; this hand-off commits it alone). Committed the one path, unedited (244 lines), as its own commit b4d81d9 'docs(design): the encrypted sync design note' on release-0.20.0, after v0200-1's merge f2898a9. Pushed: origin/release-0.20.0 f2898a9..b4d81d9. Working tree clean apart from the untracked .claude/. No change requests for the note. Note on claims: task next has no slug option and gives the oldest queued task first, so reaching v0200-1h took v0200-2 twice. Both times I put it back to queued with a note, untouched. Its claim history shows this.
```

## 01M7SAJNHKAQ6TKK6BD021B710

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-1r`
- scope: private
- created: 2026-11-19T07:15:46Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-1r"}`

```text
report.v0200-1r (worker-a, 2026-11-18): DONE. A flush retried after a failure at or past the commit point now refuses (MaybeStored) instead of returning Ok over a file that won't open.
RECALL: found. fact.post-commit-retry-bug (its evidence: write k=2 and k=3 of a 1-memory flush over 200 memories, retried, reopen PageAuthFailed(20); identical on 2d21ac6/v0.19.1 and the v0200-1 branch).
CAUSE: prepare_for_flush builds CatalogDelta { prev: ptr_tuple(self.header.catalog_delta) } and IndexDeltaRef { prev: ptr_tuple(self.header.index_delta) } from the header. After a failure at or past the commit point the header still holds the failed attempt's run pointers (page 20, never written when write 2 or 3 failed), so the retry's delta linked to it. Traces: k=2 the first attempt wrote [Page(18), Page(19), Sync], the retry wrote pages 25-29 and never 20.
CHOICE: refuse further writes until reopened, not reload in place. New field Mnemo.commit_failed: Option<u64>, set when the new fn commit_and_checkpoint (wal::commit + fsync, checkpoint, shrink) fails. Every later flush returns MnemoError::MaybeStored, so close does too and try_close hands the handle back; the open finishes the commit (WAL replay) or drops it. The first failing flush keeps its original error. Why not reload: an exclusive handle carries per-handle settings a reload must rebuild, and the reload can hit the same failing fsync; restoring the old header is unsafe since the commit may be durable (its pages and generation would be reused). Shared handles already reload (finish_failed_commit/install) and weren't affected.
TESTS: new mnemo/tests/flush_retry.rs (failpoints): every write and every sync of a 1-memory commit over 200 and a 1500-memory commit over 3000, retried on the same handle: writes before the commit write are retried and reopen post; the commit write is refused and reopens pre; later writes are refused and reopen post; sync 0 (barrier) retried, syncs 1 and 2 refused, all post; plus a_reopened_handle_commits_again. All 3 pass; with the poison removed all 3 fail (small: PageAuthFailed at the reopen; big: the "is refused" assertion; reopen test: the MaybeStored assertion). prop_big_batch_commits_crash_pre_or_post now retries on the same handle (refusal only after a failure, retried commit has fsyncs_last_flush 3, Sync(0) must retry, reopen pre or post and post if retried); it passes with the fix (5.37 s) and also without it (5.82 s), likely because batches of 400+ rewrite the whole catalog so there's no delta link - flush_retry.rs is the test that bites.
GATES (local, rustc 1.95): fmt ok; cargo test --features failpoints 798 passed, 0 failed, 1 ignored, 73 suites (load 7.28 at start); clippy all-features, -p mnemo-uniffi and no-features all exit 0. CI: PR 144 all 21 checks pass (run 44476223304).
FSYNCS (device_budget --sizes 1000,10000,100000, load 6.39 -> 5.21): single_write_fsyncs 3 and batch_write_fsyncs 3 at 1K, 10K and 100K, same as v0200-1. Single write median 14.8/14.8/14.4 ms, batch 23.5/22.0/32.5 ms.
COMMITS: e6f2125 on v0200-1r-retry-after-commit, PR 144 into release-0.20.0, squash-merged as 3a295ed on release-0.20.0. No format change. CHANGELOG [Unreleased] ### Fixed; mnemo-python/README.md MaybeStoredError row; MaybeStored doc in error.rs.
FOR THE PLANNER TO TRIAGE (a question, not changed or tested here): after v0200-1's restore before the commit point, the retry allocates the same page numbers under the same generation (the session's lease isn't renewed). Nonces have a per-session random prefix (SEC-1), so this isn't nonce reuse; whether it matters for the stale-page cases ensure_gen_lease exists for is worth a look.
FACTS: fact.post-commit-retry-bug (now says fixed), fact.flush-commit-failed (the design).
```

## 01M7SBKVMXTG22YD3DV48HZ7KC

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-2`
- scope: private
- created: 2026-11-19T07:33:53Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-2"}`

```text
report.v0200-2 (worker-a, 2026-11-18): READY for the Planner's review. The design note and baseline for step 6 (incremental catch-up) plus step 8's gating question. PR 145 into release-0.20.0 (docs only: new docs/design/catch-up.md, one pointer line in docs/design/12.4-shared-access.md), commit da96a2a, no CI run (ci.yml paths-ignore docs/**, as PR 135; mergeable); left open for your review, as PR 135 was.
RECALL: partial. Found spec.v0.20.0 (part B, the step 6 and step 8 wording) and the 12.4 note's step 5 numbers (5.2 ms at 10K, 34.1 ms at 100K); nothing stored on how catch-up should find deltas, the page cache keying or gating, which came from the code.
BASELINE (v0.19.1 via git archive v0.19.1, fresh corpora built by it, M1 Pro, hardware AES via the root .cargo/config.toml, failpoints feature for SharedMnemo::__reloads which confirmed one reload per commit; load 2.8-3.6; two runs, each size in its own process). p50/p99 ms after another handle's one-memory commit:
- catch-up (empty read window): 10K 3.78/4.39 and 3.56/4.23; 100K 30.74/32.32 and 30.80/33.67;
- first recall after it: 10K 4.92/5.88 and 4.82/5.36; 100K 24.05/32.59 and 24.67/35.67;
- recall as the first call after the commit: 10K 11.25/14.58 and 11.67/16.00; 100K 64.55/73.90 and 64.61/73.73;
- warm recall, no commit: 10K 2.43/2.62 and 2.41/2.49; 100K 2.69/4.14 and 2.75/4.04.
- footprint: see fact.shared-reload-footprint (about 4.7 MB per reload at 100K on macOS, freed large blocks, 0 leaks).
A first run with software AES (the extract lacked the root .cargo/config.toml) was stopped and discarded; a second combined run built the corpora in-process (RSS polluted), so only the two clean runs are in the note.
DESIGN (catch-up.md): (1) find new deltas: same file identity and the seal verifies under the held key; same catalog base pointer; walk H_n.catalog_delta back until the first pointer with gen <= t, which must equal H_t.catalog_delta exactly (writers lease after catching up, so every newer run has gen > t); the index chain the same, but a mismatch only unloads the index. (2) apply: rows as apply_delta_chain does, collecting changed rows C; update IdIndex, RecPages and the vector column per row in C (clear have, move page_rows, grow for appends); drop key_index/superseded/layers/agent_set as apply_snapshot does; reread manifest/free list/key slots/cache dir/BM25 manifest only when their pointer changed; apply index ops when loaded; extend delta_chain/delta_rows and index_chain. (3) page cache: key it by (page, generation); every lookup already has the generation (read_page, read_page_uncached, batch reads; dirty_gens exists); chosen over dropping pages named by changed rows, which can't cover control-plane runs or free-list reuse. (4) fallback = today's full reload: identity/seal change, pending WAL or torn page 0, base change (64-run chain, >1/4 rows, restore; tombstone drop rides a base write), any delta error, and no trusted state after a failed/refused write. One-row commits fall back about once per 64. A base-diff middle path is noted, not planned. (5) tests: equivalence vs a fresh from_locked across remember/batch/update/forget/cache put/rekey, base rewrite, tombstone compaction, compact_file, a page reused between the two points, index delta/rebuild/64-run chain, refused write; a __catch_ups() dev-hooks counter; proptest with a second shared handle; crash injection; a cache.rs unit test. (6) targets (p50): catch-up <= 1 ms at both sizes; first recall after it <= 1.5x warm; recall as first call after a commit <= 5 ms; footprint after 100 catch-ups under +10 MB at 100K; the fallback no slower than today; fsyncs 3, the per-call check and the write window kept. The build adds catchup_bench as shared_bench cell 10.
STEP 8: recommend no gate and no format change. v9 is the default since v0.11.0, so v0.11.0-v0.13.2 are the builds with the pre-0.14 compaction gap; the only gate they obey is the format version, and a v10 would also lock out v0.14-v0.19 (including mobile 0.19.0). The gap's lost writes are never acknowledged (ReplacedDuringWrite or a refused rename), which CI's old compactor (v0.13.2) job asserts on every PR (the_0_13_2_compactor_never_loses_an_acknowledged_write). Installers already write --shared, so the default flip reaches hand-written configs only. If a gate is wanted anyway, it's a format change for the user.
FOR THE PLANNER: fact.shared-reload-footprint (the macOS footprint growth on every full reload in v0.19.1) is new and wasn't asked for; triage whether it needs its own task beyond what step 6 fixes.
FACTS: fact.shared-reload-footprint.
```

## 01M7SVCFMHAYF3YYKGQCJXB65K

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-3`
- scope: private
- created: 2026-11-19T12:09:29Z
- importance: 0.8
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-3","area":"report","topic":"v0200-3"}`

```text
report.v0200-3 (worker-a, 2026-11-19): READY. Step 6, incremental catch-up, built per docs/design/catch-up.md and decision.catch-up-design. PR #146 squash-merged into release-0.20.0 as ed4e5b1, with CI green (21 of 21 checks passed).

RECALL: found. report.v0200-2, decision.catch-up-design (additions a to d), fact.shared-reload-footprint.

BUILD: store/catchup.rs (catch_up_to, plan and apply, reconcile_ann_rows, adopt_column) and store/shared.rs (try incremental, else full reload; a half-applied error clears the stamp; a trusted reload adopts the old vector column). The page cache is keyed by page and generation (cache.rs, pager.rs). The vector column has an eighth of headroom (records.rs). Dev hooks; shared_bench cells 10, 11 and 12. AGENTS.md's task table names shared.rs and catchup.rs. CHANGELOG [Unreleased] updated; docs/shared-mode.md cost line updated. New section 8a 'As built' in the design note.

TESTS: tests/catch_up.rs, 9 cases: section 7 cases 1 to 8, additions (a) and (b), and a crash at each of a commit's 11 writes. The proptest gets a second shared handle. Full suite with failpoints: 808 passed, 0 failed, 1 ignored, 74 suites. fmt and the three clippies pass. Mutations that each failed a test: the re-added rule dropped, fp16 entries treated as always equal, adopt_column keeping all rows, the cache ignoring the generation, have not cleared, BM25 stats skipped.

NUMBERS (M1 Pro, corpora built fresh by this build, load 3.0 to 4.3, p50 ms, 10K A/B then 100K A/B):
- catch-up: 0.39/0.44, 0.83/0.82 (v0.19.1: 3.6, 30.8). Target 1 ms MET.
- first recall after it: 5.80/6.74, 5.80/5.93 (warm 2.43/2.41, 2.82/2.77). Target 1.5x warm MISSED.
- direct recall: 10.6/11.0, 8.16/8.19 (v0.19.1: 11.7, 64.6). Target 5 ms MISSED.
- Control, a warm recall after a 20 ms sleep with no commit: 11.8/16.0, 17.5/18.3. Warm recalls after all the catch-ups: 2.41/2.45, 2.21/2.22. So the misses follow this machine's slowdown after a pause (fact.recall-after-pause), not proven. The second recall after a catch-up (5.7 ms at 10K, against v0.19.1's 2.7) is unexplained.
- Fallback at 100K, the long run, 7 per run (load 2.7 to 7.4): reload 37.3/38.3, against v0.19.1 on the same 700-commit shape at 27.6 (load 5.0 to 11.9). 'No slower than today' MISSED by about 10 ms. The time is in from_locked; user CPU is about the same, system time about 10 vs 4.8 ms (ps probes), which fits fresh page faults. The recall after a fallback is 73.6/73.0, the same as v0.19.1's 73.6.
- Incremental catch-up over the long run rises to 1.18/1.22 ms p50. Not investigated.
- Kept: fsyncs 3 (device_budget single and batch); per-call check 0.039/0.037 ms (was 0.048); shared write window 24.9 ms at 10K, 32.0 ms at 100K.

(c) FOOTPRINT (physical MB, 100K):
- cell 10, 200 commits: 131.3 -> 240.9 (v0.19.1: +472 over 100 reloads). Target +10 MB after 100 MISSED.
- 500-commit long run, three runs: 167.0 -> 262.4, 168.2 -> 244.8, 165.1 -> 225.5. Target +20 MB MISSED.
- v0.19.1 on the same shape reached 1,433.6 then 1,126.4.
- So the fallback no longer grows about 4.7 MB per reload in a straight line: steps of about 2 to 34 MB at fallbacks, flat between them, two drops, about 5 to 9 MB net per fallback. A plateau past 500 commits wasn't measured.
- What remains: the first catch-ups double the capacity of the catalog, the ANN lists and the id map (live data +22.6 MB), and each fallback builds a new state beside the old one. Section 6's middle path, beyond adopt_column, isn't built. The Planner decides whether the footprint, direct-recall and fallback misses get a task.

(d) LAZY PARTS, cell 11 at 100K, first-recall p50, A/B: key_index 3.35/3.31, superseded 2.54/2.64, layers 2.56/2.47, agent_set 2.39/2.28, bm25_stats 2.27/2.35, bm25_dicts 2.18/2.13, ann 9.54/9.41, column 4.51/4.47; none (first, includes warm-up) 3.45/3.51. The four the catch-up drops are cheap, so none was made incremental. The index and the column are kept.

DEVIATIONS:
- adopt_column is a partial middle path (section 6).
- The vector column headroom.
- Earlier runs in this task used a stale cached corpus with a layout-1 index (fact.bench-corpus-cache). Every number above is from fresh corpora.
- A known edge: a copy restored in place (same inode, same key) can reuse page and generation numbers. It limits the catch-up and adopt_column, and is documented in 8a.
- The 'update every mnemo' release-notes line belongs to step 8 (v0200-4), not this PR.

FACTS SAVED: fact.recall-after-pause, fact.catch-up-ann-reconcile, fact.catch-up-footprint, fact.bench-corpus-cache.
```

## 01M7T30KCBRV3G7VNYN7P8M209

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-3r`
- scope: private
- created: 2026-11-19T14:22:48Z
- importance: 0.8
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-3r","area":"report","topic":"v0200-3r"}`

```text
report.v0200-3r (worker-a, 2026-11-19): READY. A catch-up never trusts held state across a copy restored in place. PR #147 was squash-merged into release-0.20.0 as b2a2dc0, with CI green on all 21 checks. No format change. RECALL: found decision.catch-up-results, decision.catch-up-design and status.v0200-review-scope. BUILD: the handle keeps anchors, the on-disk nonce of the first page of each run in the header it holds. They are taken at load or install, at a catch-up and at its own commit. Before a catch-up or a trusted reload, anchors_hold needs new wal_seq >= held, and reads the anchors uncached for the runs the new header keeps: the catalog base and head always; a loaded index's base and head when the index base is unchanged; other runs only when the pointer is unchanged. A mismatch means a full reload (fresh pager, so fresh page cache) and no adopt_column. On success, clean cached pages newer than the newest checked run are dropped (PageCache::drop_newer_than), and a BM25 dict is kept only if its segment gen <= that run (catch_up_to's trusted_gen). DEVIATIONS from the suggested shape: (1) it checks all kept runs, not just the catalog head, because the catalog-only mutation misses a restore whose next commit changes only the cache run, and cache_get served the old value; (2) superseded runs aren't checked, because checking them made a_reused_page_is_never_served_stale take FULL under max_snapshots(1), where a superseded free run is reused legitimately; (3) the clean-page drop, for a run that moved onto pages the old timeline wrote at the same gen. TESTS: tests/catch_up.rs has 13 cases (4 new). Each restores a copy in place with the inode asserted equal, and asserts the pointers coincide (held head in the restored chain; >=20 of 60 vector entries; the cache pointer; the value page's start_page and gen). The new tests are: never caught up incrementally (FULL, then INCREMENTAL after another commit and after its own write); keeps no vector column (new base, FULL); only a cache put (FULL, cache_get returns the new value); never serves a cached page of the old timeline (INCREMENTAL, new value). MUTATIONS: no check, all 4 new tests fail; catalog runs only, the cache-put test fails (stale value); no cache drop, the moved-run test fails (stale value). COST (shared_bench long run, M1 Pro): catch-up 0.495/0.499 ms at 10K and 0.803/0.814 ms at 100K (load 6.2-11.5), path counts unchanged (394/7, 197/4). A/B at load 2.9-5.0: with 0.450/0.403 against without 0.455/0.413 ms (10K); with 0.808/0.824 against without 0.802/0.846 ms (100K), within noise and under the 1 ms target. GATES: cargo test --features failpoints 813 passed, 0 failed, 1 ignored across 74 suites; fmt and the three clippies clean. DOCS: catch-up.md 8a ('A copy restored in place'), CHANGELOG [Unreleased]. NOTE: task next took v0200-4 and v0200-d1..d4 by mistake before v0200-3r; all were requeued untouched, with no work done.
```

## 01M7T8KSW1VMHPY32B5082TAYB

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-4`
- scope: private
- created: 2026-11-19T16:00:40Z
- importance: 0.8
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-4","area":"report","topic":"v0200-4"}`

```text
report.v0200-4 (worker-a, 2026-11-19): READY. mnemo serve --mcp is shared by default (step 8 of spec.v0.20.0). PR #148 was squash-merged into release-0.20.0 as 54afc04, with CI green on all 21 checks. No gate and no format change (catch-up.md section 9). RECALL: found spec.v0.20.0 (step 8), decision.catch-up-design (no gate, no format change; the release notes repeat v0.14's update-every-mnemo line), decision.catch-up-results and the catch-up.md section 9 text. BUILD: a bare serve --mcp is shared. --shared is still accepted, so existing configs work. --exclusive keeps the lifetime lock and conflicts with --shared. --allow-network-fs no longer requires --shared and conflicts with --exclusive. When shared mode refuses a network or synced folder (Unsupported), the CLI error adds that serve is shared by default since v0.20 and names --allow-network-fs and --exclusive. The library's serve_stdio_* functions are unchanged (None is still a plain handle). DOCS: guide.md section 2 (llms-full.txt and the plugin SKILL.md regenerated; llms.txt, the card, unchanged), the serve --help text, mnemo/README.md (serve section and lock note), docs/shared-mode.md (surface table, plain callers), and CHANGELOG [Unreleased] with the planner's release-notes line: update every mnemo on the machine before relying on shared mode. TESTS: new cli_shared::serve_is_shared_by_default_and_exclusive_keeps_the_lifetime_lock. A CLI write with a 500 ms wait gets in beside a bare server and beside a --shared one, and is locked beside --exclusive; list confirms; --shared --exclusive and --exclusive --allow-network-fs are refused. Mutation: with the old default restored, the test fails at the bare server's write (cli_shared.rs:62). Tests that meant the plain server now name --exclusive: mcp_context's session helper, and hook_cli::a_brain_locked_past_the_wait_is_busy_not_blocking. cli_smoke::mcp_serve_initialize_list_and_call_tools now removes MNEMO_AGENT and MNEMO_ROLE: with an agent name, a shared server writes a presence record (since v0.17.2), which stats counts as a memory, and the test failed from this session's environment. GATES: cargo test --features failpoints 814 passed, 0 failed, 1 ignored across 74 suites; fmt clean; CI's clippy (failpoints,benches,keychain,embed-candle) and -p mnemo-uniffi clean. NOTE for the Planner: a plain cargo clippy --all-targets with no features fails to compile tests/rekey_cut.rs, which uses mnemo::__failpoints without a cfg gate. It's untouched here and not run by CI; logged, not fixed. Possible follow-up for the Planner to decide: stats.memories counts coordination records such as presence, which recall leaves out.
```

## 01M7T8TXG1R7CBZAGVW1W1HQ4X

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-d1`
- scope: private
- created: 2026-11-19T16:04:33Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-d1","area":"report","topic":"v0200-d1"}`

```text
report.v0200-d1 (worker-a, 2026-11-19): READY, with one NEEDS-USER step. The GitHub social preview image. PR #149 was squash-merged into release-0.20.0 as da0a234. CI didn't run, by design: docs/** is in ci.yml's paths-ignore (RB-001.7), as with docs-only PR #145. RECALL: found decision.discoverability (the user's yes to the social preview) and status.discoverability; the site palette was read from index.html. BUILD: docs/img/social-preview.png (1280x640 RGB, 47,577 bytes) and its source, docs/img/social-preview.py, a Pillow script run with test/.venv/bin/python from the repository root. It uses the macOS Helvetica Neue and Menlo fonts and the site's dark palette. DEVIATION: the source is a script, not an SVG, as the task allowed. GitHub's upload takes PNG or JPG, and this Mac has no SVG renderer to check an SVG against the PNG. CONTENT: the name MNemo; 'Shared memory for your coding agents, in one encrypted file.'; '$ mnemo onboard'; an encrypted project.mnemo card holding the README's three layers (Memory you can search; Agents sharing work; The engine underneath). Dark background (#0E1113). None of the words orchestration, orchestrator or swarm. CHECKS: measured on the PNG, the content's bounding box below the top rule is 72/118/71/87 px from left/top/right/bottom, inside the 40 px safe area; only a decorative 6 px accent rule along the top reaches the edge. A 600x300 downscale was checked by eye and every line is readable. NEEDS-USER: upload docs/img/social-preview.png in the repo's Settings > General > Social preview (the Worker doesn't change repo settings). NOTE: task next served v0200-5 (the oldest queued) first; it was held only to reach v0200-d1 and released untouched, with notes.
```

## 01M7TAJ84E1PFBC9PV17X7B6GZ

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-d2`
- scope: private
- created: 2026-11-19T16:34:47Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-d2","area":"report","topic":"v0200-d2"}`

```text
report.v0200-d2 (worker-a, 2026-11-19): READY. crates.io and PyPI descriptions and keywords. PR #150 was squash-merged into release-0.20.0 as 402e183, with CI green on all 27 checks (mobile CI included, since it touched mnemo/bindings). Live with the v0.20.0 publish. RECALL: found decision.discoverability; partial on packaging (the naming history, the release credentials); no earlier metadata decision was stored. CRATES.IO (mnemo/Cargo.toml): description 'Shared memory for your coding agents, in one encrypted file: an embedded Rust engine with recall, an MCP server and a CLI'. Keywords (5, the max): mcp, agent-memory, encryption, vector-search, embedded-database; each <=20 chars of [A-Za-z0-9_+-] starting with a letter, read back with cargo metadata. Categories (5, the max): database-implementations, database, cryptography, development-tools, command-line-utilities; each slug checked against the crates.io categories API. PYPI (mnemo-python/pyproject.toml): description 'Shared memory for your coding agents, in one encrypted file: Python bindings for the embedded MNemo engine, with recall'; the same 5 keywords. 14 classifiers, all in PyPI's published list: Development Status 4 Beta; Intended Audience Developers; Apache license; OS MacOS, Microsoft Windows and POSIX Linux (release.yml's wheel targets); Rust, Python 3, Python 3 Only; Topic Database, Database Engines/Servers, AI, Cryptography, Software Development Libraries. project.urls add Documentation (mnemo-python/README.md on main), Changelog and Issues, each returning 200. maturin sdist's PKG-INFO carries every field. OTHERS: mnemo-python/Cargo.toml and mnemo-uniffi (both unpublished) match. DEVIATION: the Kotlin POM description is unchanged. It describes the Android library for app developers, and leading with coding agents would misdescribe it; the Planner can ask for a change. Swift's Package.swift has no description field. CHANGELOG [Unreleased] line added. None of the words orchestration, orchestrator or swarm. NOTE: task next served v0200-5 first; it was held only to reach d2 and released untouched, with a note.
```

## 01M7TCE7Z86TZJRRPTK63HN3VZ

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-d3`
- scope: private
- created: 2026-11-19T17:07:32Z
- importance: 0.8
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-d3","area":"report","topic":"v0200-d3"}`

```text
report.v0200-d3 (worker-a, 2026-11-19): READY, with NEEDS-USER steps after the tag. The official MCP registry listing for io.github.toarchkumar/mnemo. PR #151 was squash-merged into release-0.20.0 as 45d811e, with CI green on all 27 checks. Nothing was published. RECALL: found decision.discoverability and status.discoverability, and fact 01M7EK1P (a cargo install with default features has no keychain and no built-in embedder); nothing about the registry was stored before. READ: the registry's docs (package-types.mdx, official-registry-requirements.md, server.schema.json 2025-12-11, the CLI commands) and the MCPB manifest spec v0.3 with the reference host's src/shared/config.ts. PACKAGE TYPE: mcpb, a bundle on the GitHub Release. pypi can't run the server (the wheel has no CLI). cargo is supported, but the default features have no keychain and no embedder, and it needs a Rust toolchain. A registry package has no platform field, and an MCPB manifest picks per OS but not per CPU, so one bundle carries the five release binaries plus a /bin/sh launcher (integrations/mcpb/mnemo.sh) that picks by uname; win32 runs its .exe directly. OWNERSHIP: mcpb needs 'mcp' in the URL (the .mcpb extension) and a fileSha256; the namespace comes from mcp-publisher login github as toarchkumar. No other marker is needed. RUNS: mnemo serve --mcp --shared ${user_config.brain} (a required file). The passphrase is optional, sensitive, with default '': the reference host leaves an unset field without a default as the literal ${user_config.x}, and an empty MNEMO_PASSPHRASE counts as unset (keys::env_set), so the keychain is used. Tested locally with this branch's debug binary: the launcher runs on Darwin arm64; the bundle's command on a scratch brain lists 23 tools in shared mode; the empty default with no keychain fails with 'no secret for ...'. FILES: root server.json (the template, bumped with the manifests, no fileSha256, so the root file can't be published by mistake); integrations/mcpb/{manifest.json (no version), mnemo.sh, build.sh, which writes the bundle and server.json with its SHA-256 and refuses a version server.json doesn't name}. release.yml: new mcp-bundle job (also in dry runs; validates with npx @anthropic-ai/mcpb@2.1.2 validate and mcp-publisher v1.8.7 validate, the tarball pinned by SHA-256 00b21004...); attach adds both (14 files); the version check reads server.json's version and URL; approve-release waits for it. ci.yml shellchecks both scripts. tests/release_names.rs checks server.json against CARGO_PKG_VERSION, the URL, no hash and the 100-character description, and that the bundle covers build-cli's 5 targets (mutation: version 0.19.0 fails). RELEASING.md has the bump and the after-tag publish; AGENTS.md's task table and the CHANGELOG are updated. DRY RUN on the branch, run 44569170984: every build and check job succeeded. mcp-bundle: bundle 27,079,304 bytes, 'Manifest schema validation passes!', tarball checksum OK, mcp-publisher 'server.json is valid' against the live registry, attach '14 files would be attached'. publish to PyPI and publish to crates.io failed before any step: the pypi and crates-io environments allow only tag:v* and branch:release-*. This is expected for a feature branch, not caused by the change; v0200-5's dry run on release-0.20.0 covers them. NEEDS-USER after the v0.20.0 tag: install mcp-publisher, gh release download v0.20.0 -p server.json -p mnemo-v0.20.0.mcpb, compare the hash, mcp-publisher login github (as toarchkumar), mcp-publisher publish server.json, then check the listing (RELEASING.md). FOR v0200-5: the bump must include the root server.json version and bundle URL (the version check and release_names enforce it). NOT VERIFIED: whether each MCP host keeps the zip's executable bits and how macOS quarantine treats the extracted binaries; the first real install will show. NOTE: task next served v0200-5 first; it was held only to reach d3 and released untouched.
```

## 01M7TGKMGSD8HYFJHRFNANGMQD

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-d3h`
- scope: private
- created: 2026-11-19T18:20:23Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-d3h","area":"report","topic":"v0200-d3h"}`

```text
report.v0200-d3h (worker-a, 2026-11-19): READY. The MCP bundle is checked apart and attested like the binaries. PR #152 was squash-merged into release-0.20.0 as 75eeca2, with CI green on all 27 checks (rust macos-latest on attempt 2, see the flake below). RECALL: found report.v0200-d3, fact.release-dry-run-branch and the earlier shared_valve flake record 01M7Q1R5. (1) VALIDATORS APART: mcp-bundle builds mnemo-vX.Y.Z.mcpb and server.json, uploads the mcp-bundle artifact, and runs nothing else. New job mcp-bundle-check (needs mcp-bundle, contents: read, uploads nothing) downloads the artifact, copies both files to a mktemp dir, checks the bundle's SHA-256 against server.json, unzips the copy, and runs npx --yes @anthropic-ai/mcpb@2.1.2 validate on its manifest.json and mcp-publisher 1.8.7 validate (tarball pinned by SHA-256) on the copied server.json. approve-release and attach need it. attach now checks that exactly one bundle is present and that its SHA-256 matches server.json's mcpb fileSha256 (sha256sum -c), before attaching and in dry runs. (2) ATTESTATION: attest downloads mcp-bundle and attests mcp/mnemo-v*.mcpb and mcp/server.json beside cli/mnemo-v* (subject-path list; tag runs only, after the approval). RELEASING.md's after-tag registry step runs gh attestation verify on both with --signer-workflow toarchkumar/mnemo/.github/workflows/release.yml and --source-ref refs/tags/vX.Y.Z (the install scripts' flags) before mcp-publisher publish. AGENTS.md's task-table line and the CHANGELOG bullet say so. DRY RUNS: branch run 44574285872: mcp-bundle, mcp-bundle-check (mnemo-v0.19.1.mcpb: OK; 'Manifest schema validation passes!'; mcp-publisher.tar.gz: OK; 'server.json is valid') and attach (mnemo-v0.19.1.mcpb: OK; '14 files would be attached') succeeded. Only the two publish jobs were refused, by the branch policy (fact.release-dry-run-branch). Release-branch run 44583368965 on release-0.20.0 at 75eeca2: success, 25 jobs succeeded and 9 skipped (mobile off, tag-only), including MCP bundle and server.json, MCP bundle check, attach, publish to crates.io and publish to PyPI. The attestation itself runs only on a tag, so the first real check is v0.20.0's run plus RELEASING's verify. FLAKE, its second occurrence: shared_valve::two_writers_and_four_readers_all_get_their_turns failed on rust (macos-latest) on PR 152's first attempt (job 135737720437): 'readers stopped getting in', commits [24, 26]; reads per 500 ms [41, 82, 0, 52, 13, 112]; longest read wait 688.6 ms. It passed on the re-run (gh run rerun 44574368107 --failed). This PR changed no Rust code. The first occurrence was PR 139 (01M7Q1R5: [140, 47, 0, 103, 79, 33]); that record says the Planner decides if it recurs, and it has. NOTE: task next served v0200-5 and v0200-d4 before d3h; both were held only to reach it and released untouched.
```

## 01M7TKEHEACXJADJNYE7WTCVY8

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-d4`
- scope: private
- created: 2026-11-19T19:10:02Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-d4","area":"report","topic":"v0200-d4"}`

```text
report.v0200-d4 (worker-a, 2026-11-19): READY. The terminal demo GIF. PR #153 was squash-merged into release-0.20.0 as 52832a5, with CI green on all 28 checks (CI, Mobile CI and demo ran, since the PR adds a workflow). RECALL: found decision.discoverability (option 1, a scripted GIF re-recorded each release; the Planner places it in README.md and index.html). Nothing earlier on demos was stored. TOOL: VHS v0.12.1 through charmbracelet/vhs-action pinned to v2.1.1's commit f6d7db07. VHS, ttyd and ffmpeg aren't on this Mac, so it records in CI rather than installing them. FILES: docs/demo/demo.tape; docs/demo/setup.sh (sourced hidden: unsets every MNEMO_* var, MNEMO_PASSPHRASE=demo-passphrase-not-a-secret, mktemp -d, mnemo embedder install off camera, mnemo init team.mnemo --no-manifest; never the project brain, and no keychain since the build has no keychain feature); .github/workflows/demo.yml (on pull_request touching docs/demo/** or the workflow, and workflow_dispatch; builds this commit's mnemo with --release --locked --features embed-candle; runs the tape; fails at 3 MB or more; uploads the demo-gif artifact; registered and active, id 424183778); docs/img/demo.gif. CONTENT (32.4 s): '# in Claude Code' remember --key decision.release 'Ship from the release branch.' (added); '# in Cursor, later' recall 'how do we ship a release?' --top-k 1 (score 0.921); Cursor refines it under the same key (superseded) and history --key shows live and superseded; about | head -n 1 ('2 memories ... encrypted ... 3 snapshots'); grep -c 'release branch' team.mnemo shows 0. Every output is real, from the run. Trial-run locally first with a release build (embed-candle, no keychain) in a scratch dir and a scratch model cache. GIF: from demo run 44589623935, 389,061 bytes, 1400x540, 809 frames. The first frame is an empty prompt (none of the setup shows). Checked downscaled to 800 px: readable, no line wraps. An earlier take (run 44587927506, 620 px tall) had an empty bottom fifth. RELEASING.md: a new 'Before the tag' step 4, re-record the demo (gh workflow run demo.yml --ref <release branch>; gh run download <id> -n demo-gif -D docs/img; look; commit by path); later steps renumbered and the step-7 cross-reference fixed. CHANGELOG Added line. None of the words orchestration, orchestrator or swarm. FOR v0200-5: re-record at READY per the new step, so the GIF shows the 0.20.0 build (today's shows this branch's 0.19.1-versioned build; no version string appears in it). NOTE: the demo workflow also re-runs on every push to a PR that touches docs/demo (GitHub's pull_request paths filter matches the whole PR diff); harmless, the artifact is only committed by hand. task next served v0200-5 first; it was held only to reach d4 and released untouched.
```

## 01M7TKRTNA627JFKPTP05GCC0Z

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-t1`
- scope: private
- created: 2026-11-19T19:15:39Z
- importance: 0.8
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1795116077,"_mnemo.key":"report.v0200-t1","_mnemo.superseded_by":"01M7TM34M6Z3F0A01EDRY35TNY","area":"report","topic":"v0200-t1"}`

```text
report.v0200-t1 (worker-a, 2026-11-19): BLOCKED, because the spec's two failure rules conflict with the lock layer's design. The measurements, a proposal, and no code change. RECALL: found fact.flake-shared-valve, record 01M7Q1R5 (PR 139) and the LOCK-2 decisions (01M7DZKM, 01M7DQC7: T = 500 ms). CODE (src/shared/lock.rs acquire/wait): a waiting writer holds the intent lock exclusively, so a reader that arrives during another writer's window finds intent Busy and backs off (doubling, up to 50 ms). A reader also backs off while a writer holds the valve, until it has itself waited VALVE_AFTER (500 ms); then it hurries and queues (LOCK-2 round 3). The test's write windows are sleep(5 ms) with no disk, so 'a slow write window on a CI disk' isn't the cause. PROBE (a temporary ignored test copying test 5's mix: 4 readers holding 1 ms, 2 writers holding 5 ms, 3 s; timestamps for every read wait and every write window; this Mac, M1 Pro, 8 cores, load average 5 to 11 during the runs; not committed, kept at scratchpad zz_valve_probe.rs). Valve on, 4 runs: longest window held 8.2-9.1 ms; longest read wait 160-222 ms, during which write windows held 21-54 ms in all; most write windows in a row with no read granted between them 4-8; 0 or 1 valve takes per run. Valve off (MNEMO_DEV_NO_VALVE), 4 runs: longest read wait 198-303 ms (windows held 49-97 ms during it); runs of 7-14 windows. With 8 and 16 CPU-hog threads (valve on): longest read wait 169 and 233 ms; runs of 3 and 7. So readers mostly wait while no write window is held: writers hand the lock to each other through the intent lock, and the reader's backoff misses the gaps. That's the documented design (readers defer up to 500 ms of their own wait, then hurry), not a slow window. CI's 688.6 ms read wait fits that bound (500 ms + windows + backoff). CONFLICT: (a) 'fail when readers get no turn across two writer windows in a row' fails on every run measured here (runs of 3-14); (b) 'fail when a read waits more than the longest window held while it waited + 250 ms' allows about 260 ms, but the design allows over 500 ms, and it was exceeded locally (303 ms) and on CI (688.6 ms). Both would make the test fail more often, not less. PROPOSAL (test-only, the Planner decides): measure each read's idle wait, its wait minus the time write windows held the lock during it, and fail when an idle wait exceeds VALVE_AFTER (500 ms) + 500 ms of slack (1 s), the design's own bound plus scheduler slack. Locally the largest idle wait was about 206 ms; CI's 688.6 ms total stays under 1 s even with no windows subtracted. Keep 'every writer commits' and a total read-wait cap. Drop the fixed 500 ms buckets. Show that it bites with a local mutation in lock.rs (not committed): readers always defer to the valve (drop the call.waited() < valve_after() condition), so two writers handing the valve on keep readers out past 1 s. Neither the task's 'readers-first off' nor 'a writer that loops without yielding' can be shown by this rule: there's no readers-first switch, and a writer that never releases is held time, which the rule subtracts by design. Alternative: just widen the bucket check (one empty 500 ms bucket allowed, never two in a row). No change to the locking: the numbers show waits within the design's bound, not starvation. Nothing committed; the tree is clean (the probe was moved out). NOTE: task next served v0200-5 first; it was held only to reach t1 and released untouched.
```

## 01M7TM34M6Z3F0A01EDRY35TNY

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-t1`
- scope: private
- created: 2026-11-19T19:21:17Z
- importance: 0.8
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1795118230,"_mnemo.key":"report.v0200-t1","_mnemo.superseded_by":"01M7TP4TK2XHNW7M3873C5T08A","_mnemo.supersedes":"01M7TKRTNA627JFKPTP05GCC0Z","area":"report","topic":"v0200-t1"}`

```text
report.v0200-t1 (worker-a, 2026-11-19, round 1 of 3): BLOCKED again. The idle-wait check is built and passes, but the decision's proof doesn't hold: the named mutation causes no starvation in this mix, and a real starvation mutation passes the idle-wait check. Measured; a proposal follows. BUILT (uncommitted, branch v0200-t1-valve-idle-wait): test 5 records every read's (asked, got) and every write window's (got, released) and computes each read's idle wait (its wait minus the windows' held time inside it). It fails when an idle wait reaches __shared::VALVE_AFTER + 500 ms (1 s), when a writer never commits, when no read gets in, or when a read waits 5 s or more. The 500 ms buckets are gone. VALVE_AFTER is exposed through the hidden __shared hooks (pub(crate) in lock.rs and a testing re-export; no behavior change). The shared_valve suite passes (8); a plain cargo build has 0 warnings. HEALTHY, 5 runs on this Mac: commits [38,30] [22,36] [26,32] [34,34] [27,32] (58-68 windows); 1594-2188 reads; longest read wait 185-256 ms; longest idle wait 144-200 ms; longest window 8.4-15.8 ms. MUTATION A, the decision's (readers always defer to the valve: drop call.waited() < valve_after() in acquire's reader check), 3 runs: 1682-2177 reads; longest idle wait 115-133 ms; it passes, because writers take the valve 0-1 times per run in this mix, so no starvation occurs. A with MNEMO_DEV_VALVE_MS=1 (a busy valve): 151-161 commits per writer; 377-396 reads; longest idle wait 41-48 ms; it passes, because the writer lets the valve go once it has the main lock and readers keep the intent lock while waiting. MUTATION B, the real starvation (the pre-step-5 behavior lock.rs names: a reader drops the intent lock when the main lock is busy, 'if intent.is_none() || kind == LockKind::Shared { break; }'), 3 runs: commits [95,100] [87,121] [107,106]; 29, 34 and 44 reads in 3 s; longest read wait 0.82-1.00 s; longest idle wait 469-559 ms. It passes the idle-wait check, because LOCK-2's reader hurry caps any single read's idle wait at about VALVE_AFTER, so starvation shows in throughput, not in one wait. The old bucket check would have caught B. lock.rs is restored. PROPOSAL: keep the idle-wait check and add a throughput check that isn't a fixed bucket: reads >= write windows over the run. Healthy: 1594-2188 reads against 58-68 windows (about 30x). Mutation B: 29-44 reads against 195-213 windows (0.15-0.21x, fails). CI's failing run on PR 152: about 300 reads against 50 windows (6x). The proof would be mutation B, run locally and recorded, not committed. Mutation A can't be the proof: it doesn't starve readers in this mix. The decision is the Planner's; nothing is committed yet. NOTE: the idle-wait numbers here also confirm round 0's reading: readers wait while writers hand the lock on, within the design's bound.
```

## 01M7TP4TK2XHNW7M3873C5T08A

- type: episodic
- agent: `cursor-agent`
- key: `report.v0200-t1`
- scope: private
- created: 2026-11-19T19:57:10Z
- importance: 0.8
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"report.v0200-t1","_mnemo.supersedes":"01M7TM34M6Z3F0A01EDRY35TNY","area":"report","topic":"v0200-t1"}`

```text
report.v0200-t1 (worker-a, 2026-11-19, after round 2): READY. shared_valve's two-writer test measures idle waits and throughput. PR #154 was squash-merged into release-0.20.0 as c0c66bd, with CI green on all 21 checks (rust windows-latest on attempt 2, see the flake below). No locking change. RECALL: found fact.flake-shared-valve, 01M7Q1R5, the LOCK-2 decisions, and 01M7DQC8 (a sibling cli_shared timing failure in v0.15). BUILT, per the planner's round-2 decision: test 5 records every read's (asked, got) and every write window's (got, released) and fails when (1) a read's idle wait (its wait less the windows' held time inside it) reaches __shared::VALVE_AFTER + 500 ms; (2) reads < write windows over the 3 s run; (3) a writer never commits, or a read waits 5 s or more. The fixed 500 ms buckets are gone. VALVE_AFTER is exposed through the hidden __shared hooks (pub(crate) const in lock.rs plus a testing re-export; behavior unchanged). HEALTHY, 3 runs after the throughput check: commits [24,48] [37,28] [35,29]; 1748, 1780 and 2425 reads; longest idle wait 183, 198 and 128 ms; longest window 8.0, 7.5 and 18.8 ms; pass. PROOF, mutation B, local and not committed ('if intent.is_none() || kind == LockKind::Shared { break; }' in acquire's inner loop, the pre-step-5 reader behavior): FAILED 3 of 3, 'readers got in 30 times while writers committed 184 times', 29 vs 236, 29 vs 215; idle waits 556, 588 and 486 ms (under the bound, as round 1 found). lock.rs was restored (git diff shows only the pub(crate)). Mutation A was dropped per the decision. GATES: fmt clean; CI's clippy clean; shared_valve 8, shared_lock 17, shared_handle 19, cross_process 14 pass. FLAKE (new, a different test): cli_shared::a_shared_mcp_server_embeds_before_it_takes_the_lock failed on rust (windows-latest) on PR 154's first attempt (job 135826405514): 'the import waited 1.9746906s' against its 1.2 s cap. The import succeeded within its --wait 500ms (ok(&out) passed), so the time was the CLI process, not the lock. It passed on the re-run (gh run rerun 44599412284 --failed). This PR doesn't touch that path. A sibling, the_cli_embeds_before_it_takes_the_lock, had Ubuntu timing failures in v0.15 (01M7DQC8). Logged as fact.flake-cli-shared-embed; the Planner decides if it recurs.
```

## 01M7PEYPQ3MXR42MYWFEHJCE0J

- type: procedural
- agent: `cursor-agent`
- key: `spec.inspect-date-filter`
- scope: private
- created: 2026-11-18T04:34:31Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"spec.inspect-date-filter"}`

```text
spec.inspect-date-filter (v0.19.0, the user's request 2026-11-17; PLANNER-A). `mnemo inspect` gets a date range filter on every tab with dated rows (memories, Findings, Tasks): a From and a To date, inclusive, in the viewer's local time; either may be empty (open-ended). It filters by the row's created time; Tasks may offer last change instead, labelled. It combines with the existing filters, and the counts shown reflect it. The inspector's rules are unchanged: GET only, brain text into text nodes only, no handle held between requests, nothing written. If a tab's rows are paged by the server, add since/until query parameters to its GET route instead of filtering one page in the browser; reject a malformed date with a 400. Tests: routes in tests/inspect_cli.rs, and a page check like the existing ones. Guide or README line if the inspector is described there.
```

## 01M7PEYPMDMCFJ0407GATXH96F

- type: procedural
- agent: `cursor-agent`
- key: `spec.perf-2`
- scope: private
- created: 2026-11-18T04:34:31Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"spec.perf-2"}`

```text
spec.perf-2 (v0.19.0, HARNESS-DOGFOOD-1 Run 2; PLANNER-A 2026-11-17). PERF-2 (Phase 11.1): a faster first search after opening, with NO file-format bump. The v9 ANN index run carries a layout version byte, so a new index layout can ship: a reader that doesn't know a layout treats the file as having no index (exact scan or rebuild_index), never as an error.

Targets (M1 Pro, release build, 384-dim, about 300 B of text per memory: the v0.11 perf setup):
- first recall on a fresh handle at 100K (ANN): <= 15 ms (about 120 ms since v0.11);
- first recall at 10K (no index, exact fp16 scan): <= 5 ms (about 9 ms on the M1 Pro, 7.1 ms on an iPhone);
- warm recall at 10K: <= 2 ms (2.19 ms), for example a SIMD fp16 scan;
- the 4th fsync on some batch commits (9c's ordering): investigate and report; change it only if it's provably safe.

Known starting point (first_recall_10k_profile, v0.11): the vector-column fill is 66% of first recall and AES-GCM decrypt 48%. Levers: partition-major index loading (read only the partitions a query probes), a lazy or partial vector-column fill, one pread per run instead of an lseek and a read per page.

Must not regress: ranking_v2 (hit@1 0.9231, MRR 0.9487), recall@10 at 100K (98% at the defaults), exact results below 20K (recall 1.0), durable write latency, open time, RSS at 100K (112 MB), shared mode, read-only opens, and v8 read-only files.

Step 1 (task v0190-1): measure the baseline on v0.18.1 with the existing benches and examples, and write a short design note, docs/design/perf-2.md: the layout, the lazy fill, how it works with shared mode and the lock, which numbers it should reach, and what an older reader does with the new layout. Then ready, for the Planner's review before any engine change.
Step 2 (task v0190-2): the build, with tests: an old-layout index still read; a new-layout file opened by v0.18.1 falls back without error; failpoint and proptest coverage if any write path changes; test/scripts/perf_v0.19.0.json with the measured numbers. README and CHANGELOG numbers come only from measured runs. REVIEWER-B's READY pass applies (a minor release that touches the read path).
```

## 01M7TFQQJ6D2XY60MZXZ3AASFZ

- type: episodic
- agent: `cursor-agent`
- key: `spec.rev-011`
- scope: private
- created: 2026-11-19T18:05:09Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"spec.rev-011"}`

```text
spec.rev-011 (planner-a, 2026-11-19; the user called it: "we should do deep research of if this is even an efficient path"): REVIEWER-A's batch REV-011. Read docs/design/hosted-brains.md (the idea note; untracked until the Planner hands it off). PART A, deep research, the main part: is a .mnemo file per tenant, in object storage, with KMS-wrapped keys, an efficient backend for an agent harness that gives standardized consultation to outside clients? Answer with sources and numbers: (1) The alternatives for isolated, encrypted, per-client agent memory, compared on cost per tenant, latency (cold and warm), operations burden, key management (BYOK, crypto-shredding), compliance (SOC 2, GDPR, data residency) and lock-in: Postgres with pgvector and row-level security (e.g. Supabase); a database per tenant (Turso/libSQL, Neon); SQLite in Cloudflare Durable Objects; object-storage-native search (turbopuffer, LanceDB); managed agent-memory services (Mem0, Zep, Letta); vector databases with namespaces (Pinecone). Where does a single encrypted file per tenant win, lose or tie? (2) The market, in both readings of the note: (A) the user's own consultancy on MNemo, (B) MNemo selling hosted brains. Who buys, what they pay for today, what they need from memory (isolation, audit, handing data back, erasure), and whether "one encrypted file per client" is a selling point or invisible to buyers. Include what agent harness platforms (Claude Managed Agents, Vercel, LangGraph Platform, Letta and similar) use for memory. (3) Unit economics for both readings: storage, compute per active session, KMS calls, egress, cold start (fetch plus open), at 100, 1,000 and 10,000 tenants. (4) Technical risk: single writer per file over object storage with leases; how LiteFS, Turso, Durable Objects and similar handle placement, failover and durability; what breaks first for MNemo (note section 4). (5) A verdict: go, no-go, or what to prove first, with the smallest experiment that would prove or kill it, and which reading has better odds. PART B, the scheduled pre-phase check (AGENTS.md Reviewer rule 4, due before the merge phase): given Part A, is decision.roadmap-order after v0.20.0 still right (13.7 merge; change sets on disk; smarter memory with elicitation and mnemo doctor; mobile see-and-delete; Node.js with the AI SDK memory package first; on hold: relay, Pro, 13.2, 13.4, Teams)? Keep Part B short. OUTPUT: finding records REV-011.N (mostly PRODUCT or PLAN-REFINEMENT), each with its evidence and sources in the body; the full report, with every source linked, as a file outside the repo at ~/mnemo-reviews/REV-011.md (the Planner hands it into the repo if the user wants it kept). Then set the review task ready with a short summary note, and ask the user to tell planner-a "REVIEW DONE: REV-011". Use the product terms: memory, shared memory for your agents, coordinate; never orchestration, orchestrator or swarm.
```

## 01M7RVPRDNEVEH4GMRYKX0AX5V

- type: episodic
- agent: `cursor-agent`
- key: `spec.v0.20.0`
- scope: private
- created: 2026-11-19T02:55:51Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"spec.v0.20.0"}`

```text
spec.v0.20.0 (planner-a, 2026-11-18; the user's go "Aligned with your recommendation"): v0.20.0 ships two things that change how saves and shared access work, with one REVIEWER-B pass at READY. (A) 3f, the fsync merge (docs/design/perf-2.md section 3f): defer Pager::flush's sync until after FMT-9c's direct writes, so one barrier covers the data pages and the direct runs before wal::commit; batch commits with a big run go from 4 fsyncs to 3, everything else stays 3. Must not fold that barrier into the commit's own fsync. Tests: failpoint crashes at every write around the merged sync (no committed memory lost, no uncommitted one visible after reopen), a proptest round of batch commits with big runs, fsync counts measured (device_budget) at 1K, 10K, 100K. (B) Sharing one file, continued (docs/design/12.4-shared-access.md implementation order steps 6 and 8): step 6, incremental catch-up: a shared handle at transaction t applies only the catalog and index deltas newer than t, keeps the resident vector column (adds new facts' vectors, drops deleted and superseded), keeps its page cache only if keyed by page and generation or after dropping every page a changed row names, and falls back to the full reload when the chain doesn't reach t; built behind the full reload with an equivalence test (a base rewrite, a tombstone compaction, a page reused between the two points) and measured against step 5's 5.2 ms at 10K and 34 ms at 100K per call after another process's commit. Step 8: `mnemo serve --mcp` with no flag becomes --shared (installers already write --shared); --exclusive keeps today's lifetime lock; the note decides whether to gate older binaries, and a format change for that comes to the user first. Order: a short design note and baseline for step 6 first (like PERF-2's v0190-1), reviewed by the Planner, then the builds.
```

## 01M7T8MPYWH528MY8WC0AYSD8Y

- type: episodic
- agent: `cursor-agent`
- key: `status.backlog-small`
- scope: private
- created: 2026-11-19T16:01:10Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795119095,"_mnemo.key":"status.backlog-small","_mnemo.superseded_by":"01M7TPZ7WHD2EBBYT4G7PP5E5G"}`

```text
status.backlog-small (planner-a, 2026-11-19): small items for the smarter-memory step (mnemo doctor and friends), after v0.20.0: (1) `mnemo task next --slug` or similar, since the Worker took and requeued the wrong task several times reaching a pointed slug (v0200-1h, v0200-3r); (2) stats.memories counts coordination records such as presence, which recall leaves out (found in v0200-4); (3) the catch-up leftovers in decision.catch-up-results (base-diff middle path, second-recall 5.7 ms, incremental catch-up rising to 1.2 ms over 200 commits).
```

## 01M7TPZ7WHD2EBBYT4G7PP5E5G

- type: episodic
- agent: `cursor-agent`
- key: `status.backlog-small`
- scope: private
- created: 2026-11-19T20:11:35Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"status.backlog-small","_mnemo.supersedes":"01M7T8MPYWH528MY8WC0AYSD8Y"}`

```text
status.backlog-small (planner-a, 2026-11-19): small items for the smarter-memory step (mnemo doctor and friends), after v0.20.0: (1) `mnemo task next --slug` or similar, since the Worker took and requeued the wrong task about 8 times this release reaching a pointed slug; (2) stats.memories counts coordination records such as presence, which recall leaves out (found in v0200-4); (3) the catch-up leftovers in decision.catch-up-results (base-diff middle path, second-recall 5.7 ms, incremental catch-up rising to 1.2 ms over 200 commits); (4) a stale MCP server: the sessions' three `mnemo serve --mcp --shared` processes started 2026-11-17 20:30 and still run that build (v0.18.x), while ~/.local/bin/mnemo is 0.19.1 (installed 2026-11-19 09:40). Hooks and CLI calls use the new binary; the MCP tools don't. A doctor check, or a one-line notice from the server or the hook when the installed binary is newer than the running one, would catch it. Until then: after each install, each session reconnects mnemo (/mcp in Claude Code), a user step.
```

## 01M7RWMBXZ973G264CW4B4J37N

- type: episodic
- agent: `cursor-agent`
- key: `status.discoverability`
- scope: private
- created: 2026-11-19T03:12:01Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795064000,"_mnemo.key":"status.discoverability","_mnemo.superseded_by":"01M7S2DWBR7XQXNXVFG28T18GR"}`

```text
status.discoverability (planner-a, 2026-11-18, measured with gh and the registries): GitHub repo toarchkumar/mnemo had 2 stars, 0 forks, 22 unique visitors in 14 days (186 views; referrers aocr.org 7, the site 3), no topics, no custom social preview, and the About description still old ("Encrypted SQLite like brain and orchestrator for your SLM/LLM on device use cases", which uses the banned word). Not listed in the official MCP registry. crates.io mnemo-engine 783 downloads (keywords ai, database, encryption, memory, vector; old description); PyPI about 7,751 a month (mostly mirrors and CI, likely). Research ideas from Supabase Select 2026 and Vercel Ship 2026: MCP elicitations before destructive tools; brain health advisors and scheduled agent checks; shareable inspector views; an AI SDK memory package (AI SDK 7) and an eve extension; MNemo as the memory file inside a Vercel Sandbox or Drive; reproducible public benchmarks (like dbarena); a crisp "X alternative" line.
```

## 01M7S2DWBR7XQXNXVFG28T18GR

- type: episodic
- agent: `cursor-agent`
- key: `status.discoverability`
- scope: private
- created: 2026-11-19T04:53:20Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795109884,"_mnemo.key":"status.discoverability","_mnemo.superseded_by":"01M7TE648FCEGQH6DQCDEV0NNZ","_mnemo.supersedes":"01M7RWMBXZ973G264CW4B4J37N"}`

```text
status.discoverability (planner-a, 2026-11-18): the user ran gh repo edit. Verified with gh repo view: the description is now "Shared memory for your coding agents, in one encrypted file. Written in Rust." (no more "orchestrator"), with 16 topics (agent-memory, ai-agents, claude-code, coding-agents, cursor, developer-tools, embeddings, encryption, llm-memory, local-first, mcp, mcp-server, model-context-protocol, multi-agent, rust, vector-search). Still open, offered but not confirmed by the user: a 1280x640 social preview image, an MCP registry server.json, crates.io and PyPI descriptions and keywords (Worker tasks), README badges and a demo (Planner), a launch day at v0.20.0. Before: 2 stars, 22 unique visitors in 14 days, no topics.
```

## 01M7TE648FCEGQH6DQCDEV0NNZ

- type: episodic
- agent: `cursor-agent`
- key: `status.discoverability`
- scope: private
- created: 2026-11-19T17:38:04Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"status.discoverability","_mnemo.supersedes":"01M7S2DWBR7XQXNXVFG28T18GR"}`

```text
status.discoverability (planner-a, 2026-11-19): done: the repo description ("Shared memory for your coding agents, in one encrypted file. Written in Rust.") and 16 topics (user, 2026-11-18); the custom social preview (user uploaded docs/img/social-preview.png 2026-11-19; gh graphql usesCustomOpenGraphImage true); crates.io and PyPI descriptions and keywords merged (v0200-d2, live with the v0.20.0 publish); the MCP registry bundle and server.json merged (v0200-d3; hardening v0200-d3h). Open: v0200-d4 the demo GIF; README badges and the GIF's placement in the Planner's v0.20.0 version-text hand-off; after the tag, the user's mcp-publisher login and publish (RELEASING.md). No launch day for now (user).
```

## 01M7TPNR4S10SQBQAR07PPJM5C

- type: episodic
- agent: `cursor-agent`
- key: `status.ideas-file`
- scope: private
- created: 2026-11-19T20:06:24Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"status.ideas-file"}`

```text
status.ideas-file (planner-a, 2026-11-19): the user asked for a succinct list of the ideas we've explored. docs/ideas.md (Planner-owned, uncommitted) covers hosted brains (REV-011, on hold), encrypted sync's relay, Pro and Teams (on hold), public benchmarks (backlog), and demo and launch (GIF chosen; video, in-browser demo and launch day not now). It replaces the long untracked docs/design/hosted-brains.md, now removed; the full report stays outside the repo at ~/mnemo-reviews/REV-011.md. Hand it off with the v0.20.0 version-text hand-off (README.md, index.html, docs/ideas.md). Add each newly explored idea there.
```

## 01M7TP71W90HCRC0EA6S25ARZ1

- type: episodic
- agent: `cursor-agent`
- key: `status.rev-011`
- scope: private
- created: 2026-11-19T19:58:23Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"status.rev-011"}`

```text
status.rev-011 (planner-a, 2026-11-19): REVIEWER-A's REV-011 is done (report ~/mnemo-reviews/REV-011.md, findings REV-011.1-.6). Verdict: (B) MNemo selling hosted brains: no-go (Turso with BYOK on Pro, Cloudflare Durable Objects and turbopuffer already sell per-tenant isolation cheaply; a router, leases, 24/7 SRE and SOC 2 don't pay for a small team). (A) The user's own consultancy on MNemo: prove first, with a 1-week pilot on v0.20.0 with no engine change (keys wrapped by KMS outside the engine via open_with_key, a read-only playbook file plus one file per client, one sticky host or Vercel Sandbox with Drives, S3 conditional writes), gated on 2-3 paid pilot commitments from 5-10 buyers. Memory infra is under 0.5% of consulting revenue; LLM tokens and compliance dominate. Guardrails: never full-file write-back per commit; one shared KMS key wrapping per-tenant data keys. Part B: the roadmap after v0.20.0 stays; optional swap of Node.js ahead of mobile. Triage: .4 accepted into roadmap step 2's design (change sets as immutable files fit for conditional writes); .2, .3 deferred; .1, .5, .6 needs-user. Spot-checked and confirmed: Vercel Drives GA, Turso BYOK on Pro, Claude Managed Agents memory stores. Not confirmed: Turso's $417 Pro price, the Harvey and Hebbia seat prices.
```

## 01M7P5HQ3XCY792ZH38JFEER8G

- type: episodic
- agent: `cursor-agent`
- key: `status.roadmap`
- scope: private
- created: 2026-11-18T01:50:09Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1794976518,"_mnemo.key":"status.roadmap","_mnemo.superseded_by":"01M7PF04H3SXFRVZH441NFR94F"}`

```text
Roadmap status (2026-11-17): v0.18.1 (INSPECT-COORD, task refs) in release prep; bump PR 132 72d78b5 merged, dry run 44213067706 green; waiting on the user's install of v0.18.0 for the docs/brain.md backup step. Then: HARNESS-DOGFOOD-1 Run 2 with PERF-2 as its work in v0.19.0 (with #[non_exhaustive]); sync is decided later on the waitlist's numbers.
```

## 01M7PF04H3SXFRVZH441NFR94F

- type: episodic
- agent: `cursor-agent`
- key: `status.roadmap`
- scope: private
- created: 2026-11-18T04:35:18Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795015123,"_mnemo.key":"status.roadmap","_mnemo.superseded_by":"01M7QKT8F0XB7Z4K8R9QNRYTKX","_mnemo.supersedes":"01M7P5HQ3XCY792ZH38JFEER8G"}`

```text
Roadmap status (2026-11-17): v0.18.1 released; the README/site rework is live (PR 134). HARNESS-DOGFOOD-1 Run 2 started: the brain is the ledger, PLAN.md and reviewer_notes.md frozen. v0.19.0 is Run 2's release: tasks v0190-0 (setup) to v0190-6 (READY): PERF-2 (spec.perf-2, design then build), non_exhaustive on the coordination and findings types, the inspect date filter (spec.inspect-date-filter) and a Findings screenshot from a demo brain, a site hand-off. Then judge Run 2, then sync, decided on the waitlist's numbers.
```

## 01M7QKT8F0XB7Z4K8R9QNRYTKX

- type: episodic
- agent: `cursor-agent`
- key: `status.roadmap`
- scope: private
- created: 2026-11-18T15:18:43Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795053750,"_mnemo.key":"status.roadmap","_mnemo.superseded_by":"01M7RRN2CK0K59DCDVYWNH7591","_mnemo.supersedes":"01M7PF04H3SXFRVZH441NFR94F"}`

```text
Roadmap status (2026-11-18): v0.19.0 released (tag on b345073, run 44284629207): PERF-2 (first search 11.5 ms at 100K, 4.2 ms at 10K; warm 10K 2.3 ms, target 2 not met, accepted), index layout 2 in v9, inspect date range and screenshot, non_exhaustive coordination and findings types, mobile 0.19.0. REVIEWER-B RB-015: two LOW findings accepted for v0.19.1 (v0191-1). Run 2's whole release ran from brain tasks, reports and facts, with no hand-edited PLAN.md or reviewer_notes.md. Next: judge Run 2, v0.19.1, the 3f fsync merge, the sync design note.
```

## 01M7RRN2CK0K59DCDVYWNH7591

- type: episodic
- agent: `cursor-agent`
- key: `status.roadmap`
- scope: private
- created: 2026-11-19T02:02:30Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"status.roadmap","_mnemo.supersedes":"01M7QKT8F0XB7Z4K8R9QNRYTKX"}`

```text
Roadmap status (2026-11-18): v0.19.1 released (tag on 2d21ac6, run 44430183319, mobile off; crates.io and PyPI 0.19.1, GitHub Release 12 assets; mobile stays 0.19.0): RB-015.1 (a damaged index run loads as no index, notice, rebuilt by the next write; verify_index; I/O errors retried) and RB-015.2 (layout-2 fuzz target, compat at 20K, tab link). v0.19.0 the same day (PERF-2). Run 2 made permanent (AGENTS.md 470b9de). Next per decision.roadmap-order: the 3f fsync merge (the user picks patch or v0.20.0), the sync design note, sharing one file continued.
```

## 01M7QKTKNR4YSV3KC0EE8CM0HM

- type: episodic
- agent: `cursor-agent`
- key: `status.run-2`
- scope: private
- created: 2026-11-18T15:18:54Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"status.run-2"}`

```text
Run 2 (HARNESS-DOGFOOD-1) result, judged by planner-a 2026-11-18: PASSED on its test. From dispatch (v0190-0) to the v0.19.0 tag, nobody edited PLAN.md or reviewer_notes.md (git log 9fb1191..v0.19.0 shows only the freeze, 72ae920). 10 tasks plus a REVIEWER-B task, 10 report.* facts, 2 findings as records (RB-015.1, RB-015.2), all pointers named a slug. Recall at task start: 9 found, 2 partial (v0190-0: decision.run-2 not in the top 5 for its title; v0190-5: older hand-off notes came back), 0 not found. Friction: recall of a just-written decision by a differently worded title; one Worker local-branch slip; one macOS CI timing flake. Keeping the rule permanently (and the AGENTS.md wording) is the user's call.
```

## 01M7RVV9JCPDBBNARBH8EKRXFX

- type: episodic
- agent: `cursor-agent`
- key: `status.sync-design`
- scope: private
- created: 2026-11-19T02:58:20Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795064234,"_mnemo.key":"status.sync-design","_mnemo.superseded_by":"01M7S2N0VY11TV5GG6BJ2CRTJK"}`

```text
status.sync-design (planner-a, 2026-11-18): the sync design note is drafted at docs/design/sync.md (working tree, uncommitted, Planner's file; hand-off as a task once the user has read it). Shape: replicas exchange padded AES-GCM envelopes of whole records through an append-only mailbox relay that sees only ciphertext; keys derived from the data key by HKDF (sync key, mailbox id, write and read tokens); version vectors per replica, idempotent apply by ULID and content hash; a hybrid logical clock in _mnemo.hlc for newest-wins; 13.7's merge rules; checkpoints to trim the mailbox; claims, presence, snapshots and derived data don't sync. Build order: 13.7 mnemo merge (free), change sets on disk (free), relay protocol, hosted relay (Pro), Teams after 13.2 and 13.4. Open for the user: when (steps 1-2 now, or wait on the waitlist), whether messages sync, pricing, hosting.
```

## 01M7S2N0VY11TV5GG6BJ2CRTJK

- type: episodic
- agent: `cursor-agent`
- key: `status.sync-design`
- scope: private
- created: 2026-11-19T04:57:14Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"status.sync-design","_mnemo.supersedes":"01M7RVV9JCPDBBNARBH8EKRXFX"}`

```text
status.sync-design (planner-a, 2026-11-18): the user read the summary of docs/design/sync.md and approved it. Hand-off task v0200-1h (after v0200-1) commits it alone on release-0.20.0 as 'docs(design): the encrypted sync design note'. The note now says steps 1 (13.7 mnemo merge) and 2 (change sets on disk) are on the roadmap after v0.20.0; steps 3-5 (relay protocol, hosted relay/Pro, Teams) are on hold per decision.roadmap-order. Shape: padded AES-GCM envelopes of whole records through an append-only mailbox relay that sees only ciphertext; HKDF-derived sync key, mailbox id, write/read tokens; version vectors, idempotent apply, an HLC in _mnemo.hlc; 13.7's merge rules; messages don't sync in v1.
```

## 01M7NG7T0DCNRRBV7QV97TQ196

- type: episodic
- agent: `cursor-agent`
- key: `status.v0.18.0`
- scope: private
- created: 2026-11-17T19:37:44Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1794957523,"_mnemo.key":"status.v0.18.0","_mnemo.superseded_by":"01M7NWWE3769P0GBDKHPN857QM","area":"release","topic":"v0.18.0"}`

```text
v0.18.0 READY (2026-11-17): release-0.18.0 at 430a21a after PR 125 (main merged, 9f3ab28), 123 (findings), 126 (export --git backup, docs/brain.md), 127 (thread essence and unverified SECURITY findings out of --git), 128 (bump, mobile true). Dry run 44156579200 succeeded with mobile on; Central deployment validated and dropped. Left: Planner HANDOFF of README/index.html, the backup/coordination commit, main fast-forward, tag, the user's approval click.
```

## 01M7NWWE3769P0GBDKHPN857QM

- type: episodic
- agent: `cursor-agent`
- key: `status.v0.18.0`
- scope: private
- created: 2026-11-17T23:18:43Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"status.v0.18.0","_mnemo.supersedes":"01M7NG7T0DCNRRBV7QV97TQ196","area":"release","topic":"v0.18.0"}`

```text
v0.18.0 released 2026-11-17: tag v0.18.0 at 9cd5daf, release run 44191123230 (31 jobs, mobile on, user approved). crates.io, PyPI (5 wheels + sdist), mnemo-swift tag 0.18.0 (ed32685), Maven Central (repo1 live ~12 min after the job), GitHub Release with 14 assets; site shows v0.18.0. PRs: 123 findings, 125 main merge, 126 export --git backup, 127 thread essence and unverified SECURITY out of --git, 128 bump, 129 review round 1 (RB-014). release-0.18.0 deleted. Next: v0.18.1 (INSPECT-COORD, task refs), whose branch starts with the AGENTS.md WAITING-pointer handoff commit.
```

## 01M7P8GFDYS6R9YQP20HZ9B74W

- type: episodic
- agent: `cursor-agent`
- key: `status.v0.18.1`
- scope: private
- created: 2026-11-18T02:41:54Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.invalid_at":1794973762,"_mnemo.key":"status.v0.18.1","_mnemo.superseded_by":"01M7PCC0NQXNNBPZ4JENGG42Q2","area":"release","topic":"v0.18.1"}`

```text
v0.18.1 READY (2026-11-17): release-0.18.1 at 7c93da4 after 88c3c37 (AGENTS.md WAITING pointer), PR 130 (inspector findings/tasks tabs), 131 (task refs), 132 (bump, mobile false), 133 (docs/brain.md re-exported by installed mnemo 0.18.0: 292 records; tests/brain_backup.rs header check). Dry run 44213067706 on 72d78b5 succeeded with mobile off. Left: Planner HANDOFF of README/index.html, coordination-file commit, main fast-forward, tag v0.18.1, user approval (log WAITING: at the stop).
```

## 01M7PCC0NQXNNBPZ4JENGG42Q2

- type: episodic
- agent: `cursor-agent`
- key: `status.v0.18.1`
- scope: private
- created: 2026-11-18T03:49:22Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"status.v0.18.1","_mnemo.supersedes":"01M7P8GFDYS6R9YQP20HZ9B74W","area":"release","topic":"v0.18.1"}`

```text
v0.18.1 released 2026-11-17: tag v0.18.1 at 863c6fe, release run 44225322609 (user approved after a WAITING: pointer, the first), mobile off: crates.io and PyPI 0.18.1, GitHub Release 12 assets, mnemo-swift stays 0.18.0; release-0.18.1 deleted. Then the README/site rework (three layers, mnemo inspect as hero step 3) merged to main as PR 134 9fb1191 (docs-only, no CI by paths-ignore), live on Pages. Next: v0.19.0 (PERF-2, #[non_exhaustive] on 13a types, inspector date filter, a Findings screenshot from a demo brain).
```

## 01M7Q1R53H1DN8BP1CBXTSYRRC

- type: episodic
- agent: `cursor-agent`
- key: `status.v0.19.0`
- scope: private
- created: 2026-11-18T10:03:00Z
- importance: 0.7
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"status.v0.19.0","area":"release","topic":"v0.19.0"}`

```text
v0.19.0 READY (2026-11-18), Run 2 release: release-0.19.0 at 9b78312 after d3f828a and 72ae920 (Run 2 rules, archives frozen), PR 135 (PERF-2 design), 136 (PERF-2 build 8a63540: first recall 11.5 ms at 100K, 4.2 ms at 10K), 137 (non_exhaustive types b97e689), 138 (inspector date range and screenshot dabb5fc), 3ba2f7b (site hand-off), 139 (bump, mobile true, CHANGELOG, docs/brain.md by installed 0.18.1, 326 records). Dry run 44278116677 succeeded with mobile on. Left: REVIEWER-B READY pass, the version-text hand-off (README, index.html still name 0.18.1), main fast-forward, tag, the user approval click.
```

## 01M7SAKGX8N8T9RBDJHHCY4071

- type: episodic
- agent: `cursor-agent`
- key: `status.v0200-review-scope`
- scope: private
- created: 2026-11-19T07:16:14Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795090258,"_mnemo.key":"status.v0200-review-scope","_mnemo.superseded_by":"01M7SVF6VAGH85T37WF14SKFVH"}`

```text
status.v0200-review-scope (planner-a, 2026-11-18): items for REVIEWER-B's scoped v0.20.0 READY pass, gathered as the release builds. (1) v0200-1 (f2898a9): the barrier fsync before wal::commit, and the (header, free, free_dirty) restore when staging fails before the commit point. (2) v0200-1r (3a295ed): commit_failed refuses later flushes with MaybeStored until reopen; check that every path past the commit point sets it, and that close/try_close behave. (3) The Worker's open question: after a restore before the commit point, the retry rewrites the same page numbers under the same generation without renewing the session's lease (nonces stay unique through SEC-1's per-session prefix). Does that weaken what ensure_gen_lease protects against, e.g. a stale page from the failed attempt authenticating in place of a lost newer write? (4) Later: v0200-3 incremental catch-up, v0200-4 shared by default, v0200-d3's release.yml change if any.
```

## 01M7SVF6VAGH85T37WF14SKFVH

- type: episodic
- agent: `cursor-agent`
- key: `status.v0200-review-scope`
- scope: private
- created: 2026-11-19T12:10:58Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.invalid_at":1795108100,"_mnemo.key":"status.v0200-review-scope","_mnemo.superseded_by":"01M7TCFP5XFZ7729YA6JD0HGTX","_mnemo.supersedes":"01M7SAKGX8N8T9RBDJHHCY4071"}`

```text
status.v0200-review-scope (planner-a, 2026-11-19): items for REVIEWER-B's scoped v0.20.0 READY pass, gathered as the release builds. (1) v0200-1 (f2898a9): the barrier fsync before wal::commit, and the (header, free, free_dirty) restore when staging fails before the commit point. (2) v0200-1r (3a295ed): commit_failed refuses later flushes with MaybeStored until reopen; check every path past the commit point sets it, and close/try_close. (3) The Worker's question: after a restore before the commit point, the retry rewrites the same page numbers under the same generation without renewing the lease (nonces unique via SEC-1's per-session prefix); does that weaken what ensure_gen_lease protects? (4) v0200-3 (ed4e5b1) incremental catch-up: store/catchup.rs and shared.rs 583-660; the (page, generation) page-cache key in cache.rs and pager.rs; the generation walk-back rule; adopt_column; the ANN reconcile skip rule (fact.catch-up-ann-reconcile). (5) v0200-3r: the restore-in-place guard (wal_seq not going back, and the head page's on-disk nonce). (6) Later: v0200-4 shared by default; v0200-d3's release.yml change, if any.
```

## 01M7TCFP5XFZ7729YA6JD0HGTX

- type: episodic
- agent: `cursor-agent`
- key: `status.v0200-review-scope`
- scope: private
- created: 2026-11-19T17:08:20Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.key":"status.v0200-review-scope","_mnemo.supersedes":"01M7SVF6VAGH85T37WF14SKFVH"}`

```text
status.v0200-review-scope (planner-a, 2026-11-19): items for REVIEWER-B's scoped v0.20.0 READY pass, gathered as the release builds. (1) v0200-1 (f2898a9): the barrier fsync before wal::commit, and the (header, free, free_dirty) restore when staging fails before the commit point. (2) v0200-1r (3a295ed): commit_failed refuses later flushes with MaybeStored until reopen; check every path past the commit point sets it, and close/try_close. (3) The Worker's question: after a restore before the commit point, the retry rewrites the same page numbers under the same generation without renewing the lease (nonces unique via SEC-1's per-session prefix); does that weaken what ensure_gen_lease protects? (4) v0200-3 (ed4e5b1) incremental catch-up: store/catchup.rs and shared.rs; the (page, generation) page-cache key in cache.rs and pager.rs; the generation walk-back rule; adopt_column; the ANN reconcile skip rule (fact.catch-up-ann-reconcile). (5) v0200-3r (b2a2dc0): the restore-in-place guard (anchors_hold: wal_seq not going back, on-disk nonces of kept runs; superseded runs skipped; drop_newer_than). (6) v0200-4 (54afc04): serve --mcp shared by default, --exclusive, --allow-network-fs. (7) The release workflow: v0200-d3 (45d811e) adds the mcp-bundle job, a /bin/sh launcher in the bundle that runs the release binaries (integrations/mcpb/mnemo.sh), and server.json; v0200-d3h moves the validators out of the building job and attests the bundle.
```

## 01M7NT5YYT1X243FM6KGJ4RZQ5

- type: procedural
- agent: `cursor-agent`
- key: `testing.v0-17-compat`
- scope: private
- created: 2026-11-17T22:31:29Z
- importance: 0.6
- metadata: `{"_mnemo.author":"worker-a","_mnemo.key":"testing.v0-17-compat","area":"testing","topic":"compat"}`

```text
Mixed-version tests against a published CLI: tests/v0_16_compat.rs (MNEMO_V0_16_1_BIN) and tests/v0_17_compat.rs (MNEMO_V0_17_2_BIN, since v0.18.0 PR 129) skip unless the env var names the old binary; CI's compat (v0.16.1|v0.17.2, linux) jobs cargo install the exact version into $RUNNER_TEMP. Locally, v0_17_compat runs against the installed ~/.local/bin/mnemo (any 0.17.x) on a temp file with a test passphrase, never the keychain.
```

## 01M7TFQQRXBZ9B7PYSHC6676BV

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T18:05:09Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"reviewer-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"reviewer","_mnemo.slug":"rev-011","_mnemo.status":"done","_mnemo.status_history":[{"at":1795112291,"by":"reviewer-a","from":"queued","to":"active"},{"at":1795116164,"by":"reviewer-a","from":"active","note":"Recall: partial (found spec.rev-011 and coordination/Phase 13 context; no prior hosted-brains research stored). Recorded REV-011.1 to REV-011.6 and full sourced report at ~/mnemo-reviews/REV-011.md. Part A verdict: Reading (B) (MNemo selling hosted open-handle brains) is NO-GO against Turso/Cloudflare DO/turbopuffer and fixed SRE/SOC 2 costs; Reading (A) (consultancy on MNemo) is PROVE FIRST via a 1-week zero-engine-change spike on v0.19.1/v0.20.0 using delta change sets (or Vercel Drives), S3 If-Match fencing, and hierarchical envelope KMS. Part B pre-phase check: decision.roadmap-order after v0.20.0 remains sound (steps 1-3 reinforced; paid features stay on hold), with one optional user trade-off to weigh: swapping Step 5 (Node.js/AI SDK + sandbox/drives guide) ahead of Step 4 (mobile see-and-delete).","to":"ready"},{"at":1795118302,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: 6 findings recorded and triaged (.4 accepted into roadmap step 2's design; .2 and .3 deferred; .1, .5 and .6 needs-user); the report is at ~/mnemo-reviews/REV-011.md (52 KB). Code claims checked (open_with_key and raw_key in open.rs and crypto.rs; shared mode refusing network folders and keeping its secret). Three market claims spot-checked and confirmed. The verdict is waiting on the user.","to":"done"}]}`

```text
REVIEWER-A: REV-011, hosted brains research and the pre-phase check

For REVIEWER-A only (REVIEWER-B: leave it). The user called this review on 2026-11-19. First step: recall. The scope is the keyed fact spec.rev-011 (mnemo history --key spec.rev-011, then mnemo get <id>), with docs/design/hosted-brains.md. Part A, deep research: is a .mnemo file per tenant an efficient backend for an agent harness giving standardized consultation? Part B: a short pre-phase check of the roadmap after v0.20.0. Output: REV-011.N finding records plus the full report at ~/mnemo-reviews/REV-011.md. Then set this task ready with a summary note.
```

## 01M7N18T0CYF8DA9TSYEZJDC1E

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T15:16:08Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"site-waitlist","_mnemo.status":"done","_mnemo.status_history":[{"at":1794933056,"by":"worker-a","from":"queued","to":"active"},{"at":1794933205,"by":"worker-a","from":"active","note":"PLAN.md: site-waitlist live (PR 124, 2bf9b74), Pages byte-equal","to":"ready"},{"at":1794933230,"by":"planner-a","from":"ready","to":"done"}]}`

```text
Site: the sync waitlist at the top

PLAN.md: 2026-11-17 — PLANNER-A — the waitlist moves to the top of the site; do it right after v0180-1
```

## 01M7KT4HXDBP8C63N9CFG2T01M

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T03:52:14Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0172-1","_mnemo.status":"done","_mnemo.status_history":[{"at":1794887569,"by":"worker-a","from":"queued","to":"active"},{"at":1794893724,"by":"worker-a","from":"active","note":"PLAN.md: Run 1 round 1: READY-NOTICE merged (PR 118, dedbba0), task v0172-1 ready","to":"ready"},{"at":1794893743,"by":"planner-a","from":"ready","note":"accepted: PR 118 dedbba0","to":"done"}]}`

```text
Run 1 round 1: READY-NOTICE (v0.17.2)

PLAN.md: 2026-11-16 — PLANNER-A — DECISION + HANDOFF + DISPATCH — rule 9's tasks bullet (the user: "Yes"); Run 1 round 1: v0172-1, READY-NOTICE
```

## 01M7M02KM4Z0B88NDGKM25TQAQ

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T05:36:02Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0172-2","_mnemo.status":"done","_mnemo.status_history":[{"at":1794893778,"by":"worker-a","from":"queued","to":"active"},{"at":1794899717,"by":"worker-a","from":"active","note":"PLAN.md: Run 1 round 2: polish and presence at start merged (PR 119, 997200c), task v0172-2 ready","to":"ready"},{"at":1794899732,"by":"planner-a","from":"ready","note":"accepted: PR 119 997200c","to":"done"}]}`

```text
Run 1 round 2: CLI and MCP polish, presence at start (v0.17.2)

PLAN.md: 2026-11-17 — PLANNER-A — REVIEW + DISPATCH — Run 1 round 1 accepted (v0172-1 done); the tally's first row; round 2: v0172-2
```

## 01M7M5S89KBMHPRM9008T66VWX

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T07:15:47Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0172-3","_mnemo.status":"done","_mnemo.status_history":[{"at":1794899755,"by":"worker-a","from":"queued","to":"active"},{"at":1794908279,"by":"worker-a","from":"active","note":"PLAN.md: Run 1 round 3: MODEL-RETRY and WHO-SHAPE merged (PR 120, f623159), task v0172-3 ready","to":"ready"},{"at":1794908301,"by":"planner-a","from":"ready","note":"accepted: PR 120 f623159","to":"done"}]}`

```text
Run 1 round 3: MODEL-RETRY and one who shape (v0.17.2)

PLAN.md: 2026-11-17 — PLANNER-A — REVIEW + DISPATCH — Run 1 round 2 accepted (v0172-2 done); round 3: v0172-3, MODEL-RETRY and one who shape
```

## 01M7MDYA4BFRMGK2MBD5ATZE0E

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T09:38:21Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0172-4","_mnemo.status":"done","_mnemo.status_history":[{"at":1794908309,"by":"worker-a","from":"queued","to":"active"},{"at":1794914655,"by":"worker-a","from":"active","note":"gates done; committing PR next","to":"active"},{"at":1794915473,"by":"worker-a","from":"active","note":"PLAN.md: Run 1 round 4: the legacy scan narrowed and the claim race fixed (PR 121, 796a3ad), task v0172-4 ready","to":"ready"},{"at":1794915494,"by":"planner-a","from":"ready","note":"accepted: PR 121 796a3ad","to":"done"}]}`

```text
Run 1 round 4: claim race test and legacy scan (v0.17.2)

PLAN.md: 2026-11-17 — PLANNER-A — REVIEW + DISPATCH — Run 1 round 3 accepted (v0172-3 done); round 4: v0172-4, the claim race test and the legacy scan
```

## 01M7MMSTE7V1VA45KJ4CPGJVXG

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T11:38:14Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0172-1","v0172-2","v0172-3","v0172-4"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0172-5","_mnemo.status":"done","_mnemo.status_history":[{"at":1794915503,"by":"worker-a","from":"queued","to":"active"},{"at":1794918943,"by":"worker-a","from":"active","note":"PLAN.md: Run 1 round 5: v0.17.2 bumped (PR 122, bf73ef8); dry run 44081832937 green, mobile off","to":"ready"},{"at":1794918958,"by":"planner-a","from":"ready","note":"accepted: PR 122 bf73ef8, dry run 44081832937","to":"done"}]}`

```text
Run 1 round 5: READY for v0.17.2

PLAN.md: 2026-11-17 — PLANNER-A — REVIEW + DISPATCH — Run 1 round 4 accepted (v0172-4 done); round 5: v0172-5, READY
```

## 01M7N0JH72MZXRMFMK21FXEFMM

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T15:03:58Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0180-1","_mnemo.status":"done","_mnemo.status_history":[{"at":1794927858,"by":"worker-a","from":"queued","to":"active"},{"at":1794931911,"by":"worker-a","from":"active","note":"gates green but one count; fixed; PR next","to":"active"},{"at":1794933050,"by":"worker-a","from":"active","note":"PLAN.md: v0180-1: findings as records merged (PR 123, 52feb44), task v0180-1 ready","to":"ready"},{"at":1794933112,"by":"planner-a","from":"ready","to":"done"}]}`

```text
v0.18.0: findings as records

PLAN.md: 2026-11-17 — PLANNER-A — DECISION + HANDOFF + DISPATCH — v0.18.0 the harness starts; rule 9; tasks v0180-1 to -3
```

## 01M7N0JH93GKS42VSRRK9NWE7M

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T15:03:58Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0180-1"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.slug":"v0180-2","_mnemo.status":"done","_mnemo.status_history":[{"at":1794933210,"by":"worker-a","from":"queued","to":"active"},{"at":1794939593,"by":"worker-a","from":"active","note":"PLAN.md: WORKER — DONE — v0180-2: the brain's backup (PR 126, d4f95e7)","to":"ready"},{"at":1794939692,"by":"planner-a","from":"ready","to":"done"}]}`

```text
v0.18.0: the backup (export --git with findings and essence, docs/brain.md)

PLAN.md: same entry, v0180-2
```

## 01M7N0JHB0JC2YF917CJAGYST2

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T15:03:58Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0180-1","v0180-2"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0180-3","_mnemo.status":"done","_mnemo.status_history":[{"at":1794939706,"by":"worker-a","from":"queued","to":"active"},{"at":1794944250,"by":"worker-a","from":"active","note":"PLAN.md: WORKER — READY — v0180-3: v0.18.0 (PR 127 635ae00, PR 128 430a21a; dry run green)","to":"ready"},{"at":1794949925,"by":"planner-a","from":"ready","to":"done"}]}`

```text
v0.18.0: READY

PLAN.md: same entry, v0180-3
```

## 01M7NJ5W10EP0A9NW05Q77FHJW

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T20:11:37Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0180-4","_mnemo.status":"done","_mnemo.status_history":[{"at":1794946320,"by":"worker-a","from":"queued","to":"active"},{"at":1794947140,"by":"worker-a","from":"active","note":"local gates running","to":"active"},{"at":1794949948,"by":"worker-a","from":"active","note":"PR 129 CI running","to":"active"},{"at":1794953712,"by":"worker-a","from":"active","note":"dry run re-run (runner capacity)","to":"active"},{"at":1794954677,"by":"worker-a","from":"active","note":"PLAN.md: WORKER — READY — v0180-4: v0.18.0 review round 1 (RB-014) (PR 129 61bb573; dry run green)","to":"ready"},{"at":1794954728,"by":"planner-a","from":"ready","to":"done"}]}`

```text
v0.18.0: review round 1 (RB-014)

PLAN.md: 2026-11-17 PLANNER-A REVIEW + DISPATCH + NEEDS-USER, RB-014 triaged; the spec is the two items' Planner Triage lines.
```

## 01M7NWZHCK2BSWFPDYFWAH2VWR

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T23:20:24Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0181-1","_mnemo.status":"done","_mnemo.status_history":[{"at":1794957636,"by":"worker-a","from":"queued","to":"active"},{"at":1794959091,"by":"worker-a","from":"active","note":"PLAN.md: WORKER — DONE — v0181-1: INSPECT-COORD, findings and tasks in the inspector (PR 130, 4c696f0)","to":"ready"},{"at":1794959116,"by":"planner-a","from":"ready","to":"done"}]}`

```text
v0.18.1: findings and tasks in the inspector

PLAN.md: 2026-11-17 PLANNER-A DISPATCH v0.18.1 INSPECT-COORD and task refs
```

## 01M7NWZHECPHPK6PKA90DDYKPN

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T23:20:24Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0181-2","_mnemo.status":"done","_mnemo.status_history":[{"at":1794959095,"by":"worker-a","from":"queued","to":"active"},{"at":1794959497,"by":"worker-a","from":"active","note":"local gates running","to":"active"},{"at":1794962274,"by":"worker-a","from":"active","note":"PR 131 CI","to":"active"},{"at":1794963022,"by":"worker-a","from":"active","note":"PLAN.md: WORKER — DONE — v0181-2: task refs (PR 131, 98c2a68)","to":"ready"},{"at":1794963069,"by":"planner-a","from":"ready","to":"done"}]}`

```text
v0.18.1: task refs

PLAN.md: 2026-11-17 PLANNER-A DISPATCH v0.18.1 INSPECT-COORD and task refs
```

## 01M7NWZHG61Y6ZKKD8TR318FW9

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-17T23:20:24Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0181-1","v0181-2"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0181-3","_mnemo.status":"done","_mnemo.status_history":[{"at":1794963080,"by":"worker-a","from":"queued","to":"active"},{"at":1794966582,"by":"worker-a","from":"active","note":"WAITING: the user's install of v0.18.x for step 7","to":"active"},{"at":1794969706,"by":"worker-a","from":"active","note":"PLAN.md: WORKER — READY — v0181-3: v0.18.1 (PR 132 72d78b5, PR 133 7c93da4; dry run green)","to":"ready"},{"at":1794969758,"by":"planner-a","from":"ready","to":"done"}]}`

```text
v0.18.1: READY

PLAN.md: 2026-11-17 PLANNER-A DISPATCH v0.18.1 INSPECT-COORD and task refs
```

## 01M7PEZDGXHG6Y806A4CDJGJTV

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T04:34:55Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["AGENTS.md","PLAN.md","reviewer_notes.md","decision.run-2"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-0","_mnemo.status":"done","_mnemo.status_history":[{"at":1794976530,"by":"worker-a","from":"queued","to":"active"},{"at":1794976559,"by":"worker-a","from":"active","note":"report.v0190-0","to":"ready"},{"at":1794976575,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: d3f828a is AGENTS.md alone, 72ae920 is PLAN.md and reviewer_notes.md alone, origin/release-0.19.0 at 72ae920 on main 9fb1191. Tally: partial (decision.run-2 missed the top 5 for the task's title).","to":"done"}]}`

```text
v0.19.0 setup: the release branch, the Run 2 rules commit, and the archives frozen

Run 2 starts here (decision.run-2; AGENTS.md subsection 'The brain is the ledger'). First step: recall, and say what came back in your report.
1. Branch release-0.19.0 from main (9fb1191).
2. Commit AGENTS.md alone (the Planner's protocol text, the user's yes 2026-11-17): 'docs(agents): the brain is the ledger (HARNESS-DOGFOOD-1 Run 2)'.
3. Commit PLAN.md and reviewer_notes.md by path, as they stand, as the last coordination commit: 'chore(plan): freeze the coordination files as archives (Run 2)'. Both now carry an archive head note. After this nobody edits them.
4. Push the branch. No PR is needed for these two commits on the release branch's start, as with v0.18.1's first commit.
Report: report.v0190-0 (hashes), then task update v0190-0 --status ready --note report.v0190-0, then the pointer DONE: v0190-0.
```

## 01M7PEZDK4GDA0XXXQGP8NYYCY

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T04:34:55Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-0"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["spec.perf-2"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-1","_mnemo.status":"done","_mnemo.status_history":[{"at":1794976589,"by":"worker-a","from":"queued","to":"active"},{"at":1794977998,"by":"worker-a","from":"active","note":"report.v0190-1","to":"ready"},{"at":1794978033,"by":"planner-a","from":"ready","note":"REVIEWED by planner-a: design accepted with the order and scope in decision.perf-2-design (merge PR 135 first; stage A before lazy partitions; 3f deferred; no parallel warm scan). Tally: found.","to":"done"}]}`

```text
PERF-2 step 1: baseline on v0.18.1 and the design note

Read spec.perf-2 (mnemo history --key spec.perf-2, or recall). First step: recall, and say what came back.
Do step 1 only: the baseline numbers on v0.18.1 (each from a command you ran) and docs/design/perf-2.md, on a branch from release-0.19.0 with a PR into it. No engine change. Report in report.v0190-1 with the numbers table and the note's choices; ready for the Planner's review. The build (v0190-2) waits for my done here.
```

## 01M7PEZDN4DHAEEEPFDGV1NKJS

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T04:34:55Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-1"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["spec.perf-2"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-2","_mnemo.status":"done","_mnemo.status_history":[{"at":1794978053,"by":"worker-a","from":"queued","to":"active"},{"at":1794981945,"by":"worker-a","from":"active","note":"built; local gates running, then the final measurements","to":"active"},{"at":1794987107,"by":"worker-a","from":"active","note":"final numbers measured; re-checking after the RunReader fix","to":"active"},{"at":1794989040,"by":"worker-a","from":"active","note":"report.v0190-2","to":"ready"},{"at":1794989083,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 136 merged as 8a63540 into release-0.19.0, PR checks 26 of 26; perf_v0.19.0.json matches report.v0190-2. First recall 100K 11.5 ms (target 15) and 10K 4.2 ms (target 5) met; open and RSS held or better; ranking and recall@10 unchanged; compat with published 0.18.1 tested in CI. Warm 10K 2.3 ms against a 2 ms target: accepted as not met, reported honestly (decision.perf-2-design point 5); no further work this release. The root README's old first-search numbers and the site roadmap line go in v0190-5. Tally: found.","to":"done"}]}`

```text
PERF-2 step 2: the build

Build the design accepted in v0190-1 (my done note there may add changes). Spec: spec.perf-2, step 2. First step: recall. A PR into release-0.19.0; measured numbers in test/scripts/perf_v0.19.0.json and in report.v0190-2 against every target and every must-not-regress line.
```

## 01M7PEZDQ69H4Y7P7QPC3DF659

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T04:34:55Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-0"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["decision.non-exhaustive-0.19"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-3","_mnemo.status":"done","_mnemo.status_history":[{"at":1794989113,"by":"worker-a","from":"queued","to":"active"},{"at":1794991374,"by":"worker-a","from":"active","note":"report.v0190-3","to":"ready"},{"at":1794991400,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 137 merged as b97e689 into release-0.19.0, PR checks 20 of 20; all 13 types carry #[non_exhaustive] on the branch. The correction is right: E0639 refuses ..Default::default() outside the crate, so TaskSpec::new and FindingSpec::new plus Default are the way in; accepted. Branch CI for 8a63540 is green; b97e689's runs were still going and get checked at READY. Tally: found.","to":"done"}]}`

```text
13a and findings public types become non_exhaustive

First step: recall (decision.non-exhaustive-0.19). Mark #[non_exhaustive] the public structs in mnemo/src/store/coordination.rs and findings.rs that callers build or match: TaskSpec, Task, TaskTaken, TaskClaim, NewWork, Message, Claim, Presence, Stale, KindCounts, FindingSpec, Finding, FindingFilter (say in the report if one shouldn't be, and why). Where a struct literal was the only way to build one outside the crate, give it Default and/or a constructor so ..Default::default() or new() works. Update the CLI, MCP, Python and any uniffi callers; doc tests compile. CHANGELOG Changed: what struct-literal and exhaustive-match code needs. A PR into release-0.19.0. Report: report.v0190-3.
```

## 01M7PEZDS5V5NTEE4S8ZZMEEBE

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T04:34:55Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-0"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["spec.inspect-date-filter","decision.positioning"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-4","_mnemo.status":"done","_mnemo.status_history":[{"at":1794991408,"by":"worker-a","from":"queued","to":"active"},{"at":1794993004,"by":"worker-a","from":"active","note":"report.v0190-4","to":"ready"},{"at":1794993078,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 138 merged as dabb5fc into release-0.19.0, PR checks 20 of 20. Screenshot viewed: synthetic DEMO ids, role names only, header shows only demo.mnemo, 101,720 bytes, 1200x540. The local branch slip touched only the Worker's own unpushed commit; noted, no harm. v0190-5 opens: README.md and index.html are edited in the working tree (the screenshot in both; the first-search row and known-limitation numbers from v0.19.0; the site's two first-search roadmap lines removed and the shipped list now v0.13 to v0.19). Commit those two paths alone as the task says. Tally: found.","to":"done"}]}`

```text
inspect: a date range filter, and a Findings screenshot from a demo brain

First step: recall. (1) The date filter: spec.inspect-date-filter. (2) A screenshot of the Findings tab for the README and the site, taken from a demo brain built by a small script (synthetic findings and tasks, ids like DEMO-1.1; nothing from the project brain, no real names or paths), light theme, PNG under 300 KB, at docs/img/inspect-findings.png; commit the script beside the fixture tooling so it can be retaken. A PR into release-0.19.0. Report: report.v0190-4. After my review I add the image to README.md and index.html and fix the roadmap's duplicate line; that hand-off is v0190-5.
```

## 01M7PEZDV55PPXYJX38F174YP8

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T04:34:55Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-4"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-5","_mnemo.status":"done","_mnemo.status_history":[{"at":1794993088,"by":"worker-a","from":"queued","to":"active"},{"at":1794993110,"by":"worker-a","from":"active","note":"report.v0190-5","to":"ready"},{"at":1794993122,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: 3ba2f7b on origin/release-0.19.0, README.md and index.html alone, matching the working-tree edits; tree clean. Tally: partial (as reported).","to":"done"}]}`

```text
Hand-off: README.md and index.html (the inspector screenshot, the roadmap's duplicate line)

The Planner's edits to README.md and index.html are in the working tree when v0190-4 is done (this task opens then). Commit those two paths alone on release-0.19.0: 'docs(site): the inspector screenshot; one roadmap line'. Report: report.v0190-5.
```

## 01M7PEZDX45SF50M212BHMN43P

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T04:34:55Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-2","v0190-3","v0190-4","v0190-5"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-6","_mnemo.status":"done","_mnemo.status_history":[{"at":1794993138,"by":"worker-a","from":"queued","to":"active"},{"at":1794993278,"by":"worker-a","from":"active","note":"PR 139 (bump, CHANGELOG, backup) in CI; dry run next","to":"active"},{"at":1794996169,"by":"worker-a","from":"active","note":"report.v0190-6","to":"ready"},{"at":1794996208,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: release-0.19.0 at 9b78312 (PR 139, checks 26 of 26 after the logged macOS shared_valve re-run); origin/main an ancestor; dry run 44278116677 success on 9b78312 with mobile on; push CI and Mobile CI green on 9b78312; docs/brain.md scanned: no home or temp paths, no email, RB-006.12 by id only. Next: REVIEWER-B's READY pass (v0190-review), then my version-text hand-off (v0190-7), then the release (v0190-8). shared_valve flake logged with shared_lock: change only if it recurs. Tally: found.","to":"done"}]}`

```text
v0.19.0 READY: the bump, the CHANGELOG, the backup, the dry run

First step: recall. As RELEASING.md: the bump (mobile by its rule: v0190-3 may change a public type the bindings use), the CHANGELOG's [0.19.0], docs/brain.md exported by the installed release (step 7), the dry run. No chore(plan) commit: the archives are frozen. Report: report.v0190-6, then ready. REVIEWER-B's READY pass follows (I call it).
```

## 01M7Q1T5BDRQMBNYWCKMJSJT9J

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T10:04:05Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-review"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-7","_mnemo.status":"done","_mnemo.status_history":[{"at":1794997235,"by":"worker-a","from":"queued","to":"active"},{"at":1794997254,"by":"worker-a","from":"active","note":"report.v0190-7","to":"ready"},{"at":1794997263,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: b345073 on origin/release-0.19.0, README.md and index.html alone, matching my edits; tree clean. v0190-8 (the release) opens. Tally: found.","to":"done"}]}`

```text
Hand-off: README.md and index.html, the v0.19.0 version text

First step: recall. The Planner's edits to README.md and index.html are in the working tree: the site's pill, the examples line and example string at v0.19.0, 35 tagged releases; the README's example string, mobile versions at 0.19.0, and a 'What's new in v0.19.0' section. Commit those two paths alone on release-0.19.0: 'docs: v0.19.0 version text'. This opens after REVIEWER-B's pass and any fixes it needs. Report: report.v0190-7.
```

## 01M7Q1T5D59TW997D3KVC449H9

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T10:04:05Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-7"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0190-8","_mnemo.status":"done","_mnemo.status_history":[{"at":1794997270,"by":"worker-a","from":"queued","to":"active"},{"at":1794998444,"by":"worker-a","from":"active","note":"report.v0190-8","to":"blocked"},{"at":1795015070,"by":"worker-a","from":"blocked","to":"active"},{"at":1795015070,"by":"worker-a","from":"active","note":"report.v0190-8","to":"ready"},{"at":1795015089,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: run 44284629207 success; crates.io 0.19.0, PyPI 0.19.0 with 6 files, mnemo-swift tag 0.19.0, Maven Central 0.19.0 (repo1 200), GitHub Release 14 assets, the site's pill v0.19.0; release-0.19.0 gone from origin. Tally: found.","to":"done"}]}`

```text
v0.19.0: the release

First step: recall. As RELEASING.md and decision.release-order: fast-forward main to release-0.19.0, the annotated tag v0.19.0, the release run; at the approval stop set this task blocked with report.v0190-8 and send WAITING: v0190-8 (the user clicks). After the run: crates.io, PyPI, mnemo-swift 0.19.0, Maven Central, the GitHub Release's assets, the site's pill; then delete release-0.19.0. Report: report.v0190-8.
```

## 01M7Q1S9G1T05HEZ6KFMSCW4N8

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T10:03:37Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0190-6"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"reviewer-b","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["9fb1191..9b78312","decision.perf-2-design"],"_mnemo.role":"reviewer","_mnemo.seen_by":["reviewer-b"],"_mnemo.slug":"v0190-review","_mnemo.status":"done","_mnemo.status_history":[{"at":1794996284,"by":"reviewer-b","from":"queued","to":"active"},{"at":1794997190,"by":"reviewer-b","from":"active","note":"RB-015 done at 9b78312 (diff 9fb1191..9b78312). No HIGH or MEDIUM; I wouldn't hold the tag. Two LOW findings: RB-015.1, RB-015.2. Recall first: found (report.v0190-2, architecture.page-batch-reads, decision.perf-2-design).\nBaseline on this Mac: cargo test --locked --features failpoints: 778 passed, 0 failed, 1 ignored (the compat tests return early without the old binary).\n(1) index.rs layout 2: holds. Every length is checked before its allocation, the body must end at the run's length, layout 1 is still read, an unknown byte is no index. 300,000 mutated runs: no panic. v9 caps vectors at 4,072 dimensions, so the 65,536 limit can't be reached.\n(2) pager.rs and crypto.rs: holds. The nonce comes from each page, the AAD is page and generation as before, the tag is checked before the plaintext is used. A flipped nonce, ciphertext or tag byte, two pages swapped, a wrong generation: PageAuthFailed on the single, batch and streamed paths, on 1 and 8 threads; 200 runs with three bad pages, no hang. A cut file is an error. A cached dirty page wins. Whole-file: a damaged vector page fails recall and verify by name, indexed and exact.\n(3) The lazy map: every path that reads it builds it first (contains, remove, add through remove, apply_ops); a fresh handle's delete and remember are right after reopening.\n(4) inspect: GET only, the token is checked before the route, a bad since, until or date is a 400 as JSON, no HTML sink in the page.\n(5) The compat test does make v0.18.1 read a layout-2 run. I also ran the published v0.18.1 on a 20,100-note file by hand: holds (RB-015.2 item 2).\n(6) docs/brain.md: written by 0.18.1, 20 tasks, 6 essence items, no findings, no private or security text, no secrets, no local paths.\nRB-014.1 and RB-014.2 (accepted in the archive): confirmed fixed at this head on a scratch brain, with v0.18.1's and v0.17.2's export too.\nNot done: Linux and Windows (the Windows positioned read is by reading), benchmarks, Python, mobile.","to":"ready"},{"at":1794997218,"by":"planner-a","from":"ready","note":"Triaged by planner-a: RB-015.1 and RB-015.2 LOW, both ACCEPTED for v0.19.1 (v0191-1); neither holds the tag. v0190-7 opens.","to":"done"}]}`

```text
REVIEWER-B: v0.19.0 READY pass (batch RB-015)

First step: recall. REVIEWER-B's scheduled READY pass (Reviewer rule 4; this release touches the page crypto path and the index's on-disk layout). Scope, and only this: the diff 9fb1191..9b78312 on release-0.19.0. (1) mnemo/src/index.rs layout 2: decoding an untrusted or damaged base (lengths checked before allocating, trailing bytes, layout 1 still read, unknown layout falls back to exact). (2) mnemo/src/pager.rs read_pages_uncached, RunReader and for_each_page: per-page AES-GCM nonce and generation checks kept, cached dirty pages, the scoped-thread path, errors not swallowed; crypto.rs. (3) store/load.rs and records.rs: the lazy id->partition map before a write. (4) inspect.rs date range: since/until parsing, GET only, no HTML sink. (5) tests/v0_18_compat.rs and the compat CI job really exercise v0.18.1 on a layout-2 file. (6) docs/brain.md: nothing private. Out of scope: benchmarks (the Worker's numbers are in test/scripts/perf_v0.19.0.json), non_exhaustive beyond a glance, docs wording. CARGO_TARGET_DIR=/tmp/rb-target and --locked. Findings as records: mnemo finding add RB-015.<n> (a SECURITY one is private until verified; SECURITY.local.md as usual). Then set this task ready and send REVIEW DONE: RB-015 to planner-a.
```

## 01M7R3H185S8A7F8E7N9PGAZNZ

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T19:53:18Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0191-0","_mnemo.status":"done","_mnemo.status_history":[{"at":1795031611,"by":"worker-a","from":"queued","to":"active"},{"at":1795031740,"by":"worker-a","from":"active","note":"report.v0191-0","to":"ready"},{"at":1795031760,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: release-0.19.1 from b345073; 470b9de is AGENTS.md alone and its coordination section matches the approved text byte for byte; 74e0569 aligns the dogfood section (rule 10, who writes facts, default agent; the remember flags exist). Tree clean. v0191-1 opens. Tally: found.","to":"done"}]}`

```text
v0.19.1 setup: the release branch and the AGENTS.md coordination rewrite (hand-off)

First step: recall (status.run-2). (1) Branch release-0.19.1 from main (b345073). (2) The Planner's rewrite of AGENTS.md's 'Multi-agent roles & coordination' section is in the working tree (the user approved it 2026-11-18: the brain is the permanent way of working). Commit AGENTS.md alone: 'docs(agents): the brain carries the work (Run 2 made permanent)'. (3) Then, as your own separate commit, fix your 'Dogfood the engine on yourself' section to match: 'tmux rule 9' is now 'tmux rule 10'; the Worker writes facts, and the Planner writes the project essence and keyed decision.*, spec.* and status.* facts; since v0.17 the brain's default agent (cursor-agent) gives a keyed write its slot, so a write needn't name the agent; the Reviewers only read. (4) Push. No PR for (2); a PR into release-0.19.1 or a direct commit for (3), your choice. Report: report.v0191-0.
```

## 01M7Q2QVFQJRAD4BE19QZGYEF6

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T10:20:18Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0191-0"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["RB-015.1","RB-015.2"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0191-1","_mnemo.status":"done","_mnemo.status_history":[{"at":1795031764,"by":"worker-a","from":"queued","to":"active"},{"at":1795038499,"by":"worker-a","from":"active","to":"active"},{"at":1795038528,"by":"worker-a","from":"active","note":"report.v0191-1","to":"ready"},{"at":1795038561,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 140 merged as ac8725b into release-0.19.1, PR checks 27 of 27 (two macOS jobs re-run for runner capacity); ensure_ann on the branch matches the triage for damage. One gap left: on a non-damage error (I/O) ensure_ann has already cleared ann_unloaded, so the first recall fails and later ones on that handle recall exactly with no notice, RB-015.1's item 1 for that case. Follow-up v0191-1b (round 1 of 3). Recall partial: findings aren't returned by recall (they're coordination records); noted for the product. Tally: partial.","to":"done"}]}`

```text
v0.19.1: RB-015.1 and RB-015.2 (a damaged index run; layout-2 fuzz target; compat at 20K; tab link)

First step: recall. Do what the triage notes on RB-015.1 and RB-015.2 say (mnemo finding list). A PR into release-0.19.1. Report: report.v0191-1.
```

## 01M7RA5H1MK3T875PVXTTB09R6

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T21:49:21Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0191-1"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["RB-015.1"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0191-1b","_mnemo.status":"done","_mnemo.status_history":[{"at":1795038565,"by":"worker-a","from":"queued","to":"active"},{"at":1795044533,"by":"worker-a","from":"active","to":"active"},{"at":1795046278,"by":"worker-a","from":"active","note":"report.v0191-1b","to":"ready"},{"at":1795046305,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 141 merged as 5503e3c into release-0.19.1, PR checks 21 of 21 after the deprecation fix (016797f); ensure_ann sets ann_unloaded back on a non-damage error; the read failpoint is a no-op outside test or the failpoints feature. RB-015.1 is complete for v0.19.1. fact.ci-toolchain-gap noted (CI stable ahead of the local rustc). Tally: found.","to":"done"}]}`

```text
v0.19.1: ensure_ann retries after a non-damage error

First step: recall. In mnemo/src/store/ann.rs ensure_ann (release-0.19.1, ac8725b): when load_persisted_index fails with an error that isn't index damage (an I/O error), set ann_unloaded back to true before returning it, so the next call on that handle tries the load again instead of recalling exactly with no notice. A test with the failpoints feature: an I/O failure on the first recall, then a recall that loads the index (indexed, not exact). CHANGELOG Fixed, one line. A PR into release-0.19.1. Report: report.v0191-1b.
```

## 01M7R3P6TKR1P1FMQS6DRYJ21F

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T19:56:07Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0191-1b"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0191-2","_mnemo.status":"done","_mnemo.status_history":[{"at":1795046312,"by":"worker-a","from":"queued","to":"active"},{"at":1795047371,"by":"worker-a","from":"active","to":"active"},{"at":1795048409,"by":"worker-a","from":"active","note":"report.v0191-2","to":"ready"},{"at":1795048442,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: release-0.19.1 at 6838c69 (PR 142, 27 of 27), origin/main an ancestor; dry run 44427600695 success with mobile off (agreed: no mobile code changed, 0.19.0 already published mobile for this minor); push CI and Mobile CI green; docs/brain.md (c2141db) clean, the one path-like match is prose. The backup by 0.18.1 is accepted (no export filter changed). Version text edited in the working tree; hand-off v0191-2h. Tally: found.","to":"done"}]}`

```text
v0.19.1 READY: the bump, the CHANGELOG, the backup, the dry run

First step: recall. As RELEASING.md: the bump (mobile by its rule for a patch), the CHANGELOG's [0.19.1], docs/brain.md exported by the installed release (0.19.0) and committed by path, the dry run. In the report, list the version text the Planner must hand off (README.md, index.html). Report: report.v0191-2. A patch: no Reviewer pass.
```

## 01M7RKK357FSBV466A97C2QT79

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T00:34:03Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0191-2"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0191-2h","_mnemo.status":"done","_mnemo.status_history":[{"at":1795048446,"by":"worker-a","from":"queued","to":"active"},{"at":1795048466,"by":"worker-a","from":"active","note":"report.v0191-2h","to":"ready"},{"at":1795048478,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: 2d21ac6 on origin/release-0.19.1, README.md and index.html alone, matching my edits; tree clean. v0191-3 opens. Tally: per report.","to":"done"}]}`

```text
Hand-off: README.md and index.html, the v0.19.1 version text

First step: recall. The Planner's edits to README.md and index.html are in the working tree: the site's pill, examples line and example string at v0.19.1, 36 tagged releases; the README's example string and a 'What's new in v0.19.1' section. Mobile lines stay at 0.19.0. Commit those two paths alone on release-0.19.1: 'docs: v0.19.1 version text'. Report: report.v0191-2h.
```

## 01M7R3P6WQ3PBW8PT1P2QAJHM1

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T19:56:07Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0191-2h"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0191-3","_mnemo.status":"done","_mnemo.status_history":[{"at":1795048482,"by":"worker-a","from":"queued","to":"active"},{"at":1795049434,"by":"worker-a","from":"active","note":"report.v0191-3","to":"blocked"},{"at":1795053571,"by":"planner-a","from":"blocked","note":"planner-a: the user reports the release approved in planner-a at 2026-11-19T01:59Z; check run 44430183319 yourself before going on.","to":"blocked"},{"at":1795053588,"by":"worker-a","from":"blocked","to":"active"},{"at":1795053723,"by":"worker-a","from":"active","note":"report.v0191-3","to":"ready"},{"at":1795053750,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: run 44430183319 success; crates.io 0.19.1, PyPI 0.19.1 with 6 files, GitHub Release 12 assets (not draft); mnemo-swift stays at 0.19.0 as intended; the site's pill v0.19.1; release-0.19.1 gone. The approval relay (task note plus READ pointer) was used. Tally: found.","to":"done"}]}`

```text
v0.19.1: the release

First step: recall. As RELEASING.md and decision.release-order: fast-forward main, the annotated tag v0.19.1, the release run. At the approval stop: set this task blocked with report.v0191-3, send WAITING: v0191-3, and watch the run in the background (gh run watch) so the click itself wakes you; the Planner also sends READ: v0191-3 when the user reports it approved. Check the run yourself before going on. After it: the registries, the GitHub Release, the site; delete release-0.19.1. Report: report.v0191-3.
```

## 01M7RVPRJRJ4DXXBY7AHMCD7VQ

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T02:55:51Z
- importance: 0.5
- metadata: `{"_mnemo.after":[],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-0","_mnemo.status":"done","_mnemo.status_history":[{"at":1795056964,"by":"worker-a","from":"queued","to":"active"},{"at":1795056988,"by":"worker-a","from":"active","note":"report.v0200-0","to":"ready"},{"at":1795057002,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: origin release-0.20.0 at 2d21ac6, the same as main. Tally: found.","to":"done"}]}`

```text
v0.20.0 setup: the release branch

First step: recall (spec.v0.20.0). Branch release-0.20.0 from main (2d21ac6) and push. Report: report.v0200-0.
```

## 01M7RVPRMTRRGH734PJSY2EA2Y

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T02:55:51Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-0"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["spec.v0.20.0"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-1","_mnemo.status":"done","_mnemo.status_history":[{"at":1795057006,"by":"worker-a","from":"queued","to":"active"},{"at":1795058182,"by":"worker-a","from":"active","to":"active"},{"at":1795062256,"by":"worker-a","from":"active","to":"active"},{"at":1795066564,"by":"worker-a","from":"active","note":"report.v0200-1","to":"ready"},{"at":1795066614,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 143 merged into release-0.20.0 as f2898a9 (11 files, matching the report); 21 of 21 checks SUCCESS; flush.rs keeps wal::commit's own fsync after stage_commit's barrier, and restores (header, free, free_dirty) on a stage error; pager.rs has sync/unsynced. Batch fsyncs 4->3 at 1K and 100K, 3->3 at 10K, per report. fact.post-commit-retry-bug triaged into v0200-1r. Tree clean apart from the Planner's sync.md.","to":"done"}]}`

```text
v0.20.0: the fsync merge (PERF-2 3f)

First step: recall. Build part (A) of spec.v0.20.0, as docs/design/perf-2.md section 3f describes, with its crash tests and the measured fsync counts before and after. A PR into release-0.20.0. Report: report.v0200-1.
```

## 01M7S2MWHQJ0D6YWRQCXTJX4V6

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T04:57:10Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-1"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-1h","_mnemo.status":"done","_mnemo.status_history":[{"at":1795066661,"by":"worker-a","from":"queued","to":"active"},{"at":1795066690,"by":"worker-a","from":"active","note":"report.v0200-1h","to":"ready"},{"at":1795066710,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: b4d81d9 on origin/release-0.20.0, docs/design/sync.md alone (244 lines), identical to my working copy; tree clean. Next: v0200-1r.","to":"done"}]}`

```text
Hand-off: docs/design/sync.md, the encrypted sync design note

First step: recall. The Planner's design note docs/design/sync.md is untracked in the working tree; the user approved it 2026-11-18. Once v0200-1 is merged, commit that one path alone on release-0.20.0, as its own commit: 'docs(design): the encrypted sync design note'. Don't edit it; ask in the report for any change. Report: report.v0200-1h.
```

## 01M7S4XMQY1GE2KQFJTJRB6SYW

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T05:36:54Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-1h"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["fact.post-commit-retry-bug"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-1r","_mnemo.status":"done","_mnemo.status_history":[{"at":1795066727,"by":"worker-a","from":"queued","to":"active"},{"at":1795071309,"by":"worker-a","from":"active","to":"active"},{"at":1795072546,"by":"worker-a","from":"active","note":"report.v0200-1r","to":"ready"},{"at":1795072574,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 144 merged into release-0.20.0 as 3a295ed (8 files, matching the report); 21 of 21 checks SUCCESS; flush.rs has commit_failed, the MaybeStored refusal and commit_and_checkpoint. Refusing until reopened (not reloading) accepted: the commit may already be durable. fsyncs stay 3/3. The Worker's same-generation page-reuse question goes to REVIEWER-B's v0.20.0 scope (status.v0200-review-scope).","to":"done"}]}`

```text
v0.20.0: a flush retried after a failed commit write leaves a sound file

First step: recall. Fix fact.post-commit-retry-bug (pre-existing; found in v0200-1): on an exclusive handle, a flush that fails at the WAL commit write (write 2) or the first checkpoint page write (write 3), retried on the same handle, returns Ok, but the reopen fails PageAuthFailed. Goal: after any failed write or sync anywhere in a flush, a retry on the same handle either commits soundly or returns an error, and the file always reopens with the pre or post state. Find the cause first (the fact suspects step 3e's in-place header.wal_seq and run-pointer stamps, which the restore doesn't cover after the commit point). One acceptable shape: after a failure at or past the commit point, the handle reloads from disk as the shared window does, or refuses further writes until reopened; say which and why. Turn the fact's temporary test into a permanent one over every write and sync index of the flush, with the retry, and extend prop_big_batch_commits_crash_pre_or_post to retry on the same handle. Keep v0200-1's fsync counts (measure them). No format change. Own PR into release-0.20.0; the CHANGELOG under Fixed. Report: report.v0200-1r.
```

## 01M7RVPRPYMNSQFEE4CM9VYX1P

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T02:55:52Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-0"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["spec.v0.20.0"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-2","_mnemo.status":"done","_mnemo.status_history":[{"at":1795066633,"by":"worker-a","from":"queued","to":"active"},{"at":1795066647,"by":"worker-a","from":"active","note":"worker-a: taken by task next by mistake while picking up v0200-1h; put back, untouched","to":"queued"},{"at":1795066661,"by":"worker-a","from":"queued","to":"active"},{"at":1795066666,"by":"worker-a","from":"active","note":"worker-a: task next has no slug option and gives the oldest task first, so it was taken only to reach v0200-1h; put back, untouched, until the Planner's pointer","to":"queued"},{"at":1795066727,"by":"worker-a","from":"queued","to":"active"},{"at":1795066733,"by":"worker-a","from":"active","note":"worker-a: task next has no slug option, so it was taken only to reach v0200-1r (the Planner's pointer); put back, untouched","to":"queued"},{"at":1795072582,"by":"worker-a","from":"queued","to":"active"},{"at":1795073634,"by":"worker-a","from":"active","note":"report.v0200-2","to":"ready"},{"at":1795073682,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 145 (da96a2a, docs only: catch-up.md plus one pointer line in 12.4) read in full; baseline two clean runs, 10K 3.6/30.8 ms catch-up; gen-walk rule checked against records.rs write_gen/ensure_gen_lease. Approved with additions in decision.catch-up-design; step 8: no gate. PR 145 merges as v0200-3's first step.","to":"done"}]}`

```text
v0.20.0: incremental catch-up, the design note and baseline

First step: recall. For part (B) step 6 of spec.v0.20.0: measure today's catch-up after another process's commit at 10K and 100K on v0.19.1 (per call, first recall after it, RSS), then a short design addendum in docs/design/12.4-shared-access.md (or a new docs/design/catch-up.md): how a handle finds the deltas newer than its transaction, the page cache rule, the vector column, the fallback, the tests, the targets. Also step 8: whether to gate older binaries, with a recommendation (a format change goes to the user). A PR into release-0.20.0 with the note only. Report: report.v0200-2. I review before the build.
```

## 01M7RVPRRZZH769RXZMMSV6KFJ

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T02:55:52Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-1","v0200-2"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["spec.v0.20.0"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-3","_mnemo.status":"done","_mnemo.status_history":[{"at":1795066614,"by":"planner-a","from":"queued","note":"planner-a 2026-11-18: also wait for v0200-1r (flush retry fix) before the catch-up build, since both touch flush.rs.","to":"queued"},{"at":1795073682,"by":"planner-a","from":"queued","note":"planner-a 2026-11-18: build per docs/design/catch-up.md and decision.catch-up-design. First: add that fact's additions (a)-(d) to the note in PR 145 and merge it. v0200-1 and v0200-1r are done.","to":"queued"},{"at":1795073691,"by":"worker-a","from":"queued","to":"active"},{"at":1795080393,"by":"worker-a","from":"active","to":"active"},{"at":1795083678,"by":"worker-a","from":"active","to":"active"},{"at":1795087036,"by":"worker-a","from":"active","to":"active"},{"at":1795087333,"by":"worker-a","from":"active","to":"active"},{"at":1795087684,"by":"worker-a","from":"active","to":"active"},{"at":1795088183,"by":"worker-a","from":"active","to":"active"},{"at":1795088850,"by":"worker-a","from":"active","to":"active"},{"at":1795089435,"by":"worker-a","from":"active","to":"active"},{"at":1795090169,"by":"worker-a","from":"active","note":"report.v0200-3","to":"ready"},{"at":1795090251,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 146 merged into release-0.20.0 as ed4e5b1; 21 of 21 checks SUCCESS; shared.rs 621-654 tries catch_up_to on same identity + valid seal, else full reload with adopt_column when trusted. Catch-up 3.6->0.4 ms (10K), 30.8->0.8 ms (100K): met. Accepted misses, per decision.catch-up-results: direct recall 8.2 ms at 100K (was 64.6; the 20 ms-sleep control shows the machine), fallback +10 ms once per ~64 commits, footprint about +100 MB over 700 commits at 100K (was +1.3 GB). The restore-in-place edge in 8a goes to v0200-3r before step 8.","to":"done"}]}`

```text
v0.20.0: incremental catch-up, the build

First step: recall. Build step 6 as the reviewed note and my done note on v0200-2 say. A PR into release-0.20.0 with measured numbers. Report: report.v0200-3.
```

## 01M7SVF03PBD2GP49GGGNCSGE9

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T12:10:51Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-3"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-3r","_mnemo.status":"done","_mnemo.status_history":[{"at":1795090311,"by":"worker-a","from":"queued","to":"active"},{"at":1795092831,"by":"worker-a","from":"active","to":"active"},{"at":1795093517,"by":"worker-a","from":"active","to":"active"},{"at":1795097253,"by":"worker-a","from":"active","to":"active"},{"at":1795098168,"by":"worker-a","from":"active","note":"report.v0200-3r","to":"ready"},{"at":1795098192,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 147 merged into release-0.20.0 as b2a2dc0; 21 of 21 checks SUCCESS; shared.rs anchors_hold (289, used at 753) and cache.rs drop_newer_than (255, with a unit test). Deviations accepted: every kept run is checked (the cache-put case), superseded runs skipped (legitimate reuse), and newer clean pages are dropped. Catch-up stays at 0.8 ms at 100K. The superseded-run skip is on REVIEWER-B's scope item 5. Tip: after a READ pointer, take that slug directly rather than task next.","to":"done"}]}`

```text
v0.20.0: catch-up never trusts held state across a copy restored in place

First step: recall. decision.catch-up-results. The edge in catch-up.md 8a: an older copy of the same brain copied over the file in place (cp keeps the inode; same key), then committed past the reader's transaction, passes the identity and seal checks (shared.rs 621, 647). After a restore, the free list hands out the same pages again, and the generations repeat because they follow wal_seq. So a pointer the reader holds can match exactly. Then catch_up_to's walk, the (page, generation) page cache and adopt_column could keep bytes from the timeline that was overwritten. Close it without a format change. Suggested shape (say if the code says otherwise): (1) a catch-up or a trusted reload needs H_n.wal_seq >= H_t.wal_seq, else a full reload with no adopt_column; (2) the handle keeps the on-disk nonce (or tag) of the first page of H_t's catalog delta head (or base when there's no delta); before trusting held state, read that page's raw bytes uncached and compare. Per-session random nonce prefixes (SEC-1) mean a rewrite in another timeline never has the same nonce. A mismatch means a full reload, a fresh page cache and no adopt_column. One 4 KiB read per catch-up; measure it against the 1 ms target. Tests in tests/catch_up.rs: a reader at t; the file overwritten in place with a copy taken at s < t; commits past t by another handle with fixed-size rows so that the pointers coincide (assert they do, so the test really bites); the reader must equal a fresh open and take the full path; the same for adopt_column. Show the test fails without the fix. Update 8a. Own PR into release-0.20.0. Report: report.v0200-3r.
```

## 01M7RVPRV2KVX5K364A75CE4Z7

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T02:55:52Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-3"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.refs":["spec.v0.20.0"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-4","_mnemo.status":"done","_mnemo.status_history":[{"at":1795090251,"by":"planner-a","from":"queued","note":"planner-a 2026-11-19: wait for v0200-3r. Also add the release-notes line from catch-up.md section 9: update every mnemo on the machine before relying on shared mode.","to":"queued"},{"at":1795090270,"by":"worker-a","from":"queued","to":"active"},{"at":1795090280,"by":"worker-a","from":"active","note":"worker-a: taken by mistake with task next while picking up v0200-3r (the READ pointer); released untouched, no work done","to":"queued"},{"at":1795090311,"by":"worker-a","from":"queued","to":"active"},{"at":1795090311,"by":"worker-a","from":"active","note":"worker-a: taken by mistake with task next while picking up v0200-3r (the READ pointer; next serves the oldest first); released untouched, no work done","to":"queued"},{"at":1795098204,"by":"worker-a","from":"queued","to":"active"},{"at":1795098983,"by":"worker-a","from":"active","to":"active"},{"at":1795104040,"by":"worker-a","from":"active","note":"report.v0200-4","to":"ready"},{"at":1795104065,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 148 merged into release-0.20.0 as 54afc04; 21 of 21 checks SUCCESS; mnemo.rs has the exclusive flag (1324); CHANGELOG [Unreleased] carries 'Update every mnemo on the machine before you rely on shared mode'. No gate, no format change, per decision.catch-up-design. rekey_cut.rs's ungated failpoints use goes into v0200-5; stats counting presence goes to the smarter-memory backlog.","to":"done"}]}`

```text
v0.20.0: shared by default for mnemo serve --mcp

First step: recall. Step 8 of spec.v0.20.0 as decided on v0200-2: a bare serve --mcp is --shared, --exclusive keeps the lifetime lock; the guide, the CLI help, mnemo/README.md and CHANGELOG. A PR into release-0.20.0. Report: report.v0200-4.
```

## 01M7RVPRX3DVJYPPZZ96YAE78E

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T02:55:52Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-4"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-5","_mnemo.status":"active","_mnemo.status_history":[{"at":1795065822,"by":"planner-a","from":"queued","note":"planner-a 2026-11-18: READY also waits for v0200-1h, v0200-d1, v0200-d2 and v0200-d3 (user go on discoverability).","to":"queued"},{"at":1795066183,"by":"planner-a","from":"queued","note":"planner-a 2026-11-18: READY also waits for v0200-d4 (the demo GIF).","to":"queued"},{"at":1795104065,"by":"planner-a","from":"queued","note":"planner-a 2026-11-19: also gate tests/rekey_cut.rs on the failpoints feature (or cfg) so AGENTS.md's plain 'cargo clippy --all-targets -- -D warnings' compiles (found in v0200-4); include that build in the READY gates.","to":"queued"},{"at":1795104074,"by":"worker-a","from":"queued","to":"active"},{"at":1795104081,"by":"worker-a","from":"active","note":"worker-a: served by task next (oldest first) while picking up v0200-d1 (the READ pointer); released untouched, no work done","to":"queued"},{"at":1795104086,"by":"worker-a","from":"queued","to":"active"},{"at":1795104091,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-d1 with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795104303,"by":"worker-a","from":"queued","to":"active"},{"at":1795104307,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-d2 with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795106109,"by":"worker-a","from":"queued","to":"active"},{"at":1795106113,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-d3 with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795108104,"by":"planner-a","from":"queued","note":"planner-a 2026-11-19: also wait for v0200-d3h. The bump includes the root server.json version and bundle URL (report.v0200-d3).","to":"queued"},{"at":1795108113,"by":"worker-a","from":"queued","to":"active"},{"at":1795108113,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-d3h with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795108119,"by":"worker-a","from":"queued","to":"active"},{"at":1795108123,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-d3h with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795112452,"by":"planner-a","from":"queued","note":"planner-a 2026-11-19: also wait for v0200-t1 (the shared_valve test).","to":"queued"},{"at":1795112462,"by":"worker-a","from":"queued","to":"active"},{"at":1795112462,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-d4 with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795115447,"by":"worker-a","from":"queued","to":"active"},{"at":1795115447,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-t1 with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795115447,"by":"worker-a","from":"queued","to":"active"},{"at":1795115447,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-t1 with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795115447,"by":"worker-a","from":"queued","to":"active"},{"at":1795115447,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-t1 with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795115452,"by":"worker-a","from":"queued","to":"active"},{"at":1795115452,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-t1 with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795118306,"by":"worker-a","from":"queued","to":"active"},{"at":1795124946,"by":"worker-a","from":"active","note":"worker-a: renewing the claim; suite done, committing next","to":"active"}]}`

```text
v0.20.0 READY: the bump, the CHANGELOG, the backup, the dry run

First step: recall. As RELEASING.md: the bump (mobile by its rule: every minor publishes mobile once), the CHANGELOG's [0.20.0], docs/brain.md exported by the installed release and committed by path, the dry run. List the version text for my hand-off. Report: report.v0200-5. REVIEWER-B's READY pass follows (I call it).
```

## 01M7S45FHE7D9B8QJQ9GQEJNSW

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T05:23:42Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-1h"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-d1","_mnemo.status":"done","_mnemo.status_history":[{"at":1795066727,"by":"worker-a","from":"queued","to":"active"},{"at":1795066733,"by":"worker-a","from":"active","note":"worker-a: task next has no slug option, so it was taken only to reach v0200-1r (the Planner's pointer); put back, untouched","to":"queued"},{"at":1795090280,"by":"worker-a","from":"queued","to":"active"},{"at":1795090287,"by":"worker-a","from":"active","note":"worker-a: taken by mistake with task next while picking up v0200-3r (the READ pointer); released untouched, no work done","to":"queued"},{"at":1795090311,"by":"worker-a","from":"queued","to":"active"},{"at":1795090311,"by":"worker-a","from":"active","note":"worker-a: taken by mistake with task next while picking up v0200-3r (the READ pointer; next serves the oldest first); released untouched, no work done","to":"queued"},{"at":1795104086,"by":"worker-a","from":"queued","to":"active"},{"at":1795104274,"by":"worker-a","from":"active","note":"report.v0200-d1 (NEEDS-USER: upload the preview in repo Settings)","to":"ready"},{"at":1795104295,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 149 merged into release-0.20.0 as da0a234 (the PNG and its Pillow script only; docs-only, so no CI by design). The PNG is 1280x640 RGB; I looked at it: name, tagline, mnemo onboard, the three layers, no banned words. NEEDS-USER: the upload in Settings, relayed to the user.","to":"done"}]}`

```text
v0.20.0: the GitHub social preview image

First step: recall. User go 2026-11-18 (decision.discoverability). Make a 1280x640 PNG at docs/img/social-preview.png (under 1 MB), with its source beside it (docs/img/social-preview.svg, or a script that renders it). Content: the name 'MNemo', the line 'Shared memory for your coding agents, in one encrypted file.', and a small hint of the layers or the inspector; dark background, everything that matters inside a 40 px margin, legible at 600 px wide. Never the words orchestration, orchestrator or swarm. Commit it in its own PR into release-0.20.0. Uploading it in the repo's Settings is the user's step: say so in the report as NEEDS-USER. Report: report.v0200-d1.
```

## 01M7S45FK9XJGAVHJB22QX3FH4

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T05:23:42Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-1h"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-d2","_mnemo.status":"done","_mnemo.status_history":[{"at":1795066727,"by":"worker-a","from":"queued","to":"active"},{"at":1795066733,"by":"worker-a","from":"active","note":"worker-a: task next has no slug option, so it was taken only to reach v0200-1r (the Planner's pointer); put back, untouched","to":"queued"},{"at":1795090311,"by":"worker-a","from":"queued","to":"active"},{"at":1795090311,"by":"worker-a","from":"active","note":"worker-a: taken by mistake with task next while picking up v0200-3r (the READ pointer; next serves the oldest first); released untouched, no work done","to":"queued"},{"at":1795104303,"by":"worker-a","from":"queued","to":"active"},{"at":1795106087,"by":"worker-a","from":"active","note":"report.v0200-d2","to":"ready"},{"at":1795106104,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 150 merged into release-0.20.0 as 402e183; 27 of 27 checks SUCCESS; Cargo.toml and pyproject.toml descriptions and 5 keywords as reported. Kotlin POM left as an app-developer description: accepted.","to":"done"}]}`

```text
v0.20.0: crates.io and PyPI descriptions and keywords

First step: recall. User go 2026-11-18 (decision.discoverability). mnemo/Cargo.toml: description in plain words leading with 'Shared memory for your coding agents, in one encrypted file' (then what it is: an embedded Rust engine with recall, an MCP server and a CLI); keywords, at most 5, for how people search (suggested: mcp, agent-memory, encryption, vector-search, embedded-database; check crates.io's rules); categories may add development-tools (5 at most). mnemo-python/pyproject.toml: a matching description, keywords, more classifiers (Topic, Development Status, the OS list), and project.urls for Documentation, Changelog and Issues. The uniffi, Swift and Kotlin package descriptions too, if they have one. Never orchestration, orchestrator or swarm. These go live with the v0.20.0 publish. Own PR into release-0.20.0. Report: report.v0200-d2.
```

## 01M7S45FN9K6R827C26NKW2NBM

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T05:23:42Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-1h"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-d3","_mnemo.status":"done","_mnemo.status_history":[{"at":1795066727,"by":"worker-a","from":"queued","to":"active"},{"at":1795066733,"by":"worker-a","from":"active","note":"worker-a: task next has no slug option, so it was taken only to reach v0200-1r (the Planner's pointer); put back, untouched","to":"queued"},{"at":1795090311,"by":"worker-a","from":"queued","to":"active"},{"at":1795090311,"by":"worker-a","from":"active","note":"worker-a: taken by mistake with task next while picking up v0200-3r (the READ pointer; next serves the oldest first); released untouched, no work done","to":"queued"},{"at":1795106109,"by":"worker-a","from":"queued","to":"active"},{"at":1795106596,"by":"worker-a","from":"active","to":"active"},{"at":1795108053,"by":"worker-a","from":"active","note":"report.v0200-d3 (NEEDS-USER after the tag: mcp-publisher login and publish, RELEASING.md)","to":"ready"},{"at":1795108099,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 151 merged into release-0.20.0 as 45d811e (10 files); 27 of 27 checks SUCCESS; dry run 44569170984: the bundle job, SHA256SUMS, artifact checks and attach succeeded; PyPI and crates.io failed only on environment rules for a feature branch (v0200-5's dry run covers them). server.json template at 0.19.1 with no hash, mcpb type. approve-release and attach wait for mcp-bundle. Follow-up v0200-d3h: the bundle job's validator runs inside the job that builds the asset, and the .mcpb isn't attested (attest covers cli/mnemo-v* only).","to":"done"}]}`

```text
v0.20.0: a listing in the official MCP registry

First step: recall. User go 2026-11-18 (decision.discoverability). Read the official MCP registry's current docs (registry.modelcontextprotocol.io and the modelcontextprotocol/registry repo) and write server.json at the repo root for the name io.github.toarchkumar/mnemo, whose server runs 'mnemo serve --mcp --shared'. Note: the PyPI wheel ships only the Python bindings, not the CLI, so a pypi package entry won't run the server. Pick the package type the registry supports that fits the release's own binaries (likely an MCP bundle on the GitHub Release, with its SHA-256), and say why in the report. If that needs release.yml to build a bundle, keep the change small and add it to the dry run; REVIEWER-B's v0.20.0 pass will cover it. Add any ownership marker the registry asks for. Don't publish: logging in with mcp-publisher and publishing after the tag are the user's steps, written in RELEASING.md and the report as NEEDS-USER. Own PR into release-0.20.0. Report: report.v0200-d3.
```

## 01M7TCFNWYJMBHXPEEWWSR8V3Q

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T17:08:19Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-d3"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-d3h","_mnemo.status":"done","_mnemo.status_history":[{"at":1795108119,"by":"worker-a","from":"queued","to":"active"},{"at":1795112424,"by":"worker-a","from":"active","note":"report.v0200-d3h (dry run 44583368965 green; shared_valve flake recurred, for the Planner)","to":"ready"},{"at":1795112452,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 152 merged into release-0.20.0 as 75eeca2; release-branch dry run 44583368965 success at 75eeca2 (publish jobs included); mcp-bundle-check is its own job with contents: read; attest's subject-path lists cli/mnemo-v*, mcp/mnemo-v*.mcpb, mcp/server.json. The shared_valve flake's second occurrence goes to v0200-t1.","to":"done"}]}`

```text
v0.20.0: the MCP bundle is checked and attested like the binaries

First step: recall. Two small changes to release.yml's mcp-bundle job from v0200-d3 (45d811e). (1) Keep the validators away from the asset they check: build the bundle and server.json and upload them as the artifact first, then run 'npx @anthropic-ai/mcpb validate' and 'mcp-publisher validate' in a separate job (or after the upload, on a copy), and have attach check that the bundle's SHA-256 still matches the one build.sh wrote into server.json. The npx call resolves its dependencies at run time, so nothing it runs should be able to touch the file we ship. (2) Attest the .mcpb (and server.json) in the attest job, beside cli/mnemo-v*, and add 'gh attestation verify' for the bundle to RELEASING.md's after-tag steps. Keep the dry run passing; show the run id. Own PR into release-0.20.0. Report: report.v0200-d3h.
```

## 01M7S4GFJSA0QS4DWNHC7XMKA5

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T05:29:43Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-1h"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-d4","_mnemo.status":"done","_mnemo.status_history":[{"at":1795066727,"by":"worker-a","from":"queued","to":"active"},{"at":1795066733,"by":"worker-a","from":"active","note":"worker-a: task next has no slug option, so it was taken only to reach v0200-1r (the Planner's pointer); put back, untouched","to":"queued"},{"at":1795090311,"by":"worker-a","from":"queued","to":"active"},{"at":1795090311,"by":"worker-a","from":"active","note":"worker-a: taken by mistake with task next while picking up v0200-3r (the READ pointer; next serves the oldest first); released untouched, no work done","to":"queued"},{"at":1795108113,"by":"worker-a","from":"queued","to":"active"},{"at":1795108123,"by":"worker-a","from":"active","note":"worker-a: held only to reach v0200-d3h with task next (oldest first); released untouched, no work done","to":"queued"},{"at":1795112462,"by":"worker-a","from":"queued","to":"active"},{"at":1795115402,"by":"worker-a","from":"active","note":"report.v0200-d4","to":"ready"},{"at":1795115436,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 153 merged into release-0.20.0 as 52832a5; 28 of 28 checks SUCCESS; demo.yml has contents: read; the GIF is 389 KB, 809 frames; I looked at the last frame: remember (added), recall (score 0.921), the refine (superseded), history, about (encrypted), grep finds 0. Re-record at v0200-5 per RELEASING.md's new step. The Planner places it in README.md and index.html at the version-text hand-off.","to":"done"}]}`

```text
v0.20.0: the terminal demo GIF

First step: recall. User go 2026-11-18 (decision.discoverability, option 1). A scripted, re-recordable terminal demo, about 30 seconds: one agent saves a decision with a key, a second agent (labelled as a different tool, e.g. '# in Cursor' after '# in Claude Code') recalls it from the same encrypted file, then one more step that shows what's inside (mnemo about, or the essence render). Every command really runs against a throwaway brain in a temp dir, with a test passphrase from the environment or a dev key store; never the project brain or the real keychain; outputs are real, never typed in by hand. Use VHS (charmbracelet/vhs) or an equivalent: commit the script (docs/demo/demo.tape plus any setup script) and the output docs/img/demo.gif (aim under 3 MB, legible at 800 px wide). Add the re-record step to RELEASING.md's checklist. Never the words orchestration, orchestrator or swarm. The Planner places it in README.md and index.html at the version-text hand-off; don't edit those. Own PR into release-0.20.0. Report: report.v0200-d4.
```

## 01M7TGMGP5WM4PJFFKSZMRNCSX

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T18:20:52Z
- importance: 0.5
- metadata: `{"_mnemo.after":["v0200-d3h"],"_mnemo.author":"planner-a","_mnemo.creator":"planner-a","_mnemo.delivery":"sent","_mnemo.kind":"task","_mnemo.owner":"worker-a","_mnemo.pre_approved":false,"_mnemo.ready_seen_by":["planner-a"],"_mnemo.role":"worker","_mnemo.seen_by":["worker-a"],"_mnemo.slug":"v0200-t1","_mnemo.status":"done","_mnemo.status_history":[{"at":1795115452,"by":"worker-a","from":"queued","to":"active"},{"at":1795115739,"by":"worker-a","from":"active","note":"report.v0200-t1: the spec's failure rules conflict with the lock design (measured); proposal inside","to":"blocked"},{"at":1795115754,"by":"planner-a","from":"blocked","note":"planner-a 2026-11-19, DECISION (round 1 of 3): my spec's two rules are withdrawn; report.v0200-t1's measurements show the cause is the documented reader deferral (up to VALVE_AFTER = 500 ms), not a slow write window. Build the Worker's proposal: per read, the idle wait (its wait minus the time write windows held the lock during it); fail when an idle wait exceeds VALVE_AFTER + 500 ms slack; keep 'every writer commits' and a total read-wait cap; drop the fixed 500 ms buckets. Prove it bites with the local lock.rs mutation (readers always defer to the valve), not committed; record that run in the report. No locking change. Own PR into release-0.20.0.","to":"blocked"},{"at":1795116077,"by":"worker-a","from":"blocked","note":"report.v0200-t1 (round 1): built; mutation A doesn't starve, mutation B starves but passes idle-wait; proposal: add reads >= windows","to":"blocked"},{"at":1795116091,"by":"planner-a","from":"blocked","note":"planner-a 2026-11-19, DECISION (round 2 of 3): accepted as proposed. Keep the idle-wait check (VALVE_AFTER + 500 ms) and add the throughput check: reads >= write windows over the run (healthy about 30x, CI's failing PR 152 run about 6x, mutation B 0.15-0.21x). The proof is mutation B (a reader drops the intent lock when the main lock is busy), run locally, recorded in the report, not committed; mutation A is dropped. No locking change. Commit what's built, own PR into release-0.20.0.","to":"blocked"},{"at":1795116102,"by":"worker-a","from":"blocked","to":"active"},{"at":1795118230,"by":"worker-a","from":"active","note":"report.v0200-t1","to":"ready"},{"at":1795118248,"by":"planner-a","from":"ready","note":"VERIFIED by planner-a: PR 154 merged into release-0.20.0 as c0c66bd (lib.rs, lock.rs, shared/mod.rs, shared_valve.rs); 21 of 21 checks SUCCESS; lock.rs's only change is VALVE_AFTER becoming pub(crate). The proof (mutation B) failed 3 of 3, as recorded. The new Windows timing failure is logged as fact.flake-cli-shared-embed; first occurrence, acted on only if it recurs.","to":"done"}]}`

```text
v0.20.0: shared_valve tells a slow write window from starved readers

First step: recall. shared_valve::two_writers_and_four_readers_all_get_their_turns failed a second time (PR 152's first attempt, job 135737720437, macos-latest: one 500 ms window with 0 reads, longest read wait 688.6 ms; the first was PR 139, record 01M7Q1R5). Neither PR changed the locking. Likely cause, to check: one write window on a slow CI disk held the lock longer than the 500 ms sample, so readers waited behind it, which is the design, not starvation. Make the test measure what it means: record each write window's held time and each read's wait. Fail when a read waits much longer than the longest write window held while it waited (say more than that plus 250 ms), or when readers get no turn across two writer windows in a row. Don't fail just because a fixed 500 ms bucket was empty. Keep a check that catches real starvation: show that the test fails with readers-first turned off, or with a writer that loops without yielding. No change to the locking itself unless the numbers show real starvation; if they do, stop and report. Own PR into release-0.20.0. Report: report.v0200-t1.
```

## 01M7Q2PG6MWEMS2V21TFFSKKJF

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T10:19:34Z
- importance: 0.5
- metadata: `{"_mnemo.author":"reviewer-b","_mnemo.batch":"RB-015","_mnemo.category":"CORRECTNESS","_mnemo.finding_id":"RB-015.1","_mnemo.kind":"finding","_mnemo.priority":"LOW","_mnemo.refs":["mnemo/src/store/ann.rs","mnemo/src/store/load.rs","v0190-review"],"_mnemo.seen_by":["planner-a"],"_mnemo.status":"accepted","_mnemo.status_history":[{"at":1794997218,"by":"planner-a","from":"open","note":"ACCEPTED for v0.19.1 (task v0191-1), not holding the v0.19.0 tag (LOW, derived data, not new in v0.19; ensure_ann at store/ann.rs:188 clears ann_unloaded before load_index, confirmed). Behaviour chosen: a damaged index run of a known layout loads as no index, like an unknown layout: recall exactly, warn once with the mnemo index --rebuild hint and 'your memories are intact', and a read-write handle rebuilds the index with its next base write. verify reads the index run and reports it apart from the records.","to":"accepted"}],"_mnemo.triage":"ACCEPTED for v0.19.1 (task v0191-1), not holding the v0.19.0 tag (LOW, derived data, not new in v0.19; ensure_ann at store/ann.rs:188 clears ann_unloaded before load_index, confirmed). Behaviour chosen: a damaged index run of a known layout loads as no index, like an unknown layout: recall exactly, warn once with the mnemo index --rebuild hint and 'your memories are intact', and a read-write handle rebuilds the index with its next base write. verify reads the index run and reports it apart from the records.","_mnemo.triaged_by":"planner-a"}`

```text
A damaged index run: the first recall on a handle fails, later ones silently recall exactly, and verify passes (not new in v0.19)

**Context / Evidence:** tested at `9b78312` (release build, a scratch unit test in my clone), on a 20,101-fact file of 64 dimensions whose layout-2 index run is 92 pages. One byte flipped in the run's first page, a middle page, the last page, and the last page's tag. Each time:
- read-write handle: the first `recall` is `PageAuthFailed(<that page>)`; the second `recall` on the same handle is `Ok` with the right top hit (an exact scan); `verify()` is `Ok(20101)`; `build_index()` succeeds and recall is right after it.
- read-only handle: `PageAuthFailed(<that page>)`.
- So the CLI, which opens a new handle per command, fails every `mnemo recall` until `mnemo index --rebuild`, while `mnemo verify` says every page decrypts.

**Finding:**
1. `ensure_ann` (`mnemo/src/store/ann.rs:184-196`) clears `ann_unloaded` before `load_index(...)?`, so after a failed load the handle behaves as if the file had no index. A long-lived handle (the MCP server) errors once and is then slower with no word of why.
2. The index is derived data. An unknown layout byte already loads as "no index" and recalls exactly; a damaged run of a known layout is an error instead, and the error names a page, which reads as "the brain is corrupt".
3. `verify` doesn't read the index run, so it can't tell the user what's wrong.
4. `ann.rs` isn't in this release's diff: this is how v0.18 behaves too. The new read path reports the right page on every path I tried (see the task's note).

**Recommendation:** pick one behaviour for a damaged index run.
- Either treat it like an unknown layout: recall exactly, warn once ("the index is damaged; `mnemo index --rebuild` repairs it; your memories are intact"), and on a read-write handle mark the index for a rebuild;
- or keep the error on every call (set `ann_unloaded` back on failure), with that hint in the message.
- Either way, have `verify` read the index run and report it apart from the records.

**Would I hold the tag for it?** No. Derived data, an authenticated-page failure only, and one command repairs it.
```

## 01M7Q2PG7R3PX8RAPQPRARF3EF

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-18T10:19:34Z
- importance: 0.5
- metadata: `{"_mnemo.author":"reviewer-b","_mnemo.batch":"RB-015","_mnemo.category":"DX/DOCS","_mnemo.finding_id":"RB-015.2","_mnemo.kind":"finding","_mnemo.priority":"LOW","_mnemo.refs":["mnemo/src/index.rs","mnemo/tests/v0_18_compat.rs","mnemo/src/bin/mnemo/inspect.html","mnemo/src/pager.rs","v0190-review"],"_mnemo.seen_by":["planner-a"],"_mnemo.status":"accepted","_mnemo.status_history":[{"at":1794997218,"by":"planner-a","from":"open","note":"ACCEPTED for v0.19.1 (task v0191-1), not holding the tag: item 1 a fuzz_index_layout2 target through __fuzz in the smoke and weekly matrices; item 2 the old build recalls and imports on the 20,100-note file in an_older_builds_index_is_read_and_rewritten before its index; item 3 Object.hasOwn(views, name) in selectTab; item 4 no change (documented behaviour).","to":"accepted"}],"_mnemo.triage":"ACCEPTED for v0.19.1 (task v0191-1), not holding the tag: item 1 a fuzz_index_layout2 target through __fuzz in the smoke and weekly matrices; item 2 the old build recalls and imports on the 20,100-note file in an_older_builds_index_is_read_and_rewritten before its index; item 3 Object.hasOwn(views, name) in selectTab; item 4 no change (documented behaviour).","_mnemo.triaged_by":"planner-a"}`

```text
v0.19.0 at READY, smaller points: no fuzz target for the layout-2 decoder; the compat job's 20,000-fact case; a tab link; which damaged page a streamed read names

**Context / Evidence:** at `9b78312`. One line each.
1. **No fuzz target for `IvfPqIndex::read_layout2`** (`mnemo/src/index.rs`), the one new hand-written decoder; the header, record, WAL, cache-directory and MCP-frame decoders each have one. My own run: 300,000 random mutations of a valid run (bit flips, head words set to 0, 1, `u32::MAX` and random values, cuts, trailing bytes, a wrong claimed length, counts moved between partitions) gave no panic; 229,015 were refused, and the 70,985 that still decoded were then queried, changed and written again without one.
2. **CI's `compat (v0.18.1)` job runs the old build's recall and import on a 300-note layout-2 file; at 20,100 notes it runs only the old `index`.** It does make v0.18.1 read the layout byte (the index loads on the first recall at any size). By hand, with the published v0.18.1 binary (SHA-256 checked against the release) on a 20,100-note layout-2 file: a tracked recall, a 1,500-record import, one MCP session that recalls, remembers three and forgets one, then `verify`, `index` and `compact`. Every result was right, v0.19 kept its index and picked up each write (21,600 vectors after the import and the forget), and both builds' `verify` pass after the other's commits.
3. **A link ending `#__proto__`, `#constructor` or `#toString` passes `selectTab`'s check** (`views[name]` on a plain object, `inspect.html:444-449`), and the page then shows no tab (by reading; nothing reaches the DOM, so it's cosmetic).
4. **`for_each_page` names whichever damaged page arrives first; the batch read names the lowest.** With three damaged pages, 200 runs on 8 threads: the batch read said page 50 every time; the streamed read said 301 (198 times), 300 (once) and 390 (once). In 95 of the 200, some pages had reached the sink before the error. That's what its doc says; the pages delivered are authentic.

**Recommendation:** item 1: a `fuzz_index_layout2` target through `__fuzz`, in the smoke matrix. Item 2: in `an_older_builds_index_is_read_and_rewritten`, have the old build recall and import on the 20,100-note file before its `index`. Item 3: `Object.hasOwn(views, name)`. Item 4: nothing to change; it's here so nobody reads the page number in that error as "the first bad page".

**Would I hold the tag for it?** No.
```

## 01M7TM3Q6J50CY1S5RAQXSXEK5

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T19:21:36Z
- importance: 0.5
- metadata: `{"_mnemo.author":"reviewer-a","_mnemo.batch":"REV-011","_mnemo.category":"PRODUCT","_mnemo.finding_id":"REV-011.1","_mnemo.kind":"finding","_mnemo.priority":"HIGH","_mnemo.refs":["docs/design/hosted-brains.md"],"_mnemo.seen_by":["planner-a"],"_mnemo.status":"deferred","_mnemo.status_history":[{"at":1795118293,"by":"planner-a","from":"open","note":"The go/no-go is the user's. Planner agrees: (B) stays off the roadmap (paid work is on hold anyway; the sync relay stays the cleaner paid product if it ever unfreezes). (A) is the user's own business decision; spot-checked: Vercel Drives GA, Turso BYOK on Pro, Claude Managed Agents memory stores all confirmed; Turso's $417 Pro price not confirmed.","to":"needs-user"},{"at":1795118764,"by":"planner-a","from":"needs-user","note":"User 2026-11-19: not right now. (B) stays off the roadmap; (A) waits for the user. Kept in docs/ideas.md. Trigger: the user wants to test a consultancy on MNemo.","to":"deferred"}],"_mnemo.triage":"User 2026-11-19: not right now. (B) stays off the roadmap; (A) waits for the user. Kept in docs/ideas.md. Trigger: the user wants to test a consultancy on MNemo.","_mnemo.triaged_by":"planner-a"}`

```text
Split verdict on hosted brains: Reading (A) consultancy is PROVE FIRST; Reading (B) hosted-brains SaaS is NO-GO

EVIDENCE: Competitive pricing and architecture audit (full report: ~/mnemo-reviews/REV-011.md) across Turso ($4.99-$25/mo unlimited DBs; $417/mo Pro with SOC 2 Type II, HIPAA, page-level AEAD BYOK, bottomless S3, native vector search and FTS: https://turso.tech/pricing, https://docs.turso.tech/), Cloudflare Durable Objects ($5/mo + $0.20/GB SQLite per actor up to 10 GB with automatic global placement and failover: https://developers.cloudflare.com/durable-objects/platform/pricing/), turbopuffer ($16/mo Launch, object-storage WAL + NVMe cache + per-namespace CMEK: https://turbopuffer.com/pricing), Supabase/Neon, Mem0/Zep/Letta, and harness-native memory (Claude Managed Agents Memory Stores + Dreaming, Vercel Sandbox Drives GA Oct 2026, AWS Bedrock AgentCore Memory, LangGraph Postgres Store). FINDING: Reading (B) (MNemo selling hosted open-handle brains) is unfavorable: well-funded incumbents already deliver per-tenant cloud isolation, BYOK, S3 durability and SOC 2 at near-zero marginal cost, while building a stateful single-writer router, lease service, 24/7 SRE and SOC 2 Type II breaks unit economics for a small team and dilutes MNemo positioning (competes with CLAUDE.md and a notes folder). Reading (A) (the user consultancy built on MNemo) has high ARPU ($500-$5,000+/client/mo vs Harvey $1K-$1.5K/seat/mo and Hebbia $3K-$10K/seat/yr), low tenant counts (10-1,000), and directly leverages MNemo native primitives (read-only playbook + isolated client file, keyed supersession, tasks, findings, PITR, client hand-back). RECOMMENDATION: Keep Reading (B) off the roadmap; when paid work eventually unfreezes, the dumb-mailbox zero-knowledge Sync Relay (docs/design/sync.md) remains the cleaner paid server product. For Reading (A), run a 1-week zero-engine-change spike before designing any server infrastructure.
```

## 01M7TM3ZRW5V76TQW7R3R0P2AG

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T19:21:45Z
- importance: 0.5
- metadata: `{"_mnemo.author":"reviewer-a","_mnemo.batch":"REV-011","_mnemo.category":"PRODUCT","_mnemo.finding_id":"REV-011.2","_mnemo.kind":"finding","_mnemo.priority":"HIGH","_mnemo.refs":["docs/design/hosted-brains.md"],"_mnemo.seen_by":["planner-a"],"_mnemo.status":"deferred","_mnemo.status_history":[{"at":1795118293,"by":"planner-a","from":"open","note":"Trigger: the user starts reading (A). Then position the per-client file as the procurement and exit guarantee (a client engagement vault), and keep bulk documents in a separate search store.","to":"deferred"}],"_mnemo.triage":"Trigger: the user starts reading (A). Then position the per-client file as the procurement and exit guarantee (a client engagement vault), and keep bulk documents in a separate search store.","_mnemo.triaged_by":"planner-a"}`

```text
Market reality: one encrypted file per client is a procurement proof point, not a standalone headline product

EVIDENCE: Enterprise legal and advisory AI procurement benchmarks — Harvey AI (~$1,000-$1,500/seat/mo, 20-25+ seat minimums, SOC 2 Type II, ISO 27001, ISO 42001, ethical walls, zero LLM data retention: https://www.harvey.ai/) and Hebbia Matrix (~$10K/yr core, $3K-$3.5K/yr lite, permission-aware workspaces and source-cited audit trails: https://www.hebbia.com/) — plus 2025-2026 agent harness memory architectures: Anthropic Claude Managed Agents (mounted text Memory Stores with immutable versioning and background Dreaming consolidation: https://www.anthropic.com/), Vercel Sandbox Drives (GA Oct 2026, 0.7s median start, 1 RW mount + concurrent RO snapshot mounts: https://vercel.com/docs/vercel-sandbox), AWS Bedrock AgentCore Memory, LangGraph Postgres Store, and Mem0/Zep/Letta. Full report: ~/mnemo-reviews/REV-011.md. FINDING: Consulting buyers pay for expert consultation outcomes, accuracy and citations, not for a storage format. One encrypted .mnemo file per client is invisible at top-of-funnel, but becomes a concrete differentiator during security and procurement review when framed as a Client Engagement Vault: cryptographic ethical walls (no shared table RLS bug can leak Client A into Client B), clean offboarding hand-back (client receives their .mnemo file + mnemo inspect), and verifiable crypto-shredding under NDA/GDPR Art. 17. However, .mnemo does not replace SOC 2 Type II or zero-data-retention LLM contracts, and bulk raw client document corpora (thousands of PDFs) belong in dedicated RAG/object search (turbopuffer or LanceDB), not inside a single .mnemo file. RECOMMENDATION: For Reading (A), sell the standardized consulting outcome and position the per-client .mnemo vault as the security/exit guarantee during procurement; split bulk raw documents (RAG/object search) from curated engagement memory (.mnemo).
```

## 01M7TM4AB6484PHTVCWRJDTE9A

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T19:21:56Z
- importance: 0.5
- metadata: `{"_mnemo.author":"reviewer-a","_mnemo.batch":"REV-011","_mnemo.category":"PRODUCT","_mnemo.finding_id":"REV-011.3","_mnemo.kind":"finding","_mnemo.priority":"HIGH","_mnemo.refs":["docs/design/hosted-brains.md"],"_mnemo.seen_by":["planner-a"],"_mnemo.status":"deferred","_mnemo.status_history":[{"at":1795118293,"by":"planner-a","from":"open","note":"Trigger: any hosted design. Guardrails to carry: one shared KMS key wrapping per-tenant data keys (per-tenant KMS keys only as customer BYOK), and compute in the same region as storage.","to":"deferred"}],"_mnemo.triage":"Trigger: any hosted design. Guardrails to carry: one shared KMS key wrapping per-tenant data keys (per-tenant KMS keys only as customer BYOK), and compute in the same region as storage.","_mnemo.triaged_by":"planner-a"}`

```text
Unit economics at 100/1K/10K tenants: raw infra is $0.11-$2.82/tenant/mo, but naive per-tenant KMS CMKs and cross-cloud egress multiply costs 10x-29x

EVIDENCE: Verified cloud list prices — AWS KMS ($1.00/CMK/mo, up to $3.00/CMK/mo after 2 rotations; $0.03/10K requests, 20K free: https://aws.amazon.com/kms/pricing/), AWS S3 Standard ($0.023/GB-mo, $0.005/1K PUT, $0.0004/1K GET, $0 intra-region vs $0.09/GB internet egress: https://aws.amazon.com/s3/pricing/), Cloudflare R2 ($0.015/GB-mo, $0 egress, 1 write/s/key limit: https://developers.cloudflare.com/r2/pricing/), AWS EC2 i4i.large ($125.56/mo, 16 GiB RAM, 468 GB NVMe: https://aws.amazon.com/ec2/pricing/on-demand/), Fly.io Machines (https://fly.io/docs/about/pricing/) — combined with MNemo measured v0.19.0 baselines (test/scripts/perf_v0.19.0.json: RSS 3.7 MB @ 1K, 17.2 MB @ 10K, 110.9 MB @ 100K; live .git/mnemo/project.mnemo = 41 MiB at 24.8K memories). Full model and assumptions in ~/mnemo-reviews/REV-011.md. FINDING: Modeled at 50 MB avg brain + 10 MB delta log, 20 sessions/mo, 200 commits/mo, 2-5% peak concurrency: with same-region S3, delta change sets, 2-node HA NVMe compute, and hierarchical envelope KMS (1 shared CMK wrapping per-tenant DEKs; Enterprise BYOK CMK paid in customer AWS account), total raw infra is ~$282/mo at 100 tenants ($2.82/tenant; ~$75-$100/mo on Fly.io), ~$305/mo at 1K tenants ($0.31/tenant), and ~$1,078/mo at 10K tenants ($0.11/tenant), with 170-545 ms cold start and 2-4 ms warm recall. Two severe traps invert this: (1) naive 1-AWS-CMK-per-tenant adds $100-$300/mo (100), $1K-$3K/mo (1K), and $10K-$30K/mo (10K) in pure KMS fees; (2) cross-cloud S3 egress at $0.09/GB adds $900/mo at 10K tenants. In Reading (B), fixed SRE (0.5-2 FTE) plus SOC 2 Type II ($15K-$40K/yr est.) overwhelms developer-tier ARPU; in Reading (A), memory infra is <0.5% of consulting revenue (20x-90x smaller than LLM token spend). RECOMMENDATION: Any Reading (A) deployment should mandate hierarchical envelope encryption (per-tenant CMKs only as customer-owned BYOK) and same-region compute+S3 (or R2), and focus cost discipline on LLM tokens and compliance rather than storage.
```

## 01M7TM4KKNZVR3XHDV1MZKJW7Y

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T19:22:05Z
- importance: 0.5
- metadata: `{"_mnemo.author":"reviewer-a","_mnemo.batch":"REV-011","_mnemo.category":"PLAN-REFINEMENT","_mnemo.finding_id":"REV-011.4","_mnemo.kind":"finding","_mnemo.priority":"HIGH","_mnemo.refs":["docs/design/hosted-brains.md"],"_mnemo.seen_by":["planner-a"],"_mnemo.status":"accepted","_mnemo.status_history":[{"at":1795118293,"by":"planner-a","from":"open","note":"Target: roadmap step 2 (sync change sets on disk), its design note: change sets as sequential, immutable files that object storage can append with conditional writes (If-None-Match) and checkpoints with If-Match, never a full-file write-back. The lease, routing and BYOK re-check parts are deferred to any hosted design.","to":"accepted"}],"_mnemo.triage":"Target: roadmap step 2 (sync change sets on disk), its design note: change sets as sequential, immutable files that object storage can append with conditional writes (If-None-Match) and checkpoints with If-Match, never a full-file write-back. The lease, routing and BYOK re-check parts are deferred to any hosted design.","_mnemo.triaged_by":"planner-a"}`

```text
Technical risk ranking: full-file write-back and unfenced leases break first; delta change sets plus S3 conditional writes fix both

EVIDENCE: docs/design/hosted-brains.md sections 2 and 4; docs/design/sync.md sections 4-8; mnemo/src/store/shared.rs:64-71 (SharedMnemo refuses network FS and keeps secret for handle lifetime); mnemo/src/crypto.rs:71-77 (KdfParams::raw_key() runs in microseconds); mnemo/src/format.rs:144 (v9 key-slot run reserved); test/scripts/perf_v0.19.0.json; AWS S3 free conditional writes If-Match/If-None-Match (https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-requests.html); prior art from LiteFS (Consul + LTX: https://fly.io/docs/litefs/), Litestream (WAL-to-S3 RPO: https://litestream.io/), Turso Bottomless (https://docs.turso.tech/), and Cloudflare Durable Objects (https://developers.cloudflare.com/durable-objects/). Full report: ~/mnemo-reviews/REV-011.md. FINDING: Ranked failure modes in hosted-brains.md: (1) Full-file write-back of a 41-50 MB .mnemo file on commit takes 200-500 ms, saturates host NICs, and hits R2 1-write/s/key limits, while batching uploads opens a multi-minute crash RPO window; (2) Lapsed leases during GC/network stalls allow zombie writers to silently overwrite S3 state unless fenced at the storage layer; (3) Cross-host concurrent agents cannot share one file (SharedMnemo is single-host only), requiring sticky per-tenant routing; (4) Warm-handle BYOK revocation lag because SharedMnemo retains its secret in RAM until drop, plus shared-CMK wrapped-DEK backup retention delays true crypto-shredding unless key-vault rows are unbacked/purged; (5) >100K-memory files (~150-200 MB, 111 MB RSS) push cold downloads to 0.5-2.5 s. Crucially, docs/design/sync.md step 2 (MNSYNC1 delta change sets) plus S3 If-None-Match on sequential delta objects and If-Match on checkpoints solves #1 and #2 cleanly (RPO=0, storage-enforced fencing, deterministic 13.7 merge recovery), and open_with_key plus external KMS wrapping eliminates 13.2 as a prototype blocker. RECOMMENDATION: Never implement full-file write-back per commit in any hosted design; require delta change-set append logs with S3 conditional-write fencing, sticky single-host routing (or Vercel Drive single-RW mount), and a bounded 5-15 min KMS re-verification TTL for BYOK handles.
```

## 01M7TM50D07AVYPPWNXW0W1WP6

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T19:22:18Z
- importance: 0.5
- metadata: `{"_mnemo.author":"reviewer-a","_mnemo.batch":"REV-011","_mnemo.category":"PRODUCT","_mnemo.finding_id":"REV-011.5","_mnemo.kind":"finding","_mnemo.priority":"MEDIUM","_mnemo.refs":["docs/design/hosted-brains.md"],"_mnemo.seen_by":["planner-a"],"_mnemo.status":"deferred","_mnemo.status_history":[{"at":1795118293,"by":"planner-a","from":"open","note":"Whether to run the 1-week pilot is the user's call; it's glue code outside this repo, on v0.20.0, with no engine change.","to":"needs-user"},{"at":1795118765,"by":"planner-a","from":"needs-user","note":"User 2026-11-19: no pilot for now. Trigger: as REV-011.1.","to":"deferred"}],"_mnemo.triage":"User 2026-11-19: no pilot for now. Trigger: as REV-011.1.","_mnemo.triaged_by":"planner-a"}`

```text
Smallest experiment to prove or kill Reading (A): a 1-week, zero-engine-change pilot on v0.19.1/v0.20.0

EVIDENCE: Shipped MNemo v0.19.1/v0.20.0 capabilities (Mnemo::create_with_key / open_with_key with microsecond KdfParams::raw_key(), SharedMnemo, MCP server, mnemo inspect), AWS KMS GenerateDataKey/Decrypt (https://aws.amazon.com/kms/pricing/), AWS S3 conditional writes (https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-requests.html), and Vercel Sandbox Drives GA Oct 2026 (1 RW client mount + concurrent RO playbook snapshot mounts, 0.7s median startup: https://vercel.com/docs/vercel-sandbox). Full experiment design in ~/mnemo-reviews/REV-011.md. FINDING: Designing 13.2 KMS key slots, a custom lease router, or 13.4 signed writes before proving client demand for standardized consultation would invert discovery and risk months of unused infra. Everything needed to prove or kill Reading (A) already runs on v0.19.1/v0.20.0 with ~200 lines of glue code and zero Rust engine changes: wrap each client 256-bit key externally via AWS KMS, mount 1 read-only playbook.mnemo plus 1 read-write client.mnemo on a single sticky EC2 host (with S3 If-Match checkpoints) or Vercel Sandbox + 2 Drives, and test with real prospects. RECOMMENDATION: Gate any hosted-brains engine work on a 1-week two-track spike: (1) Commercial gate — demo to 5-10 target consulting buyers to verify willingness to pay (>=2-3 paid pilot commitments) and whether the exportable/shreddable Client Engagement Vault shortens security review; (2) Technical gate — verify <600 ms cold start at 50 MB, dual-brain (playbook + client) retrieval quality vs a Claude Managed Agents / pgvector baseline, and that engagement memory stays well under ~100K memories. Kill or pivot if consulting demand is absent or engagements require multi-GB bulk PDF RAG.
```

## 01M7TM587J2X9EQ98XWBGY684D

- type: episodic
- agent: `default`
- scope: shared
- created: 2026-11-19T19:22:26Z
- importance: 0.5
- metadata: `{"_mnemo.author":"reviewer-a","_mnemo.batch":"REV-011","_mnemo.category":"PLAN-REFINEMENT","_mnemo.finding_id":"REV-011.6","_mnemo.kind":"finding","_mnemo.priority":"MEDIUM","_mnemo.refs":["docs/design/sync.md"],"_mnemo.seen_by":["planner-a"],"_mnemo.status":"accepted","_mnemo.status_history":[{"at":1795118293,"by":"planner-a","from":"open","note":"A roadmap order change is the user's. Planner recommends the swap (Node.js with the AI SDK memory package before mobile see-and-delete): no measured mobile demand, and decision.vercel-supabase-ideas already puts the AI SDK package first in that step.","to":"needs-user"},{"at":1795118765,"by":"planner-a","from":"needs-user","note":"User 2026-11-19: yes. Node.js (AI SDK memory package first) moves ahead of mobile see-and-delete in decision.roadmap-order.","to":"accepted"}],"_mnemo.triage":"User 2026-11-19: yes. Node.js (AI SDK memory package first) moves ahead of mobile see-and-delete in decision.roadmap-order.","_mnemo.triaged_by":"planner-a"}`

```text
Part B pre-phase check: decision.roadmap-order after v0.20.0 stays right (optional user trade-off: Node.js/AI SDK ahead of mobile see-and-delete)

EVIDENCE: decision.roadmap-order (2026-11-18), decision.vercel-supabase-ideas (2026-11-18), Part A findings REV-011.1 to REV-011.5 (~/mnemo-reviews/REV-011.md), and 2025-2026 agent harness platforms (Anthropic Claude Managed Agents Dreaming background consolidation, Vercel AI SDK and Sandbox Drives GA Oct 2026, LangGraph JS, Cloudflare Agents). FINDING: The post-v0.20.0 free roadmap — (1) 13.7 mnemo merge, (2) sync change sets on disk, (3) smarter memory (dedupe, summarize, link, prune, mnemo doctor, MCP elicitation), (4) mobile see-and-delete, (5) Node.js package and Vercel AI SDK + sandbox/drives guide; paid features on hold — is strongly validated by Part A. Steps 1 and 2 serve local multi-machine users now and simultaneously remove the top two hosted-brains risks (delta S3 writes and split-brain reconciliation). Step 3 directly answers the market shift toward background memory consolidation (Claude Managed Agents Dreaming, Mem0/Zep consolidation) and keeps long-lived brains below the 100K-memory ceiling. Keeping relay, Pro, 13.2, 13.4 and Teams on hold remains right because Reading (B) is a no-go and Reading (A) does not need 13.2 to prototype. The one trade-off worth surfacing for the user is Steps 4 vs 5: 2026 agent harnesses are overwhelmingly TypeScript/Node.js-first, so Node.js + Vercel AI SDK + Sandbox/Drives guide (1 RW client drive + 1 RO playbook drive) has higher immediate ecosystem leverage for both local builders and Reading (A) pilots than mobile see-and-delete, unless mobile users are actively pulling for Step 4. RECOMMENDATION: Keep Steps 1 -> 2 -> 3 intact and keep paid features on hold. Surface one optional decision for the user: whether to swap Step 5 (Node.js / Vercel AI SDK + sandbox/drives guide) ahead of Step 4 (mobile see-and-delete).
```

## 01M7PEZYWFKE4Y6NHMM8XZSN57

- type: semantic
- agent: `default`
- scope: private
- created: 2026-11-18T04:35:12Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.kind":"essence","_mnemo.refs":["decision.positioning"],"_mnemo.section":"decisions","_mnemo.status":"open","_mnemo.status_history":[{"at":1794976512,"status":"open"}]}`

```text
Positioning: MNemo doesn't compete with vector databases; it competes with CLAUDE.md and a notes folder. Explained in three layers. Say coordinate, never orchestration. Free if it runs on one machine.
```

## 01M7PEZYYBAR4N4936PS8EW9PQ

- type: semantic
- agent: `default`
- scope: private
- created: 2026-11-18T04:35:12Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.kind":"essence","_mnemo.refs":["decision.run-2","decision.security-finding-private-until-verified"],"_mnemo.section":"decisions","_mnemo.status":"open","_mnemo.status_history":[{"at":1794976512,"status":"open"}]}`

```text
Every task starts with a recall; its report says found, partial, not stored or not found. Unfixed SECURITY findings are private records, named by id only anywhere else.
```

## 01M7PEZYPDHB7CVQMFQJ01P16S

- type: semantic
- agent: `default`
- scope: private
- created: 2026-11-18T04:35:12Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.kind":"essence","_mnemo.refs":["decision.run-2","status.v0.18.1"],"_mnemo.section":"state","_mnemo.status":"open","_mnemo.status_history":[{"at":1794976512,"status":"open"}],"_mnemo.updated_at":1795053750}`

```text
v0.19.1 released 2026-11-18 (a damaged index run recalls exactly and is rebuilt; verify_index; I/O retry; layout-2 fuzz target); mobile stays 0.19.0. v0.19.0 (same day): first search 11.5 ms at 100K, 4.2 ms at 10K. The brain carries the work, permanently (AGENTS.md 470b9de).
```

## 01M7PEZZ06ZB9C0VRTDF2P6PE5

- type: semantic
- agent: `default`
- scope: private
- created: 2026-11-18T04:35:13Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.kind":"essence","_mnemo.section":"open","_mnemo.status":"open","_mnemo.status_history":[{"at":1794976513,"status":"open"}]}`

```text
Carried: the shared_lock and EmbedQueueTests timing flakes (logged, change only if they recur); KEYCHAIN-REPROMPT; WAKE-SOCKET; PY-SDIST; RB-006.12 (daemon design, SECURITY.local.md); the Planner's sync design note.
```

## 01M7PEZYRGDWXEZWMFNTQQY338

- type: semantic
- agent: `default`
- scope: private
- created: 2026-11-18T04:35:12Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.kind":"essence","_mnemo.refs":["spec.perf-2","spec.inspect-date-filter"],"_mnemo.section":"next","_mnemo.status":"open","_mnemo.status_history":[{"at":1794976512,"status":"open"}],"_mnemo.updated_at":1795056958}`

```text
v0.20.0 (user's go 2026-11-18, spec.v0.20.0): tasks v0200-0 to v0200-5: setup; the 3f fsync merge; incremental catch-up note and baseline, then build; shared by default for serve --mcp; READY. One REVIEWER-B pass at READY. Planner writes the sync design note meanwhile.
```

## 01M7PEZYTHY4K8FJG46J6CTCVF

- type: semantic
- agent: `default`
- scope: private
- created: 2026-11-18T04:35:12Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.kind":"essence","_mnemo.refs":["status.roadmap"],"_mnemo.section":"next","_mnemo.status":"open","_mnemo.status_history":[{"at":1794976512,"status":"open"}],"_mnemo.updated_at":1795118764}`

```text
After v0.20.0 (decision.roadmap-order): 13.7 mnemo merge; sync change sets on disk; smarter memory (elicitation, mnemo doctor); Node.js (AI SDK memory first); mobile see-and-delete. On hold: relay, Pro, 13.2, 13.4, Teams, hosted brains (REV-011). Ideas explored: docs/ideas.md.
```

## 01M7RXSCBD05ZRF8W7CR9BEY43

- type: semantic
- agent: `default`
- scope: private
- created: 2026-11-19T03:32:14Z
- importance: 0.5
- metadata: `{"_mnemo.author":"planner-a","_mnemo.kind":"essence","_mnemo.refs":["decision.vercel-supabase-ideas","decision.roadmap-order"],"_mnemo.section":"next","_mnemo.status":"open","_mnemo.status_history":[{"at":1795059134,"status":"open"}]}`

```text
Folded into the roadmap (decision.vercel-supabase-ideas): confirmation before destructive MCP tools and mnemo doctor health checks go in smarter memory; an AI SDK memory package, an eve extension and a sandbox guide lead the Node.js step. Backlog: public repeatable benchmarks.
```

## 01KSGPG9WA2CP74CWHH44SRE4D

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 1
- metadata: `{"area":"product","topic":"overview"}`

```text
MNemo (Memory Nemo) is an encrypted single-file portable agent-memory engine written in Rust. One .mnemo file holds vectors, text, metadata, and recall logic with no external vector server.
```

## 01KSGPG9WA2F2XEEH3STG1MYS9

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.95
- metadata: `{"area":"layout"}`

```text
Repository layout: mnemo/ is the Rust core and CLI; mnemo-python/ is PyO3 bindings via maturin; mnemo/bindings/node/ is an napi-rs scaffold not shipped yet; index.html is the GitHub Pages landing page; MNEMO_BUILD_PLAN.md is the full phased spec.
```

## 01KSGPG9WAR9G9AWQ1JMYSXGQ3

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.9
- metadata: `{"area":"architecture"}`

```text
Core modules: store.rs (Mnemo DB), memory.rs (Memory types), session.rs (conversation wrapper), pager.rs, wal.rs (durability), format.rs, crypto.rs (Argon2id + AES-256-GCM), index.rs (IVF+PQ), cache.rs (LRU). Crate forbids unsafe_code.
```

## 01KSGPG9WB6MEDRNY03RT276Z0

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.8
- metadata: `{"area":"durability"}`

```text
flush() is one WAL-backed atomic transaction with single-fsync commit. Snapshots record every flush; restore_to(txn_id) rewinds until compact_file collapses history.
```

## 01KSGPG9WBDJXJ64Z25E3QCSGW

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.9
- metadata: `{"area":"model"}`

```text
Memory types: episodic (events/conversations), semantic (facts), procedural (rules/preferences), working (current session scratch). Session close() consolidates working into episodic.
```

## 01KSGPG9WBJ767DJ7XN716CXZE

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.75
- metadata: `{"area":"testing"}`

```text
Test suite: 28 integration tests in mnemo/tests/integration.rs plus 9 CLI smoke tests in mnemo/tests/cli_smoke.rs (get, list, recall). cargo test in mnemo/ runs all.
```

## 01KSGPG9WBVPP7FH10B64SXXND

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.85
- metadata: `{"area":"recall"}`

```text
recall() blends similarity with recency, importance, and ln(1+access_count). Filters by memory type, agent scope (private + shared), and TTL expiry. search() is always exact brute-force ground truth.
```

## 01KSGPG9WBY6NE1EQ70B2Y68H3

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.85
- metadata: `{"area":"crypto"}`

```text
Encryption: passphrase derives KEK via Argon2id; KEK wraps random DEK; DEK encrypts every 8 KiB data page with AES-256-GCM. Page 0 header is plaintext. rekey() only re-wraps DEK.
```

## 01KSGPG9WBZMFYTZDPKMEVNYTX

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.9
- metadata: `{"area":"commands"}`

```text
Run Rust quickstart: cd mnemo && cargo run --example quickstart. Run CLI demo: cargo run --bin mnemo -- demo. Passphrase via MNEMO_PASSPHRASE env, not CLI args in production.
```

## 01KSGPG9WC92XCBYDPAPMGVWMP

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.7
- metadata: `{"area":"roadmap"}`

```text
Roadmap gap: TypeScript/Node bindings planned in MNEMO_BUILD_PLAN Phase 6; only Python bindings are production-ready in mnemo-python/.
```

## 01KSGPG9WCCC1GGM3GCSFHBMXA

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.9
- metadata: `{"area":"cli","topic":"explore"}`

```text
Explore a .mnemo file via CLI: mnemo info (stats), mnemo list (browse, O(n) decrypt), mnemo get <ULID> (one record), mnemo recall --query vec (multi-signal). Workflow: info → list → get → recall. Or use python test/scripts/project_memory.py recall for NL queries with embeddings.
```

## 01KSGPG9WCK4V39BTZQPQQDCG6

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.7
- metadata: `{"area":"meta"}`

```text
Remote origin: https://github.com/toarchkumar/mnemo.git. Site: https://toarchkumar.github.io/mnemo/. License Apache-2.0. Latest dogfood+CLI work on main includes commit 6496f55 (list/get/recall CLI, test sandbox, Cursor rule).
```

## 01KSGPG9WCS161MPXX92539F1P

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.95
- metadata: `{"area":"cli"}`

```text
CLI commands: init, info, import (JSONL), index, list, get, search, recall, verify, rekey, compact, snapshots, restore, demo. list/get/recall support --format table|json|jsonl. recall needs --query as comma-separated floats (same dim as DB); search is similarity-only.
```

## 01KSGPG9WCYWMSWX1SCJM19DSE

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.85
- metadata: `{"area":"python"}`

```text
Python bindings: cd mnemo-python && pip install maturin && maturin develop. API: mnemo.open(path, passphrase, dimensions=...), remember, recall, session, flush, build_index, snapshots.
```

## 01KSGPG9WD4VCCR4TDCRBCJXS6

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.9
- metadata: `{"area":"dogfood","event":"bootstrap"}`

```text
2026-05-25: Started MNemo dogfooding — test/project.mnemo with local sentence-transformers embeddings and cursor-agent as agent_id. Passphrase in test/.env (gitignored).
```

## 01KSGPG9WDRH5F0YMGJD9XMJEE

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.7
- metadata: `{"area":"docs"}`

```text
index.html CLI tab documents real mnemo subcommands: MNEMO_PASSPHRASE env, init/import/index, list/get/recall/search with vector queries, rekey --new-passphrase. Landing page matches mnemo/README.md.
```

## 01KSGPG9WDS013NNJFNC2BXTWT

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.95
- metadata: `{"area":"dogfood","commit":"6496f55","event":"release"}`

```text
2026-05-25: Shipped CLI exploration (list, get, recall) and dogfood sandbox in commit 6496f55 on main. Cursor rule .cursor/rules/project-memory.mdc recalls test/project.mnemo before substantive work.
```

## 01KSGPG9WDTY6CP6EFJCTT7MHE

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-25T23:11:16Z
- importance: 0.95
- metadata: `{"area":"dogfood","event":"onboarding-refresh"}`

```text
2026-05-25: Expanded project.mnemo seed with explicit new-session onboarding memories (mission, access paths, agent workflow, setup, bootstrap) so a fresh AI can recall and understand the full dogfood context.
```

## 01KSGQ783ZTT6GAZ4J8YA0F6FH

- type: working
- agent: `default`
- scope: private
- created: 2026-05-25T23:23:48Z
- importance: 0.1

```text
bench
```

## 01KSGQ784Z3RSZZCMEE3MT5C2B

- type: working
- agent: `default`
- scope: private
- created: 2026-05-25T23:23:48Z
- importance: 0.1

```text
bench
```

## 01KSGQ785ZVXHH91B8CAX38WVZ

- type: working
- agent: `default`
- scope: private
- created: 2026-05-25T23:23:48Z
- importance: 0.1

```text
bench
```

## 01KSGSEA2P075XTGGPPKVZ42DD

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release-python (dogfood DB)","corpus":"31 memories, 384-dim, encrypted, no ANN index","measured_at":"2026-05-25","metric":"bytes_per_memory_on_disk","notes":"Worst case — raw payload is ~50 KB total (~1.6 KB/memory); fixed 512 KiB WAL reservation + page rounding + snapshot history dominate at small N.","topic":"bytes_per_memory_on_disk","units":"bytes","value":45000,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] Effective on-disk cost per memory: 45000 bytes (build=release-python (dogfood DB); corpus=31 memories, 384-dim, encrypted, no ANN index) — Worst case — raw payload is ~50 KB total (~1.6 KB/memory); fixed 512 KiB WAL reservation + page rounding + snapshot history dominate at small N.
```

## 01KSGSEA2P0AJ3X7MJT8JEMVYF

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release-python (dogfood DB)","corpus":"31 memories, 384-dim, encrypted, no ANN index","measured_at":"2026-05-25","metric":"payload_share_pct","notes":"~50 KB payload (6.8 KB text + 47.6 KB f32 vectors) inside ~844 KB at 28 memories.","topic":"payload_share_pct","units":"%","value":5.9,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] Logical text+vectors as fraction of file: 5.9 % (build=release-python (dogfood DB); corpus=31 memories, 384-dim, encrypted, no ANN index) — ~50 KB payload (6.8 KB text + 47.6 KB f32 vectors) inside ~844 KB at 28 memories.
```

## 01KSGSEA2P4K40QC01GWG0XGNF

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"debug-cli","corpus":"31 memories, 384-dim, encrypted, no ANN index","measured_at":"2026-05-25","metric":"cli_info_latency_ms","notes":"KDF-bound: ~275 ms Argon2id + decrypt header/catalog. Release build would be faster on the non-KDF part only.","topic":"cli_info_latency_ms","units":"ms","value":330,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] CLI `mnemo info` median latency: 330 ms (build=debug-cli; corpus=31 memories, 384-dim, encrypted, no ANN index) — KDF-bound: ~275 ms Argon2id + decrypt header/catalog. Release build would be faster on the non-KDF part only.
```

## 01KSGSEA2P75S9A5524RZTMJQR

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release-python (dogfood DB)","corpus":"8 KiB pages","measured_at":"2026-05-25","metric":"per_page_crypto_overhead_pct","notes":"28 bytes/page: 12-byte nonce + 16-byte GCM tag. Encryption is not the source of size bloat at small N.","topic":"per_page_crypto_overhead_pct","units":"%","value":0.3,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] AES-256-GCM per-page overhead: 0.3 % (build=release-python (dogfood DB); corpus=8 KiB pages) — 28 bytes/page: 12-byte nonce + 16-byte GCM tag. Encryption is not the source of size bloat at small N.
```

## 01KSGSEA2PBEM421927GKHB76E

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"debug-cli","corpus":"31 memories, 384-dim, encrypted, no ANN index","measured_at":"2026-05-25","metric":"cli_verify_latency_ms","notes":"Same shape as list: KDF + decrypt/validate every live record.","topic":"cli_verify_latency_ms","units":"ms","value":330,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] CLI `mnemo verify` median latency: 330 ms (build=debug-cli; corpus=31 memories, 384-dim, encrypted, no ANN index) — Same shape as list: KDF + decrypt/validate every live record.
```

## 01KSGSEA2PF8YV4RQF23SVD7MM

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release-python (dogfood DB)","corpus":"31 memories, 384-dim, encrypted, no ANN index","measured_at":"2026-05-25","metric":"file_size_bytes","notes":"Was ~844 KB at 28 memories; grew via append-only history after benchmark writes. compact would collapse old snapshot pages.","topic":"file_size_bytes","units":"bytes","value":1409024,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] test/project.mnemo file size: 1409024 bytes (build=release-python (dogfood DB); corpus=31 memories, 384-dim, encrypted, no ANN index) — Was ~844 KB at 28 memories; grew via append-only history after benchmark writes. compact would collapse old snapshot pages.
```

## 01KSGSEA2PGK5YQR5V4WRHHCZE

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release-python (dogfood DB)","corpus":"844 KB file at 28 memories","measured_at":"2026-05-25","metric":"wal_share_pct","notes":"64 pages = 512 KiB reservation, fixed today. Smaller initial WAL would need a config change. WAL share shrinks as N grows.","topic":"wal_share_pct","units":"%","value":62,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] Reserved WAL region as fraction of file: 62 % (build=release-python (dogfood DB); corpus=844 KB file at 28 memories) — 64 pages = 512 KiB reservation, fixed today. Smaller initial WAL would need a config change. WAL share shrinks as N grows.
```

## 01KSGSEA2PGWFKEQR0E2ZD2FZG

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release","corpus":"KdfParams::secure() — m_cost=19456, t_cost=2","measured_at":"2026-05-25","metric":"argon2id_open_latency_ms","notes":"Range observed ~250–300 ms. This cost dominates every sub-second CLI command. Long-lived processes amortize it across many calls.","topic":"argon2id_open_latency_ms","units":"ms","value":275,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] Argon2id KEK derivation on every open: 275 ms (build=release; corpus=KdfParams::secure() — m_cost=19456, t_cost=2) — Range observed ~250–300 ms. This cost dominates every sub-second CLI command. Long-lived processes amortize it across many calls.
```

## 01KSGSEA2PHG582Y1GJQV34FP5

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release-python (maturin develop)","corpus":"31 memories, 384-dim, encrypted, no ANN index","measured_at":"2026-05-25","metric":"python_recall_top10_latency_ms","notes":"Same crypto as the CLI but on a release build, and the binary cost is much lower. Long-lived processes that keep the DB open avoid the KDF tax entirely on subsequent calls.","topic":"python_recall_top10_latency_ms","units":"ms","value":35,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] Python open + recall top-10 (release wheel): 35 ms (build=release-python (maturin develop); corpus=31 memories, 384-dim, encrypted, no ANN index) — Same crypto as the CLI but on a release build, and the binary cost is much lower. Long-lived processes that keep the DB open avoid the KDF tax entirely on subsequent calls.
```

## 01KSGSEA2PQ2WK7ZHYG67PSSAQ

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"debug-cli","corpus":"31 memories, 384-dim, encrypted, no ANN index","measured_at":"2026-05-25","metric":"cli_list_latency_ms","notes":"KDF + decrypt every record; ~90 memories/s decrypt+decode throughput after Argon2id is paid.","topic":"cli_list_latency_ms","units":"ms","value":330,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] CLI `mnemo list` median latency (full live set): 330 ms (build=debug-cli; corpus=31 memories, 384-dim, encrypted, no ANN index) — KDF + decrypt every record; ~90 memories/s decrypt+decode throughput after Argon2id is paid.
```

## 01KSGSEA2PR5NVC697HFYRP9SR

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release-python (dogfood DB)","corpus":"844 KB file at 28 memories","measured_at":"2026-05-25","metric":"data_share_pct","notes":"38 pages = 311 KB of records, catalog snapshots, and history (append-only).","topic":"data_share_pct","units":"%","value":37,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] Data/catalog/manifest pages as fraction of file: 37 % (build=release-python (dogfood DB); corpus=844 KB file at 28 memories) — 38 pages = 311 KB of records, catalog snapshots, and history (append-only).
```

## 01KSGSEA2PYX35Q7QV834WE0TW

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"debug-cli","corpus":"31 memories, 384-dim, encrypted, no ANN index","measured_at":"2026-05-25","metric":"cli_get_latency_ms","notes":"Per-record decrypt is microseconds; ~290 ms is almost all Argon2id. Point reads at ~3/s if you reopen each time.","topic":"cli_get_latency_ms","units":"ms","value":290,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] CLI `mnemo get` median latency (single record): 290 ms (build=debug-cli; corpus=31 memories, 384-dim, encrypted, no ANN index) — Per-record decrypt is microseconds; ~290 ms is almost all Argon2id. Point reads at ~3/s if you reopen each time.
```

## 01KSGSEA2PZVN087RQSK6YP8Q1

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T00:02:36Z
- importance: 0.7
- metadata: `{"area":"performance","build":"release-python (maturin develop)","corpus":"31 memories, 384-dim","measured_at":"2026-05-25","metric":"python_remember_flush_latency_ms","notes":"Includes one MiniLM embedding + one WAL-fsynced transaction. Each durable write costs roughly one commit; append-only layout adds catalog/manifest pages per flush.","topic":"python_remember_flush_latency_ms","units":"ms","value":31,"version":"v0.1.0"}`

```text
PERF [v0.1.0, 2026-05-25] Python remember + flush (one durable write): 31 ms (build=release-python (maturin develop); corpus=31 memories, 384-dim) — Includes one MiniLM embedding + one WAL-fsynced transaction. Each durable write costs roughly one commit; append-only layout adds catalog/manifest pages per flush.
```

## 01KSK8ZEP80CENKXY3GGVDH8G3

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 0.75
- metadata: `{"area":"tooling","topic":"cross-compile-wheel"}`

```text
CROSS-BUILD Linux-arm64 wheel from Mac (for sandboxed agents that need the Python binding before Phase 6 publishes prebuilt wheels): (1) rustup target add aarch64-unknown-linux-gnu, (2) pip install ziglang, (3) cd mnemo-python && maturin build --release --target aarch64-unknown-linux-gnu --zig. Wheel lands at mnemo-python/target/wheels/mnemo_db-0.1.0-cp38-abi3-manylinux_2_17_aarch64.whl. Install in a Linux sandbox with `pip install --user --force-reinstall <wheel>`. Becomes obsolete once Phase 6 ships prebuilt wheels to PyPI for all 4 major platforms.
```

## 01KSK8ZEP89Y4DCDMG2T2X0TF4

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 1
- metadata: `{"area":"architecture","topic":"self-describing-db"}`

```text
SELF-DESCRIBING DB convention (engine-level, v0.2): memories tagged metadata.area='onboarding' surface via Mnemo::about() (Rust), db.about() (Python), and `mnemo about <path>` (CLI). The canonical orientation memory has metadata.topic='manifest' and is hoisted to the top regardless of importance. `mnemo init` auto-inserts a scaffold manifest tagged metadata.scaffold=true so new files are self-describing from creation — replace it with one that records your real embedder/agent_id/conventions and drop the scaffold flag. Single-file philosophy: every fact a downstream agent needs lives IN the .mnemo file, not in sibling AGENTS.md/config.
```

## 01KSK8ZEP8ACQ23X6S13QYDFJ6

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 0.7
- metadata: `{"area":"architecture","topic":"sizing-mrl-and-future"}`

```text
SIZING TIP: vector storage = dimensions × 4 bytes/memory and dominates everything but tiny corpora. With MRL-trained embedders (OpenAI text-embedding-3-*, Nomic nomic-embed-text-v1.5, Snowflake arctic-embed-l-v2.0), the first k dimensions carry most of the signal — truncate to k client-side before db.remember(). 4× storage savings, 1-2% recall loss. Compose with WAL right-sizing for ~8× small-N footprint reduction vs v0.1.0 defaults. Future engine work flagged in mnemo/README's Sizing tips: Zstd record compression (~30-60% on text/metadata) and Binary Quantization (~32× on vectors) — both will compose on top of MRL+dimensions, not replace.
```

## 01KSK8ZEP8DT743ZJAM154Y0Z5

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 0.75
- metadata: `{"area":"product","topic":"positioning"}`

```text
PROJECT POSITIONING (v0.2): 'agentic-first' — mnemo is built for AI agents to ONBOARD themselves (self-describing DB, scaffold-by-default, parity across Rust/Python/CLI surfaces), not just to be USED by agents. Landing page (index.html), all three READMEs, and AGENTS.md carry this framing. Future design decisions should preserve it: anything that requires a sibling file or external config to be useful is suspect. The hero subtitle, a dedicated 'Designed for agents to onboard themselves' section, and the bottom CTA all reinforce it.
```

## 01KSK8ZEP8GJ18871KXZ1TQSVP

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 0.85
- metadata: `{"area":"cli","topic":"exploration-commands-v0.2"}`

```text
CLI exploration commands added in v0.2: `mnemo about` (self-describing briefing), `mnemo list` (browse with --type/--agent/--limit/--offset/--sort and --format table|json|jsonl), `mnemo get <ulid>` (--verbose/--vector), `mnemo recall --query VEC` (multi-signal NL retrieval with --metric/--n-probe/--n-rerank). Python parity: db.about(), db.insert_default_manifest(). All CLI commands support --format table|json|jsonl. The library API gained Mnemo::about(), Memory::scaffold_manifest(), and RecallRequest builders .metric()/.weights().
```

## 01KSK8ZEP8HEW3ZEWX628P0QMA

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 0.95
- metadata: `{"area":"distribution","topic":"package-name"}`

```text
DISTRIBUTION NAME is mnemo-db (NOT mnemo) on PyPI and crates.io — the bare 'mnemo' name was taken on both (PyPI: Gabriele Girelli's 2020 notebook assistant; crates.io: aayushadhikari7/mnemo, unrelated). LIBRARY/IMPORT name stays mnemo end-to-end: `pip install mnemo-db; import mnemo` works, `cargo add mnemo-db; use mnemo::...` works, CLI binary is still `mnemo`. Path-deps in bindings use `package = "mnemo-db"` syntax to keep source-level `use mnemo` working. pip normalizes the dash so wheel filenames read `mnemo_db-0.1.0-...whl`.
```

## 01KSK8ZEP8PPJ58TJYN9NVRXXG

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 0.85
- metadata: `{"area":"dogfood","topic":"perf-workflow"}`

```text
PERFORMANCE TRACKING workflow (v0.2): `project_memory.py perf <file.json>` ingests structured measurements as episodic memories with metadata.area='performance'. Required entry keys: version, metric, label. Recommended: measured_at, value, units, build, corpus, notes. Schema example: test/scripts/perf_v0.1.0.json (13 entries from the first dogfood pass). Workflow per release: copy the latest baseline JSON to perf_v<next>.json, edit values, re-ingest. Recall later by querying metric names or version tags. mnemo/README's Performance section has a history table that gains one row per release.
```

## 01KSK8ZEP8T87CXM1EFXY8HJR1

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-05-26T23:12:36Z
- importance: 0.9
- metadata: `{"area":"architecture","date":"2026-05-26","topic":"wal-defaults-v0.2"}`

```text
2026-05-26 (v0.2): Dropped default initial WAL from 64→8 pages in MnemoConfig.wal_pages_initial. Fresh init'd file with scaffold manifest: ~544 KB → ~98 KB apparent (82% smaller; measured 98304 bytes on macOS APFS). The WAL auto-grows beyond the initial reservation, so this is a hint about expected per-transaction size, not a cap. Configure higher (e.g. 64) only for write-heavy workflows that routinely commit large catalogs or ANN indexes.
```

## 01KZFPTT7NABFQ7PJR7Z3QF3WV

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-08-08T03:31:57Z
- importance: 0.85

```text
PR 1 shipped: OS file lock via fs4 on all opens (exclusive for create/open, shared for open_read_only). MnemoError adds Locked/ReadOnly/NeedsWriteOpen. CLI: read-only commands go through open_read_only; recall --no-track added. Python: mnemo.open(read_only=False) kwarg. Reader coexists with reader; reader cannot coexist with writer (flock semantics — writer must drop first). No format bump; v7 preserved.
```

## 01KZFR0QQKNT3C81J4V065DF88

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-08-08T03:52:39Z
- importance: 0.9

```text
PR 2 shipped: hand-rolled MCP server (mnemo serve --mcp). rmcp rejected because it declares edition=2024 (needs Rust 1.85+); we're pinned to 1.75. New mnemo/src/mcp.rs, ~400 LoC, zero new deps. 7 tools: about, remember, recall, forget, list, snapshot_list, stats. Passphrase via MNEMO_PASSPHRASE only. Mutating tools flush before returning. IndexInfo now derives Serialize/Deserialize. Two new subprocess-driven smoke tests. README has Claude Desktop mcpServers config block.
```

## 01KZFV620KD0FCP8XZ22T5XY8G

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-08-08T04:48:00Z
- importance: 0.9

```text
Windows CI landmine: fs4's LockFileEx is MANDATORY (unlike Unix flock which is advisory). Two hotfix rounds after PR 1: (1) open_with_lock must recognize Windows raw OS codes 32 (ERROR_SHARING_VIOLATION) and 33 (ERROR_LOCK_VIOLATION) as MnemoError::Locked — Rust maps them to ErrorKind::Uncategorized, not WouldBlock, since std::io::ErrorKind has no portable 'resource-locked' variant. (2) Any test doing std::fs::read/write or the read_bytes/write_bytes helpers must drop(db) BEFORE the raw I/O, not after — the writer's exclusive lock blocks even plain reads on Windows. Python's close() is also broken here: it flushes but doesn't drop the inner Core, so tests need explicit del db before reopens; proper fix is refactoring mnemo-python's Mnemo.inner to Option<Core>.
```

## 01KZFYP63PB4405ATE145QY6P4

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-08-08T05:49:14Z
- importance: 0.9

```text
PR 4 shipped: Phase 10.2 semantic cache. cache_put_semantic + cache_get_semantic on Mnemo. BYO vector (dimension-checked), mandatory model string matched exactly on lookup. Top-1 cosine scan over live vectored entries in the namespace. DEFAULT_SEMANTIC_THRESHOLD=0.97. No format bump — CacheEntry/CacheDirectoryEntry gained Option<Vec<f32>> vector + Option<String> model with #[serde(default)]; rmp-serde tolerantly decoded pre-existing v8 entries as None/None. memory::cosine promoted to pub(crate) so semantic recall shares scoring with Memory recall. 5 new integration tests including a coexistence regression proving old-shape and new-shape entries round-trip in the same file. Deferred to PR 5: MCP tools + Python mirror + @db.cached(embed=...) decorator.
```

## 01KZG35FWD59N8HYY2KZRMRXYY

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-08-08T07:07:30Z
- importance: 0.85

```text
PR 5 shipped: Phase 10.4 live surfaces. MCP tools cache_put/get/stats + semantic variants (BYO vectors as JSON arrays, matching remember/recall). Python db.cache_* mirrors (5 exact-key + 2 semantic; py_value_to_bytes accepts str or bytes; cache_put_semantic needs allow(clippy::too_many_arguments) because PyO3 kwargs shape doesn't compress). ~90-line @db.cached decorator recipe in mnemo-python/README.md with OpenAI/Anthropic/semantic-mode examples — shipped as copy-paste documentation, not an installable module. Completes the 1.1 → 4 → 10 execution order from Aug 2026.
```

## 01M161C9E99AJNNR7CG8A1NFEB

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-08-29T05:55:15Z
- importance: 0.85

```text
v0.4.0 released (2026-08-29): format v8, MCP serve --mcp, exact-key + semantic result cache with Batched flush lane, OS file locking + read-only opens. Live on PyPI (6 files incl new sdist), crates.io, and GitHub Releases (11 assets). Shipped as one release commit (659004d) plus PR 6.1 (de5e375) — a test-only retry-hardening for mcp_serve_initialize_list_and_call_tools that surfaced a Linux fork/flock race under Rust 1.75 MSRV CI: cargo runs cli_smoke.rs and integration.rs in parallel; on 1.75 Linux Command::spawn still uses fork+exec which briefly clones parent FDs, so a sibling test's spawn can inherit our writer's flock for a few ms after our db has dropped. Stable Rust uses posix_spawn and doesn't have the window. Fix is a 5-attempt bounded retry (100/200/400/800 ms) triggered only on the specific 'file already locked' stderr marker — the engine lock stays untouched, it's a correctness invariant. Format v7 files (v4/v5/v6/v7) migrate on first open under v0.4.0; files written by v0.4.0 do NOT open on 0.3.x (v8 header seal AAD covers new cache pointer). release.yml gained a build-sdist job so PyPI now serves a source distribution alongside the wheels — first sdist ever for mnemo-engine. Repo-wide cargo fmt landed with this release (planner decision: bigger release commit vs deferring to PR 9 which now just wires fmt-check CI + cargo-deny).
```

## 01M33R00TB3T2WAZR78BYNSZMK

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-09-22T05:04:10Z
- importance: 0.7

```text
PR 7 shipped Phase 1.2 fuzzing: 5 cargo-fuzz targets (header, wal_replay, record_decode, cache_directory, mcp_frame) + fuzz-smoke on every PR + weekly deep run. No panics found.
```

## 01M33R0E5N4AC65PBAKSMHD98Y

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-09-22T05:04:24Z
- importance: 0.7

```text
PR 8 shipped Phase 1.3: proptest state machine + crash-injection failpoint (feature-gated). It caught two model bugs and zero engine bugs; regression seeds pinned.
```

## 01M33R0RE7HF4AWARRKXDZ6V3D

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-09-22T05:04:34Z
- importance: 0.8

```text
Authoritative execution order as of 2026-09-21: next is PR 9 (Phase 1.4 cargo-deny + fmt CI + supply-chain), then PR BP (binding parity), then Phase 3 embedder. Full list in PLAN.md NEXT DISPATCH.
```

## 01M33R1B3TMVK04VXA119JF1G1

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-09-22T05:04:53Z
- importance: 0.7

```text
Research inputs folded into Phase 7: WikiSkill (persistent knowledge layer) and StreamMeCo + the 2026 forgetting literature converge on a composite forget-score = relevance + exponential decay + access frequency + connectivity/degree. Text-domain-validated; low novelty.
```

## 01M5NWDKJ7QRPNEGJ8SR51GRPG

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-10-24T02:39:13Z
- importance: 1
- metadata: `{"_mnemo.default_agent":"cursor-agent","_mnemo.updated_at":1794860376,"agent_id_default":"cursor-agent","area":"onboarding","conventions":{"area_values":["onboarding","performance","decision","product","architecture"],"perf_keys":["version","metric","value","units","build","corpus"]},"embedder":{"dimensions":384,"name":"Snowflake/snowflake-arctic-embed-xs@d8c86521100d3556476a063fc2342036d45c106f","normalize":true,"query_convention":"arctic-query-prefix"},"project":{"dogfood_helper":"test/scripts/project_memory.py","engine_docs":"mnemo/README.md","name":"Memory Nemo (MNemo)","repo":"https://github.com/toarchkumar/mnemo"},"topic":"manifest"}`

```text
MNEMO MANIFEST — This .mnemo file is the project memory (the project brain) for Memory Nemo (MNemo), an encrypted single-file Rust agent-memory engine. Since the v0.16 dogfood switch (2026-11-15) it's .git/mnemo/project.mnemo, and its generated key is in the user's keychain. Embedder: the engine's built-in snowflake-arctic-embed-xs at a pinned revision (384-dim, normalised, with its query prefix; since v0.15), all-MiniLM-L6-v2 before. Facts belong to agent_id cursor-agent, the brain's default agent (`mnemo default-agent`, since v0.17): a write that names no agent goes to it, so a keyed write supersedes in the same slot. Metadata convention: area in {onboarding, performance, decision, ...} with a topic key. Self-describing: `mnemo about` (no path in this repo) shows every area=onboarding memory, most important first; that briefing is the source of truth for how to use this file. Dogfood helper: test/scripts/project_memory.py. Engine docs: mnemo/README.md.
```

## 01M5R6A97766V5AGRD8K24VY4E

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-10-25T00:10:39Z
- importance: 0.85

```text
v0.5.0 released 2026-10-24, Phase 3 complete: text in and out across Python (set_embedder, remember_text, recall_text), CLI (remember/recall --text) and MCP (serve --embed-cmd); embedder identity checked against the manifest; all-zero vectors rejected; embed-cmd runs as a persistent child with real timeouts and kill-and-respawn, so a vector can never be mismatched.
```

## 01M6ZT3F2MBW88M1F8ZXQBXBNR

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T09:26:50Z
- importance: 0.7

```text
mnemo-uniffi (MOBILE-1a, 2026-11-09) declares rust-version 1.82; the lockfile holds toml_writer and toml_parser at 1.0.2 (via uniffi_macros) because newer releases need edition 2024. Re-pin after cargo update with CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback cargo update -p toml_writer -p toml_parser.
```

## 01M6ZT3KQP6A1QG367YD3MP9Y8

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T09:26:54Z
- importance: 0.6

```text
cargo deny must run with --workspace (CI does since MOBILE-1a); without it only the root mnemo-engine package was checked, so bindings' licences (uniffi MPL-2.0, libloading ISC) were never seen.
```

## 01M6ZXSHKSENRQ8T7HVSDHCBGW

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T10:31:19Z
- importance: 0.7

```text
Swift package (MOBILE-1b): each file's Keychain key id is a random UUID stamped on the .mnemo file as the xattr io.github.toarchkumar.mnemo.key-id (not in the manifest, which is encrypted with that key); if the xattr is lost, MnemoStore tries each stored key with a read-only raw-key open and re-stamps.
```

## 01M6ZXSP85KXHVN4GKDB8HSZRN

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T10:31:24Z
- importance: 0.7

```text
Swift package iOS tests must run inside the XcodeGen host app (mnemo/bindings/swift/TestHost, xcodegen generate then xcodebuild test -scheme MnemoHostTests); the bare SwiftPM xctest runner gets errSecMissingEntitlement (-34018) from the Keychain, and the simulator reports no file protection class.
```

## 01M70K1BH0R44WTVCW7R1SVXXG

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T16:42:35Z
- importance: 0.8

```text
MOBILE-1c Kotlin key id: the key id is stamped on the .mnemo file as the xattr user.io.github.toarchkumar.mnemo.key-id (android.system.Os on API 26+, NIO UserDefinedFileAttributeView on the JVM), best effort; API 24-25 and file systems without user xattrs fall back to trying each stored key with a read-only raw-key open, then re-stamp. Keys are wrapped by one AndroidKeyStore AES-256-GCM key in noBackupFilesDir/mnemo/keys/<id>.key with the id as AAD.
```

## 01M70K1G6RKC0FME6DCK8R2WEX

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T16:42:40Z
- importance: 0.8

```text
UniFFI 0.32 Kotlin needs renames in mnemo/bindings/kotlin/uniffi.toml (global config format: [crates.mnemo_uniffi.bindings.kotlin.rename]): MnemoDb.close becomes closeFile because it clashes with AutoCloseable.close, and error fields named message become reason because they clash with Throwable.message. Add a rename there for any new clash.
```

## 01M70K1MM7YHSR6DWVG8T43YD7

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T16:42:44Z
- importance: 0.6

```text
Android instrumented tests: ActivityScenario.moveToState(CREATED) stops an activity by starting another on top, so ProcessLifecycleOwner never sends ON_STOP; close the scenario to put the app in the background (MOBILE-1c, 2026-11-09).
```

## 01M70V5EFQ970ZZQC5VK03SVZD

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T19:04:38Z
- importance: 0.7

```text
App Groups work with a free Apple ID (Personal Team): Apple's Supported capabilities (iOS) table ticks App groups for the free 'Apple Developer' column (checked 2026-11-09). The group id must be unique to the team, so the iOS sample derives it from the bundle id (group.<bundle id>).
```

## 01M70V5JZNEGANQZSTF5D5CB93

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T19:04:42Z
- importance: 0.7

```text
The mobile UniFFI bindings expose no build_index and no essence writes (essence_add / timeline): mobile recall reads the vector column without IVF+PQ, and a thread's essence read from Swift/Kotlin stays empty. Found while building the MOBILE-1d samples, 2026-11-09.
```

## 01M70V5QDTX8ZZXSBE466TFJBC

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T19:04:47Z
- importance: 0.6

```text
MediaPipe Text Embedder doesn't fit a 10 MB Android budget: tasks-text 1.2.2 ships a 16.6 MB arm64 libmediapipe_tasks_textgenai_jni.so plus litertlm; 0.10.20's 7.7 MB lib has 4 KB LOAD alignment (fails 16 KB pages). The Android sample uses a labelled demo embedder (2026-11-09).
```

## 01M7169RNX2VJBEFP2H4JSZHVM

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T22:19:13Z
- importance: 0.8

```text
Swift MnemoStore (MOBILE-1d.2): while the app isn't active (didEnterBackground; also UIApplication's state at start for background launches), every call runs inside an expiring activity and flushes+closes the file before it ends; willEnterForeground/didBecomeActive restore keep-open. Extensions are treated as active. Store methods touching the file are async.
```

## 01M7169XPTTQMAQ1AV33BZ8WZN

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-09T22:19:19Z
- importance: 0.7

```text
JNA's Android AAR ships libjnidispatch.so for 7 ABIs (incl. armeabi, mips), but mnemo-android ships libmnemo_uniffi.so for arm64-v8a and x86_64 only; apps (and the sample) need ndk abiFilters arm64-v8a,x86_64 or they install on unsupported ABIs and fail at first call. Found 2026-11-09.
```

## 01M71DF46NJ8HSV18W9HY9283F

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-10T00:24:29Z
- importance: 0.8

```text
2026-11-09 iPhone device run (iPhone18,3, iOS 27.0.1, sample Release): 10K disk 1329.6 B/mem, handle 7.3 MB phys_footprint, raw-key open 2.11 ms, warm recall 1.72 ms pass the phone targets; first recall 7.08 ms misses the 5 ms target (M1 Pro 8.77 ms; vector-column decrypt dominates). Both iOS release gates passed: no 0xdead10cc in either background mode, protection class completeUntilFirstUserAuthentication.
```

## 01M71DF9MBH65NXSY7TY5AGS74

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-10T00:24:35Z
- importance: 0.7

```text
NLEmbedding.sentenceEmbedding has no download API: it returns nil when the LinguisticData asset isn't on the device (seen on a real iPhone and the simulator). NLContextualEmbedding (iOS 17+) has hasAvailableAssets and requestAssets() to download on demand (English: 512 dims, max 256 tokens; mean-pool token vectors for a sentence vector).
```

## 01M722VS72AWM064KZFH39R4CN

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-10T06:38:24Z
- importance: 0.8

```text
On GitHub's macOS runners, xcodebuild hung for 10+ minutes resolving a SwiftPM package with a binaryTarget from a GitHub Release, right after 'Checking out'; -packageAuthorizationProvider netrc (plus -scmProvider system, GIT_TERMINAL_PROMPT=0) fixed it (resolve 13 s). The same command takes seconds locally. Keep that flag on any xcodebuild that resolves remote packages in CI.
```

## 01M722VY3KYRR01P89T00G4RVB

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-10T06:38:29Z
- importance: 0.7

```text
v0.12.1 (MOBILE-2b): the Swift and Kotlin stores default to FlushPolicy coalesced (1 s, 64 writes); manual is the pre-0.12.1 behaviour and is what the sample Benchmarks use. On Android a coalesced write made while the process is stopped commits before it returns.
```

## 01M722W2SXP6JF878MMYVHFY1J

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-10T06:38:34Z
- importance: 0.8

```text
SEC-2 (v0.12.1): Mnemo::rekey keeps the data key, so it doesn't revoke an old passphrase from earlier copies; mnemo rekey --rotate (rekey + compact_file) and Mnemo::export_rekeyed write a fresh data key. The mobile passphrase export and import go through export_rekeyed.
```

## 01M722W7A6WPPBHXVH1VBK85E2

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-10T06:38:38Z
- importance: 0.6

```text
2026-11-09: git fetch failed with 'bad object refs/remotes/origin/main 2' because .git held sync-conflict copies ('main 2', 'index 2'), likely from a file-sync tool on Documents. Moving them out of .git fixed it. If a ref name has a space and a number, look for sync duplicates.
```

## 01M73YE7F8G31JFQ6074FA91PD

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-10T23:59:34Z
- importance: 0.7

```text
2026-11-10: v0.12.1 shipped (tag at 6e32b68, run 42919069975). The first release through environments with a required reviewer: crates.io published by Trusted Publishing (OIDC), mnemo-swift by the deploy key, Maven Central by step-scoped repository secrets. Central reached repo1 about 15 min after the upload.
```

## 01M7BAHEFHVHEGST668MNWD0J3

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:45:44Z
- importance: 0.85
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
The MNemo team runs as four Claude Code sessions in tmux on the user's M1 Pro Mac, each with Remote Control: planner-a (the user's control center), worker-a, reviewer-b, and reviewer-a when called before roadmap phases.
```

## 01M7BAHKS1WRQ93JW8DT5TM74F

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:45:49Z
- importance: 0.9
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Sessions send each other pointer messages only (DONE:, BLOCKED:, READ:, REVIEW DONE:, NEEDS-USER: followed by a ledger entry's title); the instructions live in the ledgers: PLAN.md's WORKLOG, reviewer_notes.md and SECURITY.local.md.
```

## 01M7BAHRZ46KRY3W8P63M61K2S

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:45:55Z
- importance: 0.85
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
The Worker is the sole committer. The Planner plans and coordinates from planner-a but never builds, tests, edits code, commits or pushes.
```

## 01M7BAHYS59KW9FC993B7T3FHQ

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:00Z
- importance: 0.9
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
What stops for the user: starting a new roadmap phase or minor release, anything marked 'the user decides', a change to the roadmap order, an off-cycle review, a change to AGENTS.md's protocol text, passphrases, tokens, registry or GitHub settings, the release approval click, and anything irreversible the protocol doesn't cover.
```

## 01M7BAJ3P2CHPZBDNSAR6RBQFX

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:06Z
- importance: 0.7
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Each step gets at most three fix rounds (a review asking for changes, then the Worker's DONE, is one round); after the third the Planner logs NEEDS-USER instead of a fourth.
```

## 01M7BAJ8MG396V6YBWK05ZFX4K

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:11Z
- importance: 0.75
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Only one session writes PLAN.md at a time: the Worker appends only at its DONE or BLOCKED and then sends the pointer; the Planner appends only after reading a pointer and before sending the next.
```

## 01M7BAJDFTQ4M9NW3QEN98VGMQ

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:16Z
- importance: 0.85
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
The user's working style: plain English, short status, no recaps of steps. Verify claims in the code, on GitHub and on the registries before stating them, and search the web before stating product facts.
```

## 01M7BAJJ4PSJTRB4XPN5KA38MY

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:20Z
- importance: 0.7
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
The user decides the roadmap order and anything marked 'the user decides'; agents raise those briefly, with a recommendation.
```

## 01M7BAJPTQSZ8K1P9KRGQ2N9WX

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:25Z
- importance: 0.7
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
tmux hygiene: detach with Ctrl-b then d (never exit) to keep a session alive; run caffeinate -dims in its own window while agents work, since a closed lid sleeps the Mac unless a display is attached; /remote-control re-links a session to the phone; start every session in mnemo.nosync, because a conversation is saved under the folder it started in.
```

## 01M7BAJVFPG4BBKT5E9EWCJ3QE

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:30Z
- importance: 0.9
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
The repo lives at ~/Documents/GitHub/mnemo.nosync. It was renamed out of iCloud's reach on 2026-11-12, after stale 'name 2.ext' sync copies tripped the Worker twice.
```

## 01M7BAK02TP4JA6XVCP469B4J9

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:35Z
- importance: 0.75
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
The old ~/Documents/GitHub/mnemo folder is stale: it reappeared once as iCloud placeholders and was deleted. If it shows up again, touch nothing in it.
```

## 01M7BAKBHY1N5D350PW4P0C698

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:46Z
- importance: 0.7
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
To rebuild test/.venv: Python 3.11, pip install -r test/requirements.txt, then maturin develop --release in mnemo-python with the venv active.
```

## 01M7BAKH5HZJXSTP90PVWF7J3M

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:52Z
- importance: 0.5
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
test/project.mnemo was created before v0.14 made new brain files owner-only (mode 0600); the user may chmod 600 it.
```

## 01M7BAKPACETQ0PHKF4MTCGG8F

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:46:57Z
- importance: 0.6
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
How to check things: GitHub through its API with curl or gh (PRs, check runs, workflow runs, job annotations); crates.io and PyPI with curl; mnemo-swift with git ls-remote. Maven Central can't be reached from the Planner's tools, so the Worker's check is accepted there.
```

## 01M7BAKVE4R4JAZTYQKKAEFC01

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:03Z
- importance: 0.8
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Decision 2026-11-13: a CI job cancelled with 0 steps and the runner-acquisition annotation is 'not run', not a pass and not a failure. It doesn't block a merge, but every job must be green on the release branch's final push before the READY.
```

## 01M7BAM1BCZD6SPY02KSB5DZXB

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:09Z
- importance: 0.55
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Review triage edits the Decision Ledger row and the item's Planner Triage and Status lines with a small script, never by retyping long text by hand.
```

## 01M7BAM6J1VY0B0MG5E9DK4ZHW

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:14Z
- importance: 0.8
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Security findings stay in the gitignored SECURITY.local.md, with neutral commit and PR wording, until their fix ships; then they move into PLAN.md.
```

## 01M7BAMBBPQ4MCNFCFMYFBY1QY

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:19Z
- importance: 0.9
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
A release needs one human approval: the approve-release job on the release environment, whose reviewer is toarchkumar. The four registry environments have no reviewer and hold the secrets.
```

## 01M7BAMFY3F18CSSY5TQDK63GA

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:24Z
- importance: 0.8
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
The mobile release mode comes from .github/release-mobile in the tagged commit; ENABLE_MOBILE_PUBLISH is only an off switch. Mobile publishes at least once per minor version, so v0.14.0 publishes mobile.
```

## 01M7BAMMW3KDDRM5FW98FVRJD1

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:29Z
- importance: 0.75
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Release credentials: Trusted Publishing for crates.io and PyPI, and a deploy key for the mnemo-swift mirror. The Maven Central user token renews by 2027-11-09, and the GPG key expires before 2028-11-08.
```

## 01M7BAMYJEB8SCW8TDFKAFMQ0H

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:39Z
- importance: 0.6
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
REKEY-2 covers RB-005.2 recommendation 1, RB-005.3 recommendation 2 and RB-005.4 item 5 (see the 2026-11-12 entries in PLAN.md).
```

## 01M7BAN3EGZFK4G1KXV7ZQYRXW

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:44Z
- importance: 0.7
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
The v0.14.0 release-prep HANDOFF (PLANNER-A) covers the root README (the shared-mode lines, the kept-secret line, the file-mode line, the upgrade note), index.html (the version pill, Shipped, Next), and setting .github/release-mobile to true.
```

## 01M7BAN8A78XX4QE89AZKGT3SW

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:49Z
- importance: 0.7
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
After v0.14.0 the roadmap goes on with 12.3. That's a phase start: NEEDS-USER for the go, and REVIEWER-A before it.
```

## 01M7BANDA2727AFBW3ZZVA4QFP

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:54Z
- importance: 0.6
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Step 7 of 12.4 (presence and claims): the sidecar reopen must also pick up a sidecar made after a handle started (step 3's finding), along with RB-006.5's rework.
```

## 01M7BANJ5AND4Y2K8M552FCM3K

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:47:59Z
- importance: 0.6
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Step 8 of 12.4 (shared mode as the MCP default): first decide whether to gate pre-0.14 binaries by format version.
```

## 01M7BANQ1VDM4SRKERNKDXA46R

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:48:04Z
- importance: 0.55
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Step 9 of 12.4 (the optional daemon) starts from the trust requirements in REVIEWER-B's RB-006 review.
```

## 01M7BANVWZ3YPE03VD7MCA8S6W

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:48:09Z
- importance: 0.55
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
CLEANUP-1 candidates: raise the MSRV to 1.85 (which drops the Windows rename fallback and refusal), and call GetDriveType through a small helper outside the forbid(unsafe_code) crate.
```

## 01M7BAP0R9VQJ8AGJQYPPPGM95

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:48:14Z
- importance: 0.85
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
DOGFOOD-0 is the first step toward HARNESS-DOGFOOD-1. The user's first-success milestone is this project's git Markdown workflow running entirely in MNemo.
```

## 01M7BAP5MWJG9N2PEA19E0FV7R

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:48:19Z
- importance: 0.8
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
After DOGFOOD-0, a new session runs python test/scripts/project_memory.py recall "<topic>" next to reading PLAN.md's handover, and notes in its WORKLOG entries where the brain helped or fell short. Those notes feed 12.2, 12.3 and 13.x.
```

## 01M7BAPAEDX4QTANT7WVWPN6Y1

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:48:24Z
- importance: 0.9
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
User decision 2026-11-07: the target market is on-device agents and apps, from desktop apps down to tablets, phones and other small devices, not hyperscale enterprise. Optimise size and speed first.
```

## 01M7BAPF6TTAAY8ZSTT498GDJJ

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:48:28Z
- importance: 0.75
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
Every spec should say how it affects disk per memory, RAM per open handle, open time and recall time. The targets are in 'On-device track' under PLAN.md's execution order.
```

## 01M7BAPKY7BC1XK9YJ6E6A8QQC

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:48:33Z
- importance: 0.75
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
User decision 2026-11-12: one Planner (PLANNER-A), one Worker, and two Reviewers at milestones (REVIEWER-A for product direction, REVIEWER-B for code health). PLANNER-B and the backup Worker are retired.
```

## 01M7BAPRNRVFC01NGT33ABPYEB

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-13T20:48:38Z
- importance: 0.7
- metadata: `{"area":"handover","source":"PLAN.md handover refresh 2026-11-13"}`

```text
PLANNER-A's handover refresh of 2026-11-13 sits at the top of PLAN.md's 'Planner handover' section; read it first.
```

## 01M7C5ZXYMYZNWCMBX182QDRQR

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T04:45:30Z
- importance: 0.85

```text
v0.14.0 ships 12.4 shared mode (steps 1 to 5): Mnemo::open_shared in Rust, mnemo_engine.open(..., shared=True) in Python, and mnemo serve --mcp --shared. Each call is one read or one write window under the file's lock, holding nothing between calls, and catches up with other processes' commits first. Off by default; Swift and Kotlin don't have it yet.
```

## 01M7C607PDGMNQ7AZMY11N3ZRB

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T04:45:40Z
- importance: 0.75

```text
MnemoError::Replaced (v0.14.0) means nothing was written: another file is at the handle's path, or the brain was moved or removed (missing: true). In a shared window (shared: true) calling again is safe and the next call follows the new file. Python raises ReplacedError, a MnemoError subclass with a shared attribute. ReplacedDuringWrite is different: the write may be in the replaced file.
```

## 01M7C60CE6AZA1RFDW0YG312QD

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T04:45:45Z
- importance: 0.8

```text
Hook --wait rule (RB-008.1, v0.14.0): a session hook that only reads passes --wait 2s, so a busy brain delays a session's start by at most about two seconds; a hook or script that saves keeps the default 10 s wait. project_memory.py's shared writes use the default.
```

## 01M7CM0JAJK2R9GS8WSTAZN7WT

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T08:50:31Z
- importance: 0.6

```text
v0.15.0 step 1 (PR 78, 2026-11-14): CLI and Python tests that spawn mnemo must clear MNEMO_PASSPHRASE, MNEMO_KEY*, MNEMO_DEV_KEYSTORE and the agent markers CLAUDECODE and CLAUDE_CODE_CHILD_SESSION from the child's environment, because an agent harness sets CLAUDECODE in every shell it runs and the recovery-code display rule refuses under it. The Python test wheel needs --features dev-hooks (MNEMO_DEV_ANN_MIN_FACTS), as CI builds it; rebuild test/.venv without it afterwards.
```

## 01M7DE3M02XJZP9ZDJ3JG3XPC1

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T16:26:34Z
- importance: 0.8

```text
Every v9 build before v0.15 (v0.12 to v0.14) refuses to open a file whose BM25 manifest tokenizer byte isn't 1 (load_bm25_manifest runs on every open), so v0.15 keeps that byte at 1 and records keyword tokenizer 2 as a reserved mark term at the start of each segment's dict (posting document 0), which older builds' writes and merges drop.
```

## 01M7DE3RXR2TT1TTYNY40SDV93

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T16:26:39Z
- importance: 0.7

```text
On 2026-11-14 (v0.15 step 4) hybrid recall by default was measured on this brain: at keyword weight 0.5 the question's common words (how, the) pulled memories with similarity 0.17-0.18 into second place; at 0.25 the right fact led all three DOGFOOD-0 questions by 0.26-0.42, so the text's own keyword weight defaults to 0.25 and explicit keywords stay 0.5.
```

## 01M7DE3XKSFNRFSM5JW2QBG8CJ

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T16:26:44Z
- importance: 0.6

```text
Clippy in CI runs with --features failpoints,benches,keychain; a struct change must be checked with the benches feature too, since benches/ only compile with it (v0.15 step 4's first CI run failed on two bench files).
```

## 01M7DQC7VDXJ34MXYQFPEDCBJ2

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.7
- metadata: `{"area":"decisions"}`

```text
2026-11-14, PLANNER-A (the keys note's sign-off): a recovery code is shown only when stdout and stdin are terminals and no known agent-harness marker is set, starting with CLAUDECODE; the markers are one list in keys, each checked against the vendor's docs, and a marker only makes the rule stricter, so an agent's shell never sees the code.
```

## 01M7DQC7WYAFZBVJWQHYJFXZHT

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.7
- metadata: `{"area":"decisions"}`

```text
2026-11-14, PLANNER-A: LOCK-2's threshold T = 500 ms, chosen by the sweep rule: the largest T that keeps the writer test's 1 s bound, no Locked at 8 and 16 cheap readers, and readers' p99 within about 50 ms at 1 writer and 7 readers. A reader past T stops deferring to the valve (round 2) and retries every 1 to 2 ms (round 3), so writers can't starve readers.
```

## 01M7DQC7XSJ1A4H24KTW03H2JW

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.7
- metadata: `{"area":"decisions"}`

```text
2026-11-14, the user: ship LOCK-2 as round 3 built it (with many writers saving back to back on a slow machine a read can wait up to about 2 s; the eight-writer test asserts progress and a bounded wait, not a rate), and plan SNAP-1, reads never wait for saves (readers read the last committed snapshot), for a later release.
```

## 01M7DQC7YDGBH1CKZVXEA9838P

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.8
- metadata: `{"area":"decisions"}`

```text
2026-11-14, PLANNER-A (TOK-1, option 2, then the Worker's per-segment mark accepted): the BM25 manifest's tokenizer byte stays 1, because v0.12 to v0.14 refuse to open any other value, and keyword tokenizer 2 is recorded as a reserved mark term in each segment, which needs no extra write and which older builds' writes and merges drop, so v0.15 can report a mixed index.
```

## 01M7DQC7Z090C4SV7YN1NWRC57

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.7
- metadata: `{"area":"decisions"}`

```text
2026-11-14, PLANNER-A accepted the Worker's measurement: in a recall by text the question's own words get keyword weight 0.25 (DEFAULT_TEXT_KEYWORD_WEIGHT) and explicit keywords keep 0.5, because at 0.5 common words such as how pulled memories with similarity 0.17 to 0.18 into second place on this brain.
```

## 01M7DQC7ZN45QWFKH68AWF5JG7

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.6
- metadata: `{"area":"decisions"}`

```text
2026-11-14, PLANNER-A accepted: keyword tokenizer 2 indexes no part that is only digits (v0.9.0 gives v0, not 9 or 0), and counts each distinct term once per whitespace run (R-008.2), so versions and dotted names don't inflate matches.
```

## 01M7DQC80ACD2EEBX4AGSDVG0K

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.7
- metadata: `{"area":"decisions"}`

```text
12.3 step 3's spec (R-007.3), as built in v0.15: mnemo mcp install writes no secret, no absolute path, no --embed-cmd and no key command into a project-scoped file, always serve --mcp --shared, masks env values in its diff, and offers to move a MNEMO_PASSPHRASE into the keychain; a project scope uses mnemo from the PATH and a project-relative path, ${CLAUDE_PROJECT_DIR:-.} for Claude Code.
```

## 01M7DQC81393X7TYG0HYDF6HS7

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.8
- metadata: `{"area":"decisions"}`

```text
2026-11-14, the user: DOGFOOD-0's read side starts. Each Worker dispatch begins with a read-only project_memory.py recall of the task and logs Brain: hit, partial or miss in its next entry; each DONE stores every user and Planner decision since the last DONE as one episodic fact with area decisions. The ledgers stay the authority.
```

## 01M7DQC81Q9ZXXTY5VZTHVDRKN

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.5
- metadata: `{"area":"decisions"}`

```text
2026-11-14, PLANNER-A: the Ubuntu failures of cli_shared::the_cli_embeds_before_it_takes_the_lock in v0.15 step 4 (a shared write of 740 ms against a 500 ms bound, the lock not held) are treated as runner timing for now, with a failure message that splits the time; it comes to READY if it fails again.
```

## 01M7DQC82APB5YCEK9FTTCWEYF

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.6
- metadata: `{"area":"findings"}`

```text
v0.15 step 5a found that an optional dependency the build doesn't enable isn't parsed by Cargo: with fastembed (which needs Rust 1.92) in the lockfile but its feature off, the graph still builds on Rust 1.75 (lockfile version 3). The MSRV is 1.92 because the release builds will turn the embedder on.
```

## 01M7DQC82Y4TAX8XPTAE46AJ32

- type: semantic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.7
- metadata: `{"area":"findings"}`

```text
keyring 3.6.3 and the keyring 4 stores name entries the same way: the macOS user keychain's generic password (service, account), Windows Credential Manager's {account}.{service} in UTF-16, and Secret Service items found by service and username; tests/keychain_os.rs checks both directions in one binary, since on macOS a second binary reading an item waits on an access prompt and hangs a headless CI job.
```

## 01M7DQC83N8DH69R41EMSBY3VT

- type: procedural
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:08:34Z
- importance: 0.7
- metadata: `{"area":"findings"}`

```text
Raising rust-version turns on clippy lints that it held back (is_none_or, is_multiple_of, as_chunks), and CI's stable clippy can be newer than the local one: lint with the same version as CI (rustup run <version> cargo clippy, installed beside the default) before pushing a toolchain change.
```

## 01M7DRMHEHYXZCX8TJCCD0R1VC

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T19:30:34Z
- importance: 0.7
- metadata: `{"area":"findings"}`

```text
v0.15 step 5's model comparison (2026-11-14, 30 questions on this brain, M1 Pro): every model found every expected fact in its top 5. Hit@1 hybrid / vector-only: the brain's own sentence-transformers MiniLM 29/27, fastembed bge-small-en-v1.5 26/28, snowflake-arctic-embed-xs 27/29. Arctic-xs was the fastest (cold remember 0.64 s, 24 ms a memory) and smallest (91 MB) of the fastembed models; the text's 0.25 keyword weight cost both new models two hits at rank 1; granite-embedding-small-english-r2 has no first-party ONNX export; ONNX Runtime adds about 33 MB unstripped (24 MB stripped) to a macOS arm64 binary and has no x86_64 macOS build.
```

## 01M7DW5XDVSDBKP8NTW3DBZ8M9

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T20:32:29Z
- importance: 0.8
- metadata: `{"area":"decisions"}`

```text
2026-11-14, the user (step 5's runtime): measure a pure-Rust runtime (candle) for the built-in embedder first, and fall back to ONNX Runtime through fastembed only if it fails the budget, because part 1 found ONNX Runtime over 30 MB as built on macOS, with no Intel Mac build and a 4.1 GB peak.
```

## 01M7DW5XF03NFA8GGHQTCTS1H0

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T20:32:29Z
- importance: 0.8
- metadata: `{"area":"decisions"}`

```text
2026-11-14, the user (step 5's model): compare snowflake-arctic-embed-xs with all-MiniLM-L6-v2 head to head before picking; bge-small and granite are out. arctic-xs is built on all-MiniLM-L6-v2 (same BERT shape, tokenizer and size), so only the retrieval training differs.
```

## 01M7DWGWK765X6TNE41Z8W4TGJ

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T20:38:29Z
- importance: 0.8
- metadata: `{"area":"findings"}`

```text
v0.15 step 5 part 2 (2026-11-14, M1 Pro): candle (pure Rust) runs all-MiniLM-L6-v2 and snowflake-arctic-embed-xs with vectors matching the references (min cosine 0.999999) and adds 4.1 to 5.3 MB stripped (4.5 to 6.0 MB unstripped) over quickstart on macOS arm64 and x86_64 and Linux x86_64 and aarch64, against ONNX Runtime's 24 to 33 MB on macOS arm64; a cold remember takes about 85 ms and 207 MB peak, a re-embed of the brain 3.4 s and 0.7 to 0.8 GB, with no build-time download and no new C. The tokenizers crate is 2.6 MB of that. On the 30 questions arctic-xs leads vector-only (29/30 at rank 1, MRR 0.983) and MiniLM needs keyword weight 0.1 to match it (29/30, MRR 0.983).
```

## 01M7DXCXK0VT1GPD00CNGBDSFW

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T20:53:47Z
- importance: 0.8
- metadata: `{"area":"decisions"}`

```text
2026-11-14, the user: the built-in embedder's runtime is candle (pure Rust), settled by step 5 part 2's measurements (4 to 6 MB added on every desktop target, against ONNX Runtime's 24 to 33 MB on macOS).
```

## 01M7DXCXM6EBKG6F4TH77FBT6Q

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T20:53:47Z
- importance: 0.8
- metadata: `{"area":"decisions"}`

```text
2026-11-14, the user: the tokenizers crate ships with the embedder (pure Rust, fancy-regex only); EMBED-TOK, a minimal WordPiece tokenizer of our own, is carried as a later size cut, mainly for phones, since the crate is 2.6 MB of candle's growth.
```

## 01M7DXCXMY2CK07DN7NKH5P68G

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T20:53:47Z
- importance: 0.8
- metadata: `{"area":"decisions"}`

```text
2026-11-14, the user: no model pick yet between all-MiniLM-L6-v2 and snowflake-arctic-embed-xs; first a harder question set (step 5 part 3: ident, paraphrase, long, current and pair groups with mechanically checked rules), rerun on both through candle, because the first 30 questions were near the ceiling.
```

## 01M7DXCXNMB891C7T81DVFXGEF

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T20:53:47Z
- importance: 0.8
- metadata: `{"area":"findings"}`

```text
v0.15 step 5 part 3 (2026-11-14): on 31 harder questions through candle, arctic-xs led vector-only (hit@1 15 vs 13, MRR 0.577 vs 0.552) and MiniLM led with keyword weight (0.25: 0.572 vs 0.511; the text weight hurts arctic on paraphrases). arctic was far better on stale-versus-current facts (the stale fact won 1 to 2 of 5 against 4 of 5 for MiniLM); an exact identifier passed as an explicit keyword found its fact 10/10 for both; both missed the one long fact vector-only.
```

## 01M7DXW0AZMKGRQE3C3ZD0AHGY

- type: episodic
- agent: `cursor-agent`
- scope: private
- created: 2026-11-14T21:02:02Z
- importance: 0.8
- metadata: `{"area":"findings"}`

```text
v0.15 step 5 part 3b (2026-11-14): arctic-xs identifier-aware passed every line of the pass bar against its best fixed mode: hard set hit@1 16 vs 15 and MRR 0.608 vs 0.576, original set 29 vs 29 and 0.983 vs 0.983, ident 10/10 in the top 5, paraphrase hit@1 5 as vector-only. The rule found identifiers in all 10 ident questions plus false positives (macOS, MediaPipe, PLAN.md, and the plain words after --rotate and --wait); on h26 the identifier compact_file lifted the stale fact over the current one.
```
