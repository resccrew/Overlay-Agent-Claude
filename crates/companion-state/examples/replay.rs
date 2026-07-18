//! Rust counterpart to `tools/transcript-watcher/derive-state.mjs`: replay a
//! real transcript file through `StateMachine` and print the same summary
//! shape, so the two independent implementations can be diffed against each
//! other on the same real input as a cross-check (see
//! `tools/e2e-status-check.sh`).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use companion_state::StateMachine;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: replay <path-to-transcript.jsonl>");
        std::process::exit(1);
    });

    let file = std::fs::File::open(PathBuf::from(&path)).unwrap_or_else(|e| {
        eprintln!("failed to open {path}: {e}");
        std::process::exit(1);
    });

    let mut machine = StateMachine::new();
    let mut total_lines = 0usize;
    let mut transitions = Vec::new();

    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let Ok(raw) = line else { continue };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        total_lines = idx + 1;
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) else { continue };
        if let Some(next) = machine.apply(&parsed) {
            transitions.push((idx + 1, next));
        }
    }

    println!("Replayed {total_lines} lines, {} state transitions.", transitions.len());
    println!("Final state: {}", state_name(machine.current()));

    let mut counts = std::collections::BTreeMap::new();
    for (_, s) in &transitions {
        *counts.entry(state_name(*s)).or_insert(0usize) += 1;
    }
    println!("\nTransition counts by state:");
    for (state, n) in &counts {
        println!("  {state}: {n}");
    }
}

fn state_name(s: companion_state::CompanionState) -> &'static str {
    use companion_state::CompanionState::*;
    match s {
        Idle => "idle",
        Roaming => "roaming",
        Thinking => "thinking",
        Typing => "typing",
        WaitingForInput => "waitingForInput",
        Error => "error",
        Done => "done",
    }
}
