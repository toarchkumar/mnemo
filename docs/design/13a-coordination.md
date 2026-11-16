# 13a Coordination: design note

**Status:** draft for sign-off (Worker, 2026-11-15; v0.17.0 step 0). No code
in this step. **Revised** (round 1, the same day) for PLANNER-A's six
changes: the manifest's default agent, the per-turn hook's one write, brain
text in the hook's line and never in a pointer, the hop count across fresh
messages, what step 1 can test, and how a woken session sees the pointer.
**Revised again** (v0.17.0 step 2) for REVIEWER-A's REV-010, as PLANNER-A
triaged it:
- claims and presence change in place (R-010.8);
- acked messages are pruned (R-010.8);
- a task whose owner lapsed is reclaimed (R-010.7);
- the per-turn hook is measured first, with a fast path if needed
  (R-010.6);
- `--as` and `--role` on the coordination commands, and the alias recipe
  (R-010.9);
- tasks and the inbox are presented before claims (R-010.3).

**Scope** (PLAN.md Phase 13, R-006.1's split):
- 13.1 messages and tasks, with 13.1b delivery;
- 13.3 claims and presence;
- 13.5 visibility;
- 13.6 freshness.

13b (13.2 key slots, 13.4 signatures, 13.7 merge) comes later. Everything
here runs on one machine, so it's free (PLAN.md's Monetization rule).

**Read for this note:**
- PLAN.md's Phase 13 specs and R-006.1, R-006.3 and their triage;
- `docs/design/12.4-shared-access.md`, the shared mode and its "Claims and
  presence" and "13.1b wake-ups" sections, which this note builds on;
- `docs/design/12.2-onboard.md`, the hooks and the 8,000-byte brief;
- 12.4's open design item (RB-006.12, held in `SECURITY.local.md`);
- the client docs under "Sources" at the end, read on 2026-11-15.

## What this decides

1. **Four new record kinds,** with no format change: `message`, `task`,
   `claim` and `presence`. Each is an ordinary record with reserved
   `_mnemo.*` metadata, and none is a recall hit.
2. **An agent names itself with `MNEMO_AGENT` and `MNEMO_ROLE`** in the
   environment of its session. Without them it's anonymous, and gets only
   what's addressed to everyone.
3. **The inbox is the source of truth, and wake-ups are hints.** A
   per-turn hook shows what's new. Each session's own MCP server wakes its
   idle session through Claude Code's cross-session socket. Claude Code
   channels are optional and unlikely to be available to us.
4. **Claims and presence ship as 12.4 designed them** (its step 7), with
   no lock held across a turn.
5. **Visibility is a filter, not a security boundary,** until 13b.
   `SECURITY.local.md` stays where it is.
6. **Freshness is two optional fields and per-kind defaults,** shown in
   `about`, the brief and `mnemo stale`.
7. **Three MCP tools** (`inbox`, `task`, `claim`), each with an `op`. The
   CLI, Python and Rust get the same operations.
8. **Ten steps for v0.17.0,** with a scoped REVIEWER-B pass after the
   steps that touch concurrency, or a process that pokes another one.

## 1. The records

All four kinds go through the existing record path: `put`, the catalog, the
WAL, snapshots and the 12.4 lock window. They're stored in the **shared
scope** (any agent reads them), carry the file's **sentinel vector** (the
one essence items and thread links use), and aren't embedded. So
coordination never needs the model, and a hook can do it.

**Never a recall hit.** `recall`, the ANN, BM25 and `about` leave the four
kinds out, as they leave out turns (R-004.2). Each kind is read through its
own API. A recall filter on `_mnemo.kind` doesn't bring them back: a
sentinel vector has no meaning to rank on, as the marker-only turns of
v0.16.1 showed.

**The MessagePack rule holds:** no persisted struct changes, and only new
reserved keys in `Memory.metadata`. Older builds carry them through
untouched. A v0.16 build reading a v0.17 brain sees them as ordinary
records. That's harmless, but they'd show up in its `list` and `export`. The
CHANGELOG says to upgrade every session's `mnemo` together.

### Messages (`_mnemo.kind = "message"`)

| Field | Where | What |
|---|---|---|
| subject and body | `content`, as `subject\n\nbody` | Plain text, at most 8 KB (a larger body is refused; put it in a fact and send its id in `refs`) |
| `_mnemo.to` | metadata | An agent id (`worker-a`), `role:<role>` (`role:worker`), or `all` |
| `_mnemo.from` | metadata | The sender's agent id (unproven until 13b, see section 2) |
| `_mnemo.refs` | metadata (exists) | Record ids, a PR number, a ledger entry title |
| `_mnemo.thread` | metadata (exists) | A conversation id, so replies share one thread and its hop count |
| `_mnemo.hops` | metadata | 0 on a new message, a reply's is its parent's plus one (the hop cap, section 3) |
| `_mnemo.delivery` | metadata | `sent → delivered → seen → done` (13.1b) |
| `_mnemo.status` and `_mnemo.status_history` | metadata (exist) | `unread → acked`, with the ack's note |

- **The two state fields change in place**, through the 12.7 status core's
  edit-in-place, the way essence items resolve. A message is one record for
  its life. Supersession isn't used: a message isn't a fact with versions.
