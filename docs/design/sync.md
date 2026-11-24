# Encrypted sync: the design note

Status: **draft by PLANNER-A, 2026-11-18.** The user decided the same day
to build the free steps (1 and 2 in section 12) after v0.20.0, and to hold
the paid ones: the hosted relay, Pro, 13.2, 13.4 and Teams
(`decision.roadmap-order`). The user decides the pricing. This note is what
Phase 14.1 asked for before any server work: the change-set format, sync
cursors, replay safety, authenticating writes to the relay, compacting the
mailbox, and the metadata the relay can see.

## 1. What it's for

One person's brain on several machines: a laptop, a desktop, a cloud
worker, a phone. Each copy keeps working offline and catches up when it
can. **No server can read it.** Later, a team's brain shared across people,
which needs per-member keys first (section 9).

Under the monetization rule ("if it runs on one machine, it's free"), the
parts split like this:

| Part | Runs on | Tier |
|---|---|---|
| Merging two copies of a file (`mnemo merge`, 13.7) | one machine | free |
| The sync client in the engine and the CLI | the user's machines | free code, Apache-2.0, so anyone can audit that the relay sees only ciphertext |
| The hosted relay, accounts and billing | our server | **Pro** (one person's devices), then **Teams** |
| A self-hosted relay | the customer's server | paid (enterprise), private repo; FSL-1.1-ALv2 only if it's ever published |

## 2. Prior art, and what we take from it

- **Obsidian Sync:** end-to-end encrypted sync of a folder of files. The
  server stores encrypted files and version history; conflicts are
  resolved per file. We take the promise (the server holds only
  ciphertext) and the honest framing of what the server still sees.
  Per-file merging is too coarse for a brain: two sessions on two machines
  write different facts into the same file all the time.
- **Turso / libSQL embedded replicas:** sync for an embedded database by
  shipping its write-ahead log through a server that can read it. That's
  the opposite trade: easy merging because the server sees plaintext.
  Shipping pages also can't work for us, since each copy seals its pages
  under its own per-session nonces and puts records at different page
  numbers.
- **CRDTs and local-first software** (Automerge, Yjs, Ink & Switch's
  local-first essay): every replica converges without a coordinator. We
  don't need a general CRDT, because the engine is already close to one.
  Rule 2 of the engineering rules ("supersede, don't erase") makes the
  memories a grow-only set of records, plus a "newest wins" rule per key.
  Both merge cleanly.

**The design in one line:** replicas exchange encrypted, padded batches of
whole records through a relay that is a dumb, append-only mailbox, and
each replica merges them locally with 13.7's rules.

## 3. What syncs

| Kind | Syncs? | Why |
|---|---|---|
| Facts, keyed and unkeyed, with their supersession and history | yes | the point of it |
| Thread turns and thread essence | yes | a session started on another machine carries on |
| Project essence items | yes | |
| Tasks and findings | yes | the coordination ledger, so a worker on another machine sees the queue |
| Snapshots | no | each replica keeps its own restore points |
| Messages (the inbox) | no, in the first version | they're addressed to sessions, which are machine-local; tasks carry the work across machines |
| Claims and presence | no | they're locks with lease times on one machine's clock |
| Access statistics, the ANN index, the vector column, the result cache | no | derived or local; each replica rebuilds them |
| Private records | yes for Pro (one person); for Teams only to the members a 13.2 grant allows | |

## 4. The change set

A **replica** is one copy of the file on one machine. It's identified by a
random 128-bit replica id, kept in the file as a `_mnemo.sync` record (no
header change, so no format bump).

A **sync group** is the set of replicas of one brain. Its id is a random
128-bit value made when sync is first turned on, stored in the same
record, and carried to a new device with the file or the pairing step.

A **change set** is one replica's new work since its last push:
- every record written since then, whole: content, metadata, the vector
  (f16, so the receiver doesn't re-embed), and the `_mnemo.*` keys;
- deletions and supersessions as their own entries (the id and the time);
- essence and task changes as the records they already are.

The **envelope** that carries it:

```
magic "MNSYNC1" | version u8 | group id 16B | key epoch u32
| replica id 16B | seq u64 | nonce 12B
| AES-256-GCM( zstd(entries) || padding ) | tag 16B
```

- The header fields are the AEAD's associated data, so the relay can
  route on them but not change them.
- **Padding:** the sealed size is rounded up to a power of two, with a
  floor of 4 KiB. One visibility scope per envelope, so compression
  can't leak across scopes (R-004.5).
- **The embedder must match.** A change set carries the sender's embedder
  identity (model and revision hashes). A replica with a different one
  refuses to apply it and says why, rather than mixing vector spaces.

## 5. Keys and the relay's write token

Personal sync needs no new key material. Every device of one person
already has the data key: it's in the keychain, and the recovery code
carries it to a new machine (12.3). From it, each replica derives with
HKDF-SHA256 (salt = group id):
- the **sync key**, which seals envelopes (never the page key itself);
- the **mailbox id**, which names the group on the relay;
- the **write token**, sent with each push. The relay stores its hash when
  the mailbox is created and refuses pushes without it, so a stranger
  can't fill the mailbox;
- the **read token**, the same for pulls.

The hosted relay also has an account and billing login. That's separate
from all of this, and it never touches a key.

**Rekey and revocation:** `mnemo rekey` changes the data key, so the
derived keys move to a new epoch. Other replicas need the new key, through
the recovery code or a pairing step. For one person that's rare, and the
note says so plainly. Teams handle it with key slots (section 9).

## 6. Cursors, replay and order

- Each replica numbers its pushes 1, 2, 3, and so on. The relay keeps an
  append-only log per mailbox of `(replica id, seq) → envelope`.
- Each replica keeps a **version vector**: for every replica, the highest
  seq it has applied. A pull asks for everything past it.
- **Applying is idempotent:** a record is identified by its ULID and a hash
  of its content. Seeing it twice changes nothing; the same ULID with
  different content is kept and flagged (13.7).
- **Clocks:** ULIDs and `created_at` come from each machine's wall clock,
  and "newest wins" is only as good as those clocks. Synced writes also
  carry a hybrid logical clock (`_mnemo.hlc`): wall time, bumped past
  anything the replica has seen. Newest-wins compares that, then the ULID.
  It's metadata, so there's no format change, and older builds carry it
  through untouched.

## 7. Merging

The rules `mnemo merge` shipped with in v0.21.0 (spec.v0210 and the
decisions it names), applied one change set at a time:
- **Records:** union by ULID.
- **A key written on two replicas since they last synced:** the key's
  versions form one chain, ordered by which version was written over which,
  and by the first-written stamp only between versions with no such link.
  The last is live; the other is superseded and stamped
  `_mnemo.merge_conflict`, and both stay in the key's history.
  `mnemo inspect` and `about` show the conflicts (decision.v0210-chain-rule).
- **A delete against an edit:** a delete removes exactly the record it
  named, with no clock. A newer version of the same key that the deleting
  replica never saw is kept, and the report names the key
  (decision.v0210-merge-rules, which replaced "the newest action wins").
- **Thread turns:** union by `turn_id`.
- **Essence items:** per item, the newest change wins. "Resolved" beats a
  concurrent "open", unless the "open" came after it.
- **Tasks:** the status comes from the last history entry that changed it;
  a note never sets it. When both replicas moved it to different statuses,
  the later move wins and the task is flagged
  (decision.v0210-task-status). Claims don't sync, so a claim never
  crosses machines.
- **Settling a conflict:** a user's write to a flagged record settles it,
  and the flag goes at once (REV-019.1). This matters more under
  continuous sync than for an occasional `mnemo merge`, since flags would
  otherwise pile up.
- **For change sets (step 3):** consider carrying explicit operations
  (put, supersede, update, delete, a task's move) with (replica, sequence,
  stamp), replayed in place so local snapshots survive, and keeping
  `mnemo merge` as the offline whole-file fallback (REV-017.3, REV-019.4).
- **Findings:** the same as tasks, with 13a's transitions checked again
  after the merge.
- **The index** takes the new facts through the normal insert path, or is
  rebuilt when a pull is large.

## 8. The mailbox over time

- The log grows with every push. Any replica may upload a **checkpoint**:
  one envelope that holds the whole brain as of a version vector V.
- The relay sees only `(replica, seq)` pairs, so it can drop every
  envelope that V covers once a checkpoint for V exists and each known
  replica has pulled past it. A replica silent for 90 days stops holding
  the log back; when it returns, it starts again from the checkpoint.
- **A new device** downloads the newest checkpoint, then the tail.

## 9. Teams, later

A team's brain needs what one person's doesn't:
- **13.2 key slots:** each member and device has its own slot, and
  revoking one rotates the data key;
- **13.4 signatures:** with a shared data key, anyone can write anything,
  so signed authorship is what makes a forged write detectable;
- **visibility per member:** private records go only to the slots allowed
  to read them.

So the order is: 13.7, then personal sync (Pro), then 13.2 and 13.4, then
team sync (Teams). That's the order `PLAN.md` set on 2026-11-15, still
right.

## 10. What the relay can see

It's honest to say this plainly:

| It sees | It doesn't see |
|---|---|
| the mailbox id (a random-looking value) | any content, metadata, key or vector |
| how many replicas a group has, and when each one pushes or pulls | which records changed |
| each envelope's size, to the nearest power of two (4 KiB at least) | exact sizes |
| the account that pays, and IP addresses | which brain is whose, beyond the account |

What that can still reveal: how active a brain is and when, and roughly
how big a burst of work was. The padding limits that to a factor of two.
Hiding the timing would take cover traffic, which costs bandwidth for
little gain here. This note doesn't propose it.

## 11. Commands

- `mnemo merge a.mnemo b.mnemo -o merged.mnemo` (13.7: free, local, and
  useful on its own);
- `mnemo sync` (push, then pull), `mnemo sync --status` (the version
  vector, the conflicts, the last sync), `mnemo sync pair` (bring a new
  device in with the recovery code);
- later, background sync from the MCP server's long-lived handle, after
  each commit and on a timer;
- `mnemo relay` (the self-hosted relay; private repo): blob storage with
  the log, the tokens and the checkpoint rule, on disk or on S3-compatible
  storage.

## 12. Build order

1. **13.7, `mnemo merge`** (free). The merge rules, the HLC and
   `_mnemo.merge_conflict`, tested with diverged copies, proptests over
   random interleavings, and replay of the same change set twice.
2. **Change sets on disk** (free): `mnemo sync export --since <vector>`
   and `mnemo sync import`. This tests the envelope, padding and
   idempotence without a server, and works through any shared folder by
   hand.
3. **The relay protocol,** with a test relay in the repo's tests only.
4. **The hosted relay (Pro)**: a private repo, accounts, billing. This is
   where the business prerequisites come in (all the user's): contributor
   terms, a trademark, an entity, counsel, and a Merchant of Record.
5. **Teams**, after 13.2 and 13.4.

**On hold (user decision, 2026-11-18):** steps 3 to 5. Steps 1 and 2 are
on the roadmap after v0.20.0.

Steps 1 and 2 are useful on their own: two copies of a brain diverge
today whenever someone copies the file. Each step is its own release, and
REVIEWER-B reviews each one, since they touch the encryption.

## 13. Open questions for the user

1. **Pricing** for Pro and Teams (yours alone), before step 4 ships.
2. **Hosting:** where the relay runs, and in which regions, before step 4.
3. **The business prerequisites** in step 4, before the first paid
   sign-up.

Decided by the Planner, to revisit if it's wrong: **messages don't sync**
in the first version. They're addressed to sessions on one machine, and
the tasks, which do sync, carry the work between machines.
