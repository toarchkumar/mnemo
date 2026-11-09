//! Phase 12.7a: the thread archive (ingest, export, forget).

use super::*;

/// One incoming turn after chunking or truncation, ready to embed and
/// write (Phase 12.7a, split out for 12.7d's batch embed).
pub(super) struct PendingTurn {
    pub(super) turn_id: String,
    pub(super) role: String,
    pub(super) created_at: i64,
    pub(super) parts: Vec<String>,
    /// Hex sha256 + original length, set only for a truncated
    /// tool-result turn. Applied to the single chunk the truncation
    /// yielded.
    pub(super) truncation: Option<(String, u64)>,
}

/// Output of `Mnemo::plan_ingest`: the turns to write, the first free
/// `_mnemo.seq`, and the counts gathered so far (skips, chunking,
/// truncation).
pub(super) struct IngestPlan {
    pub(super) pending: Vec<PendingTurn>,
    pub(super) next_seq: u64,
    pub(super) report: IngestReport,
}

impl IngestPlan {
    /// Every text to embed, in write order (turns, then parts).
    pub(super) fn texts(&self) -> Vec<&str> {
        self.pending
            .iter()
            .flat_map(|p| p.parts.iter().map(String::as_str))
            .collect()
    }

    pub(super) fn text_count(&self) -> usize {
        self.pending.iter().map(|p| p.parts.len()).sum()
    }
}

/// Hex-encode a byte slice. Low-dependency alternative to pulling in a
/// hex crate for the lone call site in `ingest_thread`'s SHA-256
/// recording.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Return the longest UTF-8 prefix of `s` whose byte length is `<=
/// max_bytes`. Always lands on a `char` boundary.
fn take_utf8_prefix(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    // Walk forward through char indices; the last index whose byte
    // offset fits inside `max_bytes` tells us where to cut.
    let mut cut = 0usize;
    for (idx, _) in s.char_indices() {
        if idx > max_bytes {
            break;
        }
        cut = idx;
    }
    s[..cut].to_string()
}

