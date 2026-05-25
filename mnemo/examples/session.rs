//! Session lifecycle: hold a conversation, then consolidate it to memory.
//!
//! Run with: `cargo run --example session`

use mnemo::{Mnemo, MnemoConfig, RecallRequest, Result, Turn};

fn main() -> Result<()> {
    let path = std::env::temp_dir().join("mnemo-session.mnemo");
    let _ = std::fs::remove_file(&path);

    let cfg = MnemoConfig { dimensions: 4, ..Default::default() };
    let mut db = Mnemo::create(&path, "session-passphrase", cfg)?;

    // --- a conversation -------------------------------------------------
    // A session bundles an agent with a fresh session id and records each
    // turn as working memory. Embeddings are caller-supplied.
    {
        let mut chat = db.session("assistant");
        println!("opened session {}", chat.id());

        chat.add_turn(Turn::user("my flight is on Friday", vec![1.0, 0.0, 0.0, 0.0]))?;
        chat.add_turn(Turn::assistant("noted — Friday it is", vec![0.9, 0.1, 0.0, 0.0]))?;
        chat.add_turn(Turn::user("book me an aisle seat", vec![0.0, 1.0, 0.0, 0.0]))?;

        // Mid-conversation recall, scoped to this agent.
        let context = chat.recall(RecallRequest::new(vec![1.0, 0.0, 0.0, 0.0]).top_k(3))?;
        println!("recalled {} memories for context", context.len());

        // Closing the session consolidates the 3 working turns into
        // episodic memory — "what happened".
        let promoted = chat.close()?;
        println!("session closed: {promoted} turns consolidated to episodic memory");
    }

    // --- after the session ---------------------------------------------
    // The turns now live on as durable episodic memories.
    let episodic = db
        .memories()?
        .into_iter()
        .filter(|m| matches!(m.memory_type, mnemo::MemoryType::Episodic))
        .count();
    println!("\n{episodic} episodic memories persisted from the conversation");

    db.close()?;
    let _ = std::fs::remove_file(&path);
    Ok(())
}