- **`ack(id, note)`** sets `acked` and `done`. Reading the inbox sets `seen`.
- **Acked messages don't stay forever** (R-010.8): `compact` and `mnemo
  inbox --prune` drop a message acked more than 30 days ago. Its effect
  already lives in a task or a fact, and `export --git` never had it.
- **A message to a role** is acked by the first of that role to ack it. Each
  agent sees `seen` as it was set by anyone.

### Tasks (`_mnemo.kind = "task"`)

| Field | What |
|---|---|
| title and body | `content`, as for messages |
| `_mnemo.key` = `task:<slug>` | A keyed slot, so a re-add with the same slug is an update, not a duplicate. The slug comes from the caller or the title |
| `_mnemo.status` | `queued → active → ready → done`, or `blocked`; the back edges are `blocked → active` and, since REV-010, `active → queued` (an owner hands a task back, or someone requeues an abandoned one), in `TASK_TRANSITIONS` |
| `_mnemo.role` | The role that may take it (`worker`) |
| `_mnemo.owner` | The agent id that took it (set by `next`) |
| `_mnemo.after` | Task ids it waits for |
| `_mnemo.pre_approved` | Bool: the owner may go from `ready` to `done` without a sign-off |
| `_mnemo.delivery` | As for messages, so a new task wakes its role |

- **Status changes are in place, with history,** like messages. The keyed
  slot exists for the slug, not for versions of the body.
- **`next(role)`** returns the oldest `queued` task for the role whose
  `after` tasks are all `done`. It claims `task:<slug>` (section 4) and sets
  `active` and the owner, all in one exclusive window.
- **A task whose owner's claim lapsed is taken back** (R-010.7). After the
  ready `queued` tasks, `next` also takes an `active` task whose
  `task:<slug>` claim has lapsed: a new claim, a new owner, and a
  `status_history` entry naming the old owner, in the same window.
  - Otherwise a crashed or out-of-usage worker would leave its task stuck
    in `active`, which nothing could take.
- **A `task update` from an owner whose claim is lost is refused** with
  `Lost`, so a worker that wakes up after its task was reclaimed can't
  overwrite the new owner's work. Step 4 has a crash-and-reclaim test.
- **`ready` waits for the task's creator to set `done`,** unless
  `pre_approved`. That's today's "READY before tagging".

**As built (step 4):**
- **The slug is `_mnemo.slug`, not a `_mnemo.key` slot.** It's found
  through the row hash, as claims are (section 4's "As built"), so a task
  never enters the keyed index or `history --key`. A re-add with the same
  slug updates the title, body, role, `after` and `pre_approved` in place.
- **Who may do what:**
  - its owner moves it from `active` or `blocked`, and adds notes;
  - its creator sets `done` and adds notes at any status;
  - anyone requeues an abandoned task;
  - `queued → active` happens only through `next`.
- **An owner's update** renews its claim:
  - with a token, the claim must be that token's;
  - an owner whose claim lapsed with nobody else taking it carries on,
    and takes the claim again;
  - `ready`, `done` and `queued` release it.
- **`after`** names slugs, which must exist when the task is added.

### Claims and presence

As 12.4 designed them ("Claims and presence", with its step 7 rework), with
one change from REV-010 (R-010.8):
- a claim is one record per topic, `claim:<topic>` in the shared scope,
  holding the holder, `expires_at`, the lease and the **token**
  (`_mnemo.claim_token`);
- presence is one record per agent, `presence:<agent>`, holding the role,
  the tool and `last_active`, plus a sidecar lock for `alive`;
- **both change in place,** not by keyed supersession. A keyed write keeps
  every superseded version for `history --key`, and a heartbeat once a
  minute would leave about 10,000 versions per agent per month that nobody
  reads. A claim's new holder gets a new token in the same record, so
  `check`, `renew` and `release` still see `Lost`.

Section 4 has the TTLs.

### How each survives `export` and the inspector

| | `export` (Markdown, JSONL) | `export --git` | `inspect` |
|---|---|---|---|
| message | Yes, with its fields | **No:** it's a channel. What it decided lands in a task or a fact | Yes, under its kind, read-only |
| task | Yes | **Yes, every status but `done` older than 30 days,** ordered by slug. That's R-006.2 (d)'s "active tasks" | Yes |
| claim | No (it lapses) | No (12.4) | Yes, with its expiry |
| presence | No | No | Yes (`who`) |

The Markdown export adds a section per kind. `--git` stays deterministic,
since a task's fields are all in the record. The inspector needs no new
view to stay correct, because it shows every record by kind. Its Phase 13
views (14.2) come later.

## 2. Agent identity on one machine

**How a session names itself:**
1. **`MNEMO_AGENT`** (an id, `[A-Za-z0-9._-]{1,64}`) and **`MNEMO_ROLE`**
   (`planner`, `worker`, `reviewer`, or any word), in the session's
   environment. The MCP server and the hooks are both children of the
   client, so both inherit them.
   - **Checked for Claude Code's MCP server (2026-11-15, by hand):** on this
     Mac, each running `mnemo serve --mcp` (a child of `claude`) had the
     tmux pane's own variables (`TMUX`, `TMUX_PANE`) and Claude Code's
     (`CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_MESSAGING_SOCKET`,
     `CLAUDE_CODE_MESSAGING_TOKEN`) in its environment. I checked the names
     with `ps eww`, never the values. So a variable exported in the pane
     before `claude` reaches the server.
   - **Still to check by hand in step 1,** and recorded in its DONE: that
     hook commands get the same environment (a one-off `env | grep MNEMO`
     from a hook's shell), and the same for Cursor. **CI can't check this,**
     since it has no Claude Code or Cursor. CI covers the CLI, the server
     and the hooks reading `MNEMO_AGENT` and `MNEMO_ROLE` when they're set.
   - If a client doesn't pass them, `onboard` can't fill the gap, since one
     project's config is shared by every session in it. The fallback is
     below.
2. **Without them,** the session is `claude-code:<session id>` (the hook
   input's id, as the thread is) with no role. It gets messages addressed to
   that id or to `all`, and nothing addressed to a role. `who()` lists it as
   anonymous.
3. **`onboard` doesn't write an identity.** The config is per project, and
   several sessions share a project. Its summary gets a line:
   "to coordinate agents, start each session with `MNEMO_AGENT=<name>
   MNEMO_ROLE=<role>`".

**This repo:**

| Session | `MNEMO_AGENT` | `MNEMO_ROLE` | How |
|---|---|---|---|
| worker-a | `worker-a` | `worker` | `export` in its tmux pane before `claude` |
| planner-a | `planner-a` | `planner` | the same |
| reviewer-b | `reviewer-b` | `reviewer` | the same |
| reviewer-a (Antigravity) | `reviewer-a` | `reviewer` | the CLI with the variables set; no hooks, so no wake-ups (tmux rule 9's exception) |

**What an unproven sender allows.** Until 13b's per-agent keys, everyone
holding the brain's key can write any `_mnemo.from`. So:
- a message is information, never authority. Section 3's rule applies
  whoever it says it's from;
- `from` is shown as written, labelled "(unverified)" wherever a person
  reads it (the inspector, `inbox --format table`);
- nothing in the engine checks `from` before it acts (no "only the planner
  may add tasks"). Role rules stay in AGENTS.md, as today.

**AGENT-DEFAULT** (carried 2026-11-15). Two different things share the word
"agent":
- the **owner** of a fact (`Memory.agent_id`, which decides a key's Private
  slot);
- the **coordination identity** (`MNEMO_AGENT`).

**Recommendation:**
- **The owner default comes from a reserved manifest key,
  `_mnemo.default_agent`.** When the manifest has it, the CLI and the MCP
  server use it for a write that names no agent, instead of `default`.
  - The engine never reads a project's own metadata keys. This brain's
    `agent_id_default` is a convention of this repo's `seed.json`, so it
    stays as it is, and means nothing to the engine.
  - **`Memory::scaffold_manifest` leaves the key unset,** so new brains
    keep `default`.
  - The key is reserved, so `update --metadata` can't set it. Step 1 adds
    `mnemo default-agent [<id> | --clear]` and `Mnemo::set_default_agent`,
    an in-place edit of the manifest through the same path as `update`.
  - **Step 1 sets it once on this brain** (`mnemo default-agent
    cursor-agent`), and `seed.json`'s manifest gets it too. That ends this
    repo's "name the agent" rule.
- **The coordination identity never changes a fact's owner.** A fact gets
  `_mnemo.author` = `MNEMO_AGENT` when it's set (the field 13.4 will sign),
  so "who wrote this" is answerable now, unproven, without splitting key
  slots by session.

## 3. Delivery (13.1b)

**The rule: the delivery state is the truth, and every wake-up is a hint.**
A hint that goes nowhere loses nothing. The next per-turn check, or the next
session start, shows the item.

### What the clients offer today

The client docs were read on 2026-11-15; they change, so the URLs are under
"Sources".
- **Claude Code hooks:**
  - `UserPromptSubmit` adds context to every turn, including "turns Claude
    Code starts on its own". Plain stdout or `additionalContext` count, up to
    10,000 characters. Its command timeout defaults to 30 s.
  - **`Stop` can keep a turn going:** `decision: "block"` with a `reason`.
    There's a cap of 8 consecutive continuations
    (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`) and a `stop_hook_active` flag.
  - **No hook fires on an idle session.**
