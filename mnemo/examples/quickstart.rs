//! Quickstart: create an encrypted store, remember a few things, and recall.
//!
//! Run with: `cargo run --example quickstart`

use mnemo::{Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest, Result};

fn main() -> Result<()> {
    // Use a temporary path so the example is self-cleaning.
    let path = std::env::temp_dir().join("mnemo-quickstart.mnemo");
    let _ = std::fs::remove_file(&path);

    // Small dimensionality keeps the example fast.
    let cfg = MnemoConfig { dimensions: 8, ..Default::default() };
    let mut db = Mnemo::create(&path, "quickstart-passphrase", cfg)?;

    // Store a few memories of different types.
    db.remember(
        Memory::new(
            "the user's name is Ada",
            MemoryType::Semantic,
            vec![1.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0],
        )
        .with_agent("assistant")
        .with_importance(0.9),
    )?;
    db.remember(
        Memory::new(
            "user asked for a refund on 2026-05-20",
            MemoryType::Episodic,
            vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.0],
        )
        .with_agent("assistant")
        .with_importance(0.5),
    )?;
    db.remember(
        Memory::new(
            "to escalate, tag the ticket 'priority' and notify a human",
            MemoryType::Procedural,
            vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.1, 0.0],
        )
        .with_agent("assistant")
        .with_importance(0.7),
    )?;

    // Durably persist.
    db.flush()?;
    println!("stored {} memories", db.len());

    // Multi-signal recall: nearest to the "user identity" region.
    let query = vec![0.95, 0.05, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0];
    let hits = db.recall(&RecallRequest::new(query).top_k(3))?;

    println!("\ntop recalls:");
    for h in hits {
        println!(
            "  score={:.3}  sim={:.3}  [{}]  {}",
            h.score,
            h.similarity,
            h.memory.memory_type.as_str(),
            h.memory.content
        );
    }

    db.close()?;
    let _ = std::fs::remove_file(&path);
    Ok(())
}