fn chunk_utf8(s: &str, max_bytes: usize) -> Vec<String> {
    if s.len() <= max_bytes {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = s;
    while rest.len() > max_bytes {
        let head = take_utf8_prefix(rest, max_bytes);
        let head_bytes = head.len();
        out.push(head);
        rest = &rest[head_bytes..];
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

impl Mnemo {
    /// Ingest a batch of turns into the archive layer of `thread_id`.
    ///
    /// Each [`ThreadTurn`] becomes one or more `Episodic` records
    /// tagged with the usual thread-archive reserved-metadata set
    /// (`_mnemo.kind = "turn"`, `_mnemo.thread`, `_mnemo.turn_id`,
    /// `_mnemo.seq`, `_mnemo.role`). The ingest path is
    /// **idempotent** per `turn_id`: a turn whose `turn_id` already
    /// appears on a record in this thread is skipped, so re-running
    /// `ingest_thread` with an extended transcript appends only
    /// what's new.
    ///
    /// **Chunking.** A non-tool turn whose UTF-8 content exceeds
    /// [`THREAD_TURN_CHUNK_BYTES`] (8 KB) is split into chunks at
    /// char boundaries, each stored as its own record with
    /// `_mnemo.part = 0, 1, …` under the same `turn_id`.
    ///
    /// **Truncation.** A `role = "tool"` turn whose content exceeds
    /// [`THREAD_TOOL_RESULT_TRUNCATE_BYTES`] (16 KB) is stored
    /// truncated — the first 16 KB of UTF-8 goes into the record,
    /// and `_mnemo.sha256` + `_mnemo.length` capture the original
    /// hash and byte length so the full content can be recovered
    /// (or verified) out-of-band.
    ///
    /// **Embedding.** An embedder must be registered
    /// ([`Mnemo::set_embedder`]); the content strings (one per
    /// resulting chunk / truncated record) are batched into a single
    /// `embed()` call.
    ///
    /// Call `self.flush()` after a successful ingest to persist.
    pub fn ingest_thread(&mut self, thread_id: &str, turns: &[ThreadTurn]) -> Result<IngestReport> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let plan = self.plan_ingest(thread_id, turns)?;
        if plan.pending.is_empty() {
            return Ok(plan.report);
        }
        // Batched embed: one round trip to the embedder for the whole
        // ingest, whatever the turn count.
        let vectors = {
            let embedder = self.embedder.as_ref().ok_or_else(|| {
                MnemoError::Invalid(
                    "ingest_thread: no embedder registered on this handle; call set_embedder first"
                        .into(),
                )
            })?;
            embedder.embed(&plan.texts())?
        };
        self.check_vectors("ingest_thread", &vectors, plan.text_count())?;
        self.apply_ingest(thread_id, plan, vectors, None)
    }

    /// Check a batch of embedder output before anything is written:
    /// one vector per input, the file's width, finite and non-zero.
    pub(super) fn check_vectors(
        &self,
        ctx: &str,
        vectors: &[Vec<f32>],
        expected: usize,
    ) -> Result<()> {
        if vectors.len() != expected {
            return Err(MnemoError::Invalid(format!(
                "{ctx}: embedder returned {} vectors for {expected} texts",
                vectors.len()
            )));
        }
        for v in vectors {
            self.check_vector(ctx, v)?;
        }
        Ok(())
    }

    /// The checks `put` applies to one vector, run up front so a batch
    /// can't fail half-way through its writes.
    pub(super) fn check_vector(&self, ctx: &str, v: &[f32]) -> Result<()> {
        if v.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: v.len(),
            });
        }
        if embed::is_zero_vector(v) {
            return Err(MnemoError::Invalid(format!(
                "{ctx}: all-zero vector refused"
            )));
        }
        if v.iter().any(|x| !x.is_finite()) {
            return Err(MnemoError::Invalid(format!(
                "{ctx}: vector contains a non-finite value"
            )));
        }
        Ok(())
    }

    /// Planning half of [`Mnemo::ingest_thread`]: validate the turns, skip
    /// the ones already archived, and chunk or truncate the rest. Reads
    /// only; nothing is written.
    pub(super) fn plan_ingest(
        &mut self,
        thread_id: &str,
        turns: &[ThreadTurn],
    ) -> Result<IngestPlan> {
        if thread_id.is_empty() {
            return Err(MnemoError::Invalid(
                "ingest_thread: 'thread_id' must not be empty".into(),
            ));
        }
        // FMT-9b.2: v9 chunks fit one packed page with their metadata.
        let chunk_bytes = if self.is_v9() {
            V9_TURN_CHUNK_BYTES
        } else {
            THREAD_TURN_CHUNK_BYTES
        };
        let mut report = IngestReport::default();
        if turns.is_empty() {
            return Ok(IngestPlan {
                pending: Vec::new(),
                next_seq: 0,
                report,
            });
        }

        // Build the set of turn_ids already in this thread so the
        // idempotence check is O(1) per incoming turn. First-pass
        // cost: one decrypt per existing thread record. A future v9
        // format bump can move `_mnemo.turn_id` into the catalog row
        // and drop this scan.
        // R-004.2: the turn ids already archived and the next `_mnemo.seq`
        // come from the handle caches: one lazy scan per handle, then O(1)
        // per ingest instead of two full decrypt scans each time.
        self.ensure_key_caches()?;
        self.warm_thread(thread_id)?;
        let cached = self.layers.as_ref().and_then(|l| l.threads.get(thread_id));
        let existing_turn_ids: std::collections::HashSet<&str> = cached
            .map(|tc| tc.turns.keys().map(String::as_str).collect())
            .unwrap_or_default();
        // A repeat ingest continues the sequence instead of restarting at 0.
        let next_seq = cached.and_then(|tc| tc.max_seq).map_or(0, |n| n + 1);

        // Determine each incoming turn's final text pieces (post-
        // chunking / truncation) before any writes, so the caller can
        // embed the whole batch in one `embed()` call.
        let mut pending: Vec<PendingTurn> = Vec::new();
        let now = memory::now_secs();
        for t in turns {
            if t.turn_id.is_empty() {
                return Err(MnemoError::Invalid(
                    "ingest_thread: turn_id must not be empty".into(),
                ));
            }
            if existing_turn_ids.contains(t.turn_id.as_str()) {
                report.skipped_existing_turns += 1;
                continue;
            }
            let content_bytes = t.content.len();
            let is_tool = t.role == "tool";
            let (parts, truncation) =
                if is_tool && content_bytes > THREAD_TOOL_RESULT_TRUNCATE_BYTES {
                    report.truncated_turns += 1;
                    let hash = {
                        use sha2::{Digest, Sha256};
                        let h = Sha256::digest(t.content.as_bytes());
                        hex_encode(&h)
                    };
                    let truncated = take_utf8_prefix(&t.content, THREAD_TOOL_RESULT_TRUNCATE_BYTES);
                    (vec![truncated], Some((hash, content_bytes as u64)))
                } else if content_bytes > chunk_bytes {
                    report.chunked_turns += 1;
                    (chunk_utf8(&t.content, chunk_bytes), None)
                } else {
                    (vec![t.content.clone()], None)
                };
            pending.push(PendingTurn {
                turn_id: t.turn_id.clone(),
                role: t.role.clone(),
                created_at: t.timestamp.unwrap_or(now),
                parts,
                truncation,
            });
        }
        Ok(IngestPlan {
            pending,
            next_seq,
            report,
        })
    }

    /// Write half of [`Mnemo::ingest_thread`]. `vectors` holds one vector
    /// per text of `plan.texts()`, already checked. `ttl` stamps every
    /// new record (the `commit_deltas` `ExpireAfter` policy).
    pub(super) fn apply_ingest(
        &mut self,
        thread_id: &str,
        plan: IngestPlan,
        vectors: Vec<Vec<f32>>,
        ttl: Option<i64>,
    ) -> Result<IngestReport> {
        let IngestPlan {
            pending,
            mut next_seq,
            mut report,
        } = plan;
        // Order: each pending turn's chunks in part order; each turn
        // gets a single `_mnemo.seq` slot, so `_mnemo.part`
        // disambiguates chunks of the same turn.
        let mut vectors = vectors.into_iter();
        for p in &pending {
            let this_seq = next_seq;
            next_seq += 1;
            for (part_idx, part_text) in p.parts.iter().enumerate() {
                let vec = vectors.next().ok_or_else(|| {
                    MnemoError::Invalid("ingest_thread: ran out of vectors".into())
                })?;
                let mut m =
                    Memory::new(part_text.clone(), MemoryType::Episodic, vec).with_importance(0.5);
                if let Some(t) = ttl {
                    m = m.with_ttl(t);
                }
                m.created_at = p.created_at;
                m.accessed_at = p.created_at;
                m.metadata.insert(
                    MNEMO_META_KIND.into(),
                    Value::String(MNEMO_META_KIND_TURN.into()),
                );
                m.metadata.insert(
                    MNEMO_META_THREAD.into(),
                    Value::String(thread_id.to_string()),
                );
                m.metadata
                    .insert(MNEMO_META_TURN_ID.into(), Value::String(p.turn_id.clone()));
                m.metadata
                    .insert(MNEMO_META_SEQ.into(), Value::Number(this_seq.into()));
                m.metadata
                    .insert(MNEMO_META_ROLE.into(), Value::String(p.role.clone()));
                if p.parts.len() > 1 {
                    m.metadata.insert(
                        MNEMO_META_PART.into(),
                        Value::Number((part_idx as u64).into()),
                    );
                }
                if let Some((sha, len)) = &p.truncation {
                    m.metadata
                        .insert(MNEMO_META_SHA256.into(), Value::String(sha.clone()));
                    m.metadata
                        .insert(MNEMO_META_LENGTH.into(), Value::Number((*len).into()));
                }
                self.put(m)?;
                report.added_records += 1;
            }
        }
        Ok(report)
    }

    /// Export the archive layer for `thread_id` in `_mnemo.seq`
    /// order. Each entry is the Memory as stored; chunks of a
    /// multi-part turn come out in `_mnemo.part` order within their
    /// `_mnemo.seq`. The record `.content` is the text that was
    /// stored (post-chunking / post-truncation); callers that need
    /// the pre-truncation original should round-trip via
    /// `_mnemo.sha256` + `_mnemo.length`.
    pub fn thread_export(&mut self, thread_id: &str) -> Result<Vec<Memory>> {
        // R-004.2: the thread's own records, from the handle caches.
        // FMT-9e: a cold thread's records were just decoded to warm it;
        // reuse them instead of decoding (and decompressing) twice.
        let (entries, mut decoded) = self.thread_entries_decoded(thread_id)?;
        let mut out: Vec<Memory> = Vec::new();
        for e in &entries {
            let m = match decoded.remove(&e.id) {
                Some(m) => self.finish_memory(e, m)?,
                None => self.read_memory(e)?,
            };
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(thread_id) {
                continue;
            }
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_TURN)
            {
                continue;
            }
            out.push(m);
        }
        out.sort_by(|a, b| {
            let seq_a = a
                .metadata
                .get(MNEMO_META_SEQ)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let seq_b = b
                .metadata
                .get(MNEMO_META_SEQ)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let part_a = a
                .metadata
                .get(MNEMO_META_PART)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let part_b = b
                .metadata
                .get(MNEMO_META_PART)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            seq_a.cmp(&seq_b).then_with(|| part_a.cmp(&part_b))
        });
        Ok(out)
    }

    /// Export a thread's archive in the **ingest shape** (Phase 12.7d):
    /// one [`ExportedTurn`] per source turn, in `_mnemo.seq` order, with
    /// a chunked turn's parts joined back into one `content`. Passing
    /// the result back to [`Mnemo::ingest_thread`] for the same thread
    /// skips every turn; for a new thread it re-creates the same records.
    /// This is what Python `db.thread_export`, MCP `thread_export` and
    /// `mnemo thread export` return; [`Mnemo::thread_export`] still
    /// returns the raw records.
    pub fn thread_export_turns(&mut self, thread_id: &str) -> Result<Vec<ExportedTurn>> {
        let records = self.thread_export(thread_id)?;
        let mut out: Vec<ExportedTurn> = Vec::new();
        let mut last_seq: Option<u64> = None;
        for m in records {
            let meta_str = |k: &str| {
                m.metadata
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let seq = m.metadata.get(MNEMO_META_SEQ).and_then(|v| v.as_u64());
            // Records are sorted by (seq, part), so the parts of one turn
            // are adjacent and share a seq.
            if seq.is_some() && seq == last_seq {
                if let Some(prev) = out.last_mut() {
                    prev.content.push_str(&m.content);
                    continue;
                }
            }
            last_seq = seq;
            out.push(ExportedTurn {
                turn_id: meta_str(MNEMO_META_TURN_ID),
                role: meta_str(MNEMO_META_ROLE),
                timestamp: m.created_at,
                sha256: m
                    .metadata
                    .get(MNEMO_META_SHA256)
                    .and_then(|v| v.as_str())
                    .map(String::from),
                original_length: m.metadata.get(MNEMO_META_LENGTH).and_then(|v| v.as_u64()),
                content: m.content,
            });
        }
        Ok(out)
    }

    /// Soft-delete every record in the archive layer of `thread_id`.
    /// Returns the number of records marked deleted. Permanent
    /// removal still requires a subsequent `compact_file`, same as
    /// any `Mnemo::delete`.
    ///
    /// Does **not** touch `_mnemo.kind = "essence"` records or other
    /// non-turn metadata the thread may carry — only `_mnemo.kind =
    /// "turn"` records matching the thread id.
    pub fn thread_forget(&mut self, thread_id: &str) -> Result<usize> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        // Collect first, then delete — can't hold catalog borrows
        // across `self.delete`.
        let entries = self.thread_entries(thread_id)?;
        let mut victims: Vec<Ulid> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(thread_id) {
                continue;
            }
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_TURN)
            {
                continue;
            }
            victims.push(m.id);
        }
        let n = victims.len();
        for id in &victims {
            self.delete(id)?;
        }
        Ok(n)
    }

    // --- Phase 12.7b: essence (timeline + sections) -----------------
}