- **Claude Code cross-session messaging** (`ListAgents`, `SendMessage`; v2.1.224+):
  - same-machine delivery over a per-session socket, "never through
    Anthropic servers", on every provider;
  - **"When the receiving session is idle, Claude Code starts a new
    turn with the message";**
  - the socket's path and token are in the session's environment
    (`CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`), and
    scripts may post to it. It's restricted to the OS user;
  - a message is framed as coming from another session, and can't approve a
    permission prompt. The receiver's `crossSessionInbound` setting
    (`accept`, `hold`, `refuse`) applies.

  This repo's sessions already use it, by hand.
- **Claude Code channels:**
  - research preview, opted in with `claude --channels plugin:<name>@<marketplace>`;
  - it needs a claude.ai login or a Console API key, and isn't on Bedrock,
    Vertex or Foundry;
  - **only plugins on an Anthropic allowlist** (or an org's
    `allowedChannelPlugins`) can register; others need
    `--dangerously-load-development-channels`;
  - a channel may relay permission approvals.

  So mnemo can't count on being a channel.
- **MCP notifications in Claude Code:** `list_changed` makes an interactive
  session refetch the list. `notifications/resources/updated` isn't
  documented, and nothing says a notification reaches the model or wakes a
  session.
- **Cursor hooks:** `sessionStart` adds context; `postToolUse` adds context
  after a tool result; `stop` can send a `followup_message` as the next user
  message (`loop_limit` default 5). `beforeSubmitPrompt` can't add context.

