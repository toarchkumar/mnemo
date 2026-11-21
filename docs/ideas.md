# Ideas we've explored

Ideas that were looked at and aren't on the roadmap, with what we found
and what would bring each one back. The roadmap itself is in the project
brain (`decision.roadmap-order`).

## One brain for many projects, and namespaces (2026-11-21)

**The idea:** one `.mnemo` file holding several projects, like one SQLite
file holding several apps.

**What the review found** (REVIEWER-A, `REV-016`):
- **One file for many projects: no.** Visibility is a filter, not
  encryption, so a file can't keep one project's memories from another's.
  Unrelated projects would also crowd each other's recall, wait on the
  same write lock, and lose the automatic `.git` discovery.
- **The rule (user, 2026-11-21):** one file is one trust boundary. Files
  separate trust, sharing, sync and lifecycle.
- **Reading across brains** is on the roadmap instead: **attached brains**
  (`--attach`, step 4b, after smarter memory). A session attaches other
  brains, read-only first, such as a personal brain or a playbook beside
  the project's, and one recall searches them all.
- **Namespaces** (`_mnemo.namespace`, `--namespace`) could organize goals
  or areas inside one owner's brain, for a monorepo or an app.

**Status:** namespaces aren't on the roadmap. They come back if one brain
holds several unrelated goals and recall or the essence gets crowded.

## Hosted brains for an agent harness (2026-11-19)

**The idea:** one encrypted `.mnemo` file per client in cloud storage,
with keys protected by a cloud key service, as the memory behind an agent
harness that gives outside clients standardized consultation: one
read-only playbook file for the method, plus one file per client.

**What the research found** (REVIEWER-A, `REV-011`):
- **MNemo selling hosted brains: no.** Turso, Cloudflare Durable Objects
  and turbopuffer already sell per-customer isolation cheaply, Turso with
  customer-held keys. Running a router, leases, 24/7 operations and a
  SOC 2 audit doesn't pay for a small team.
- **A consultancy built on MNemo: worth a test, not now.** Clients pay
  for the consulting; memory costs under 1% of that. A file per client
  helps in a client's security review: a hard wall between clients, a
  clean handover, provable deletion. Bulk documents belong in a separate
  search store.
- **The test, if it comes back:** one week, no engine changes, on v0.20.0.
  Keys wrapped by a cloud key service outside the engine
  (`open_with_key`), one host per client file, 5 to 10 likely buyers.
  It passes on 2 or 3 paid pilot commitments and a cold open under 0.6 s.
- **Rules for any hosted design:** upload change sets, never the whole
  file on each save; one cloud key wrapping every client's key, not one
  cloud key per client; compute in the same region as the storage.

**Status:** on hold (user, 2026-11-19). The change-set format (roadmap
step 2) is being designed so it would fit object storage.

## Encrypted sync: relay, Pro and Teams (2026-11-18)

**The idea:** a relay that only ever sees ciphertext, so one person's
brain syncs across machines (Pro) and later a team's (Teams).
`docs/design/sync.md` has the design.

**Status:** the free steps (`mnemo merge`, then change sets on disk) are on
the roadmap. The relay, Pro, key slots (13.2), signatures (13.4) and Teams
are on hold with all paid work. Pricing, hosting and the business setup
are the user's to decide first.

## Public benchmarks (2026-11-18)

**The idea:** repeatable benchmarks against CLAUDE.md, a notes folder and
other memory tools, published with their scripts.

**Status:** backlog.

## Demo and launch (2026-11-18)

- **Chosen:** a scripted terminal GIF, re-recorded at every release
  (`docs/demo/`).
- **Not chosen:**
  - a voiced 60 to 90 second video, which mostly matters for a launch;
  - a live in-browser demo, which would take weeks (backlog).
- **A launch day** (HN, Reddit, X): not for now (user).