### The four paths, in order of what they're worth

1. **The per-turn check (every client with hooks; ships first).**
   - **`mnemo hook user-prompt --client claude-code`** on `UserPromptSubmit`,
     and on Cursor's `postToolUse` at most once a minute. It opens the
     brain read-only (2 s wait) and counts what's new for this agent:
     unread messages to it, its role or `all`, not yet `seen`; queued tasks
     for its role that are ready to take; claims of its own about to lapse.
   - **With nothing new, it prints nothing and never writes,** so most
     turns cost one read.
   - **With something to show:**
     - it prints first, at most 600 bytes. A line from the brain quotes
       what an agent wrote (a subject, a sender's name), so it comes after
       `guide::FRAMING` and through `guide::escape_brain`, as in the brief.
       For example: "mnemo: 2 unread, 1 task ready. From this project's
       brain: …: planner-a: v0.17.0 step 1 dispatched. `inbox` to read."
     - **then it takes one write window** (shared mode, 2 s wait) to mark
       those items `seen`, so the next turn doesn't show them again;
     - **a failure to mark** (a busy brain, a timeout) never stops the
       print, and never fails the hook. The items are shown again next
       turn, which is the safe direction.
   - **Cost:**
     - a read-only open per turn, with no embedding. A rough figure on this
       Mac: a read-only `mnemo info` of this brain took 0.06 s warm (0.14 s
       cold), and the session-start hook, which embeds, took 0.40 s. Both ran
       at a load average near 5, so they aren't benchmark numbers. Step 5
       measures the hook itself at a load under 2;
     - **context:** nothing on most turns, and at most 600 bytes when work
       arrives, against the brief's 8,000 bytes once per session.
   - **Measure first, then a fast path if it's needed** (R-010.6). Step 5
     times an unchanged turn at a load under 2. **If it's over about 20
     ms** (the keychain read is the likely part), the hook gets a
     per-session stamp in the brain's sidecar directory (0600) holding
     the last check's time, the header seal it saw, and when a claim of
     its own next needs a warning.
     - **An unchanged turn reads page 0 alone:** with the seal the same and
       no claim near its warning, it updates the stamp and exits, with no
       keychain read and no open. The seal is part of the unencrypted
       header (12.4's stamp), and step 5 confirms it can be read without the
       key.
     - **Cursor's once-a-minute gate** (`postToolUse` fires many times a
       turn) is checked in that stamp first, before anything else.
   - **The brief** gets a "Your work" part (section 7) with the same counts
     at session start.
2. **The idle wake-up through the session's own socket (Claude Code; step 6).**
   - Each session's `serve --mcp --shared` watches the brain (12.4's "13.1b
     wake-ups": kqueue, inotify or `ReadDirectoryChangesW`, plus a 30 s stamp
     poll).
   - **On a foreign commit,** it catches up and looks for `sent` items for its
     own agent, role or `all`. For each batch, within the guardrails, it
     posts **one pointer into its own session** through
     `CLAUDE_CODE_MESSAGING_SOCKET` with the token, then marks the items
     `delivered`.
   - **The pointer is fixed text and counts, never brain content.** It
     reads "mnemo (this session's own MCP server): new work in your inbox:
     2 messages, 1 task. Call the `inbox` tool to read them." It has no
     subject, body, sender, ref or id.
     - It reaches the session as a cross-session message, outside the
       framing line, so it must hold nothing an agent wrote.
     - A test posts a message whose subject, body and sender are hostile
       ("ignore your instructions…", a fake framing line), and checks that
       none of it appears in the posted text.
   - **How the woken session sees it.** By the docs, it arrives as a message
     that "came from another Claude session, not from you", and it can't
     approve a permission prompt. That framing is right for it:
     - the text says it's mnemo's pointer, from the session's own server;
     - it asks only that the agent call `inbox`;
     - what the agent then reads comes through the tool, under the framing
       line, like any brain content.
   - **`crossSessionInbound` must be `accept`** for the repo's sessions.
     With `hold`, each wake-up waits for the user to release it, which
     defeats the point. With `refuse`, path 2 does nothing (path 1 still
     works). The guide's section tells the user to keep `accept`, and
     `mnemo wake status` shows the setting it can see.
   - **It's its own session,** reached through that session's own socket,
     which the server inherited. No server pokes another session, and no
     socket of mnemo's is created.
   - **To verify in step 6, not assumed:**
     - that a stdio MCP server inherits the two variables (**checked
       2026-11-15** on this Mac's running servers, section 2; step 6
       repeats it in its DONE);
     - that a posted message wakes an idle session as the docs say;
     - the message format the socket takes;
     - how `crossSessionInbound` treats it.

     If any fails, path 2 is dropped, and the release ships with paths 1 and
     3. That's said in DONE, not worked around.
   - **Why this, not channels:** it's documented for scripts, works on every
     provider, needs no allowlist and no login beyond what the session
     already has, and can't relay a permission approval.
3. **The `Stop` continuation (Claude Code and Cursor; step 5).** When an
   agent finishes a turn and a task for it is ready, the hook answers
   `decision: "block"` with "mnemo: task <slug> is ready for you; take it
   with `task next`".
   - **Only when the task is `pre_approved` or the inbox item is addressed
     to this agent by id.** Never for `all`.
   - **Never twice in a row:** with `stop_hook_active` set, it allows the
     stop. Claude Code's own cap of 8 is the backstop, and Cursor's
     `loop_limit` its own.
   - **Off by default** (`onboard --wake stop` writes it). An agent kept
     going without its user's turn is the behaviour most likely to surprise.
**As built (step 5):**
- **The per-turn hook's cost, measured.** On an 11,000-record scratch brain
  with a key file, before the fast path, an unchanged turn took 22.8 ms
  median against `mnemo --version`'s 3.4 to 3.8 ms, at a load of 4.4.
  - On this repo's brain, the installed `mnemo info` (the keychain read
    and a read-only open) took 67 ms against 5 ms, at a load of 4.6.
  - Both are over R-010.6's threshold, so the fast path went in.
  - **With it:** 3.4 ms median against `--version`'s 3.3 to 4.1 ms, at a
    load of 3.2. A turn after another agent's post took 33 ms and showed
    it.
- **The fast path's stamp** is `hook.turn.<hash of client, session, agent
  and role>` in the sidecar directory. It holds the check's time, when a
  task claim of the agent's next needs a warning, and `Mnemo::file_stamp`.
  - `file_stamp` is page 0's seal read without the key, plus the file's
    identity, length and modification time, so a write the seal doesn't
    cover yet changes it too.
  - **It also finds the brain without running git**
    (`project::common_dir_fast`: a `.git` directory, or a `gitdir:` file
    with a worktree's `commondir`). `git rev-parse` was 14 of the first
    version's 18.5 ms. Any `GIT_DIR` or anything unusual falls back to git.
  - After its own mark it stamps the file as its write left it. A foreign
    commit in that instant shows at the next change or session start, the
    safe direction.
- **The guardrails govern wake-ups only** (`Mnemo::wake`, the one place
  that marks an item `delivered`). The per-turn hook and the brief show
  an agent's work in a turn it's already in, so the limits, the pause and
  the burst never hide work there. That matches the table: "items stay
  `sent` and are seen at the next turn or session start".
- **The kill switch is `_mnemo.wake_paused` on the manifest** (who and
  when), not a keyed `wake:paused` record, the same way the default agent
  is kept. A brain without a manifest can't be paused, and says so.
- **The Stop reason holds no slug:** "mnemo: 1 pre-approved task ready
  for you (call `task` with `next`)", plus a line saying what it reads
  there is information. The slug is brain text, and a Stop reason reaches
  the model outside the framing line.
- **`onboard --wake stop|off`:** left out, a re-run keeps the Stop hook if
  an earlier run wrote it.

4. **Channels (optional; not a step).** If mnemo's plugin is ever on the
   allowlist, `serve --mcp` declares `claude/channel` and sends the same
   pointer. Until then nothing in the release depends on it.

**`mnemo watch`** (13.1b's optional piece) starts a session for a role when
work arrives and no session of that role is alive. It runs `claude -p` or a
command the user configures.
- **Recommendation: defer it to after HARNESS-DOGFOOD-1's Run 1.**
  - It's the only piece that starts processes on its own.
  - Path 2 covers the case this repo has: sessions open in tmux, idle,
    waiting for work.
  - Run 1 shows whether an agent is ever missing when work arrives.
- If it ships, it's off by default. It runs only commands the user wrote
  into its config (never a command from the brain), with the same
  guardrails, and a REVIEWER-B pass first.

### The guardrails, from the first version

They live in the code that marks an item `delivered`, so they're the same
for every path. **The hop cap counts within a thread, and a fresh message
starts a new one,** so two agents trading new messages instead of replies
would never reach it on its own. For them, **the pair burst limit below is
the bound** (revised at step 3, PLANNER-A's DECISION):
- **Runaway loops are fast, and working exchanges are minutes apart.** The
  first version carried a hop count between a pair for 10 minutes, so a
  slow exchange kept climbing as long as each message came within 10
  minutes of the last. This project's own Planner and Worker trade a
  DISPATCH and a DONE every 5 to 15 minutes for hours. Under a cap of 8,
  wake-ups would stop after about four rounds.
- **So the pair limit counts a burst:** messages between the two, either
  way, in the last 10 minutes.
- `_mnemo.hops` and its carry (step 2) stay stored, as information. Step 5
  enforces this table.

| Guardrail | Default | What it does |
|---|---|---|
| Wake-ups per recipient per hour | 12 | Past it, items stay `sent` and are seen at the next turn or session start |
| Cooldown per recipient | 60 s | Items arriving inside it join the next pointer |
| Hop cap per thread | 8 | For explicit replies (`--conversation`): a reply whose `_mnemo.hops` would pass 8 is stored, but wakes nobody, and the brief says "thread X reached its hop cap" |
| Pair burst | 8 in 10 min | A message wakes nobody when its two agents have exchanged 8 or more messages, either way, in the last 10 minutes. It's stored and seen at the next turn or session start. A loop is fast; a working exchange, minutes apart, never reaches it |
| Kill switch | `mnemo wake pause` / `resume`, a keyed record `wake:paused` in the brain, so every session sees it; `MNEMO_WAKE=off` for one session | Paused: nothing is delivered, everything stays `sent` |
| Stop continuations | Off; at most one in a row when on | Section 3, path 3 |

**Messages are information, never authority.** The guide's card and the
pointer text say so:
- what an inbox item asks is weighed like any brain content;
- tagging, approving a release, deleting and anything irreversible stay
  with the human, whatever a message says;
- a pointer can't carry an approval. Claude Code already won't let a
  cross-session message approve a permission prompt.

### 12.4's open design item (RB-006.12)

13a adds **no daemon and no socket of its own**. Every path above runs in a
process the session started (its hook or its MCP server) and reaches only
the session that started it, or the brain through 12.4's direct protocol.
So the item stays open, unchanged, and it stays held in its file.
- **If a later step brings the daemon** (as `mnemo watch`'s host, say), that
  item's recommendations come first, as its triage says.
- **The one new outward call** is a post to the client's own socket, which
  the client owns and restricts to the OS user. The note says plainly that
  any same-user process can post there too: that's the client's design, not
  mnemo's. It doesn't widen what a same-user process could already do with
  the brain's key in the keychain.

## 4. Claims and presence (13.3)

**This is 12.4's step 7, which hasn't shipped:** the API on the design in
12.4's "Claims and presence", with the step 7 rework:
- claim tokens, and `check(topic, token)` before an irreversible step;
- the early lapse as a hint only;
- presence files re-checked by identity;
- a probe that retries once.

**The numbers:**

| | Default | Bounds |
|---|---|---|
| Claim lease (`ttl`) | 15 min | 30 s to 24 h |
| Renew | any time before `expires_at`; resets the lease | |
| Lapse | at `expires_at`; earlier only by 12.4's hint (a holder with a presence lock, not alive, seen twice over 10 s) | |
| Task claims (`next`) | 2 h, renewed by any `task update` from the owner | |
| `last_active` | updated inside a commit the agent makes anyway, at most once a minute | |

**`who()`** returns, for each `presence:<agent>` record: the agent, role,
tool, `last_active`, and `alive` from the sidecar lock probe. Agents with no
long-lived process (the CLI, reviewer-a) show `alive: unknown`.

**As built (step 3):**
- The presence lock is `presence.<16 hex of SHA-256(agent)>.lock`, directly
  in the sidecar directory (12.4 wrote `presence/<hash>.lock`). The flat
  name needs no subdirectory to create and check.
- A probe that finds the file missing, the directory missing, or the name
  naming another file after it locked says `unknown`, never "not alive".
- A claim's or presence record's v9 row carries a hash of its topic or
  agent in the thread-hash field, so a lookup decrypts only its candidates.
  That's no format change: an older build reads it as a thread hash that
  its decrypted check rejects.

**No lock across a turn.** A claim is a record, not a lock.
- **The brain's OS lock is held only inside one call's window,** as 12.4
  requires: `claim`, `renew` and `release` are each one exclusive window.
- **The only lock held between calls** is the presence lock on the agent's
  own sidecar file, which 12.4 already allows. It never touches the brain's
  lock, and only a long-lived `serve --mcp` takes it, never a hook or the
  CLI.

**Claims are advisory.** They're for cooperating agents, and nothing in
the engine refuses a write because of one. `task next` uses them so two
workers can't take one task.

## 5. Visibility (13.5)

`_mnemo.visibility = public | team | private` on any record, default `team`.
**A filter, not a security boundary,** until 13b: everyone holding the key
reads every record, and the engine says so.

| | public | team (default) | private |
|---|---|---|---|
| `recall`, `about`, the brief, `inbox` | Yes | Yes | Only when asked (`--visibility private`, or `"visibility": ["private"]`) |
| `export` (Markdown, JSONL) | Yes | Yes | Only with `--visibility all`; a stderr line says private records were included |
| `export --git` | Yes | Yes | **Never**, whatever the flag |
| `inspect` | Yes | Yes | Yes, with a "private" badge (it's the user's own, local and read-only) |

**`SECURITY.local.md` moving into `private` records before 13b: no.** I
agree with your view, for four reasons:
1. Every session holds the brain's key, so every session can read a
   private record.
2. A record is one forgotten flag away from an export.
3. The brain is copied, as the dogfood switch's backup showed, and a copy
   keeps everything.
4. R-006.3's triage already chose a **separate, never-committed private
   brain** for these, after 13b gives it its own key slots.

Until then the gitignored file stays the place.

## 6. Freshness (13.6)

- **`_mnemo.as_of`** (unix seconds): when the content was true. It
  defaults to the record's `created_at`, or `_mnemo.updated_at` after an
  edit.
- **`_mnemo.review_by`** (unix seconds, optional): when someone should look
  again.
- **`stale(now)`** lists records past `review_by`, or older than their
  kind's default:

| Kind | Stale after (no `review_by`) |
|---|---|
| essence `state` | 14 days |
| essence `next` | 14 days |
| essence `open` | 30 days |
| task `active` | 3 days with no update |
| task `blocked` | 7 days |
| message `unread` | 3 days |
| a fact (no kind) | never, unless `review_by` is set |
| claim, presence | their own TTLs, not freshness |

- **Where it shows:**
  - `about` and the essence render mark a stale item `[stale since <date>]`;
  - the brief's "Your work" part counts stale items that are this agent's
    (its tasks, its role's essence items);
  - `mnemo stale` and the MCP `about` with `{"stale": true}` list them.
- **Setting it:** `remember --review-by 30d` and `--as-of <date>`, and the
  same on `update`, the MCP tools and Python.

## 7. The surfaces

**MCP: three new tools, each with an `op`,** to keep the tool list short.
Their descriptions, the guide and the site present **tasks and the inbox
before claims** (R-010.3): agents think in "what's next" and "what's in my
inbox", and a claim is mostly what `task next` uses underneath.

| Tool | Ops |
|---|---|
| `inbox` | `list` (default: unread for me), `read <id>`, `post {to, subject, body, refs?, thread?}`, `ack {id, note?}` |
| `task` | `add {title, body?, role, after?, pre_approved?, slug?}`, `next {role?}`, `update {slug, status?, note?}`, `list {status?, role?}`, `get {slug}` |
| `claim` | `take {topic, ttl?}`, `renew {topic, token}`, `release {topic, token}`, `check {topic, token}`, `who` |

- **`about`** gets `stale`, and every write tool gets `visibility`,
  `review_by` and `as_of`.
- The identity is the server's (`MNEMO_AGENT`); the tools take no `from`.

**CLI:**
- `mnemo inbox [--all]`, `mnemo post <to> <subject> [--body …] [--refs …]`, `mnemo ack <id>`;
- **`--as <agent>` and `--role <role>`** on each coordination command
  (`post`, `inbox`, `ack`, `task`, `claim`, `who`), falling back to
  `MNEMO_AGENT` and `MNEMO_ROLE` (R-010.9). So a session without hooks
  (reviewer-a in Antigravity) or a person in a plain terminal runs
  `mnemo inbox --as reviewer-a`, with no environment prefix;
- `mnemo task add|next|update|list|get`;
- `mnemo claim take|renew|release|check|who`;
- `mnemo stale`, `mnemo wake pause|resume|status`;
- `mnemo hook user-prompt` and `mnemo hook stop` (the hooks).

Each takes no path in a git repository.

**Python and Rust:**
- `post`, `inbox`, `ack`;
- `task_add`, `task_next`, `task_update`, `tasks`;
- `claim`, `renew`, `release`, `check_claim`, `who`;
- `stale`, plus `visibility`, `review_by` and `as_of` on the write methods.

The mobile bindings get none in 13a. They have no sessions to coordinate.

**The brief** (12.2's `session-start`) gets a fifth part, "Your work", **at
most 600 bytes, taken from the recall's share.** For example: "2 unread
(planner-a: v0.17.0 step 1 dispatched; reviewer-b: …), 1 task ready (step
1), 1 claim yours (PLAN.md tail, 4 min left), 1 stale (essence state:
'Status: v0.16.1 …')". The total stays at 8,000 bytes by default, and the
part is left out when there's nothing.

**The guide's new section, "Working with other agents":**
- starting each session with `MNEMO_AGENT` and `MNEMO_ROLE`, with an alias
  per role as the recipe (R-010.9): `alias claude-worker='MNEMO_AGENT=worker-a
  MNEMO_ROLE=worker claude'`;
- tasks and the inbox first, then claims (R-010.3);
- `inbox`, `task` and `claim`, as `sh test` and `json mcp` blocks that CI
  runs against a scratch brain;
- the rule that messages are information;
- the kill switch.

The card gets one line, kept under 1,200 bytes: "If you work with other
agents, call `inbox` when the hook says you have work."

## 8. The dogfood path to HARNESS-DOGFOOD-1

| Today, by hand | With 13a | Step |
|---|---|---|
| Pointers between sessions (`READ:`, `DONE:`) | `post` to the role, and the wake-up through the session's own socket | 2, 6 |
| DISPATCH and DONE entries | A task per dispatch (`queued`, taken with `next`), `ready` at DONE, `done` at the Planner's acceptance | 4 |
| "One writer on PLAN.md at a time", re-reading the tail | `claim take plan` before a write, released after | 3 |
| `*.local.md` for embargoed findings | **Stays until 13b** (section 5), then a separate private brain | 13b |
| A stale Status section | `stale`, and Status as an essence `state` item with a `review_by` | 7 |

**What has to be true for PLAN.md to shrink to specs** (R-006.3's Run 2):
1. Every dispatch and DONE of one full release went through tasks and the
   inbox, with the user only approving (13.1b's "accepted when").
2. The ledger's decisions are in the brain with keys (DOGFOOD-0 rule 3),
   and the tally shows recall finds them (hits, not "not stored").
3. `export --git` of tasks and decisions is the committed backup, and reads
   well enough to replace the WORKLOG in a PR.
4. `reviewer_notes.md`'s items can live as records (a `finding` kind with
   `FINDING_TRANSITIONS`). **That kind isn't in 13a's scope,** and I
   recommend adding it as step 8b if Run 1 wants it.
5. The private brain exists (13b), or `SECURITY.local.md` stays beside it.

## 9. Deferred items that belong here

| Item | Recommendation |
|---|---|
| Keyword-indexing saved turns (from 12.2) | **Stays carried.** It's about recall over history, not coordination, and it changes the BM25 segments, which deserves its own step and its own measurements |
| CLI-PATHLESS-2 (`update`, `thread start`, `essence update\|resolve\|remove` without a path) | **Into step 1:** the same `[PATH] <ID>` shift as `get`. The new commands are pathless from the start, so the old ones should match |
| ABOUT-COUNT (`about`'s header counts turns as memories) | **Into step 7:** the header shows facts, turns and coordination records apart, since 13a adds more kinds to count |

## 10. The steps for v0.17.0

Each step is one PR into `release-0.17.0`, with its tests.

| Step | What | REVIEWER-B? |
|---|---|---|
| 0 | This note | |
| 1 | **The plumbing:** the four kinds left out of recall, BM25, ANN and `about`; the sentinel vector; `_mnemo.author`, `visibility`, `as_of` and `review_by` accepted on writes; AGENT-DEFAULT (`_mnemo.default_agent` on the manifest, `mnemo default-agent`, set once on this brain, and in `seed.json`); `MNEMO_AGENT`/`MNEMO_ROLE` read by the CLI, the server and the hooks (CI tests that; **whether the client passes them to hooks is a by-hand check** in its DONE); CLI-PATHLESS-2 | |
| 2 | **Messages:** `post`, `inbox`, `ack`, the delivery and status fields in place, the hop count and its pair carry; the MCP `inbox` tool, the CLI and Python; the brief's "Your work" part | |
| 3 | **Claims and presence** (12.4 step 7): the claim window, tokens, `check`, the presence sidecar lock, `who()`; the 1,000-race test and FLAKE-1's pattern | **Yes:** the file's concurrency |
| 4 | **Tasks:** `add`, `next` (which claims), `update`, `list`, `get`, dependencies, `pre_approved`; the MCP `task` tool | |
| 5 | **The per-turn hook and the Stop continuation:** `mnemo hook user-prompt` (a read on most turns, one write window only when it shows something; its line under the framing line) and `hook stop`; `onboard` writes the first, and `--wake stop` the second; the guardrails and the kill switch; the hook's cost measured at a load under 2 | |
| 6 | **The idle wake-up:** the file watcher in `serve --mcp --shared`; the post to the session's own socket, fixed text and counts only (with the hostile-subject test); the verifications in section 3. Dropped if they fail | **Yes:** a long-lived watcher, and a process that posts into another program's socket |
| 7 | **Visibility and freshness:** the filters, the `export` rules, `stale`, the `about` and brief marks, ABOUT-COUNT | |
| 8 | **The guide's section** and the card line, `llms.txt` regenerated; the README rows | |
| 9 | **READY:** the bump to 0.17.0, the CHANGELOG, the dry run | |

**The scoped REVIEWER-B pass: after step 6,** over steps 3 to 6:
- the claim window and presence locks;
- the watcher's descriptor and catch-up;
- the socket post;
- the Stop continuation's loop bounds.

Visibility (step 7) is a filter and needs no pass of its own. If step 8b (a
`finding` kind) or `mnemo watch` is added, each needs its own look.

**What the user does:**
- starts each session with `MNEMO_AGENT` and `MNEMO_ROLE` (one line per
  tmux pane), once step 1 ships;
- re-runs `mnemo onboard` after step 5, to add the per-turn hook, and
  decides whether to turn on `--wake stop`;
- keeps `crossSessionInbound` at `accept` for the repo's sessions: with
  `hold` each wake-up waits for them, and with `refuse` only the per-turn
  check works;
- needs no claude.ai login and no channel flags for any of this. Channels
  stay optional (section 3, path 4);
- approves the release.

## For sign-off

1. The four kinds as records with the sentinel vector, never recall hits.
2. `MNEMO_AGENT` and `MNEMO_ROLE` from the environment (checked for
   Claude Code's server; hooks by hand in step 1), and AGENT-DEFAULT by a
   reserved manifest key, `_mnemo.default_agent`.
3. Delivery: the per-turn hook first, the idle wake-up through the
   session's own socket (verified in step 6, or dropped), Stop
   continuations off by default, channels optional, `mnemo watch`
   deferred.
4. The guardrail defaults: 12 an hour, 60 s, 8 hops with the 10-minute pair
   carry, and the kill switch.
5. Claims as 12.4 step 7, with the TTLs above.
6. Visibility as a filter, and `SECURITY.local.md` staying.
7. Freshness's per-kind defaults.
8. Three MCP tools, the brief's 600-byte part, and the per-turn hook's one
   write when it shows something.
9. The step list, and REVIEWER-B after step 6.

## Sources (read 2026-11-15)

- Claude Code hooks reference: https://code.claude.com/docs/en/hooks.md;
  guide: https://code.claude.com/docs/en/hooks-guide.md
- Claude Code channels: https://code.claude.com/docs/en/channels.md
- Claude Code MCP (channels capability, `list_changed`):
  https://code.claude.com/docs/en/mcp.md
- Claude Code cross-session messaging:
  https://code.claude.com/docs/en/cross-session-messaging.md
- Claude Code agent teams: https://code.claude.com/docs/en/agent-teams.md
- Cursor hooks: https://cursor.com/docs/hooks
- Cursor cloud (background) agents: https://cursor.com/docs/background-agent

The hooks reference's event table was read truncated, so the note relies
only on the events it names. The channels page gives no version.
