//! Heuristic stall detection for tool-heavy turns with no forward progress.

use clido_tools::ToolOutput;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub(crate) struct StallTracker {
    score: u32,
    last_batch_fp: Option<u64>,
}

impl StallTracker {
    pub(crate) fn new() -> Self {
        Self {
            score: 0,
            last_batch_fp: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.score = 0;
        self.last_batch_fp = None;
    }

    pub(crate) fn score(&self) -> u32 {
        self.score
    }

    /// Observe a completed tool batch. Increments score on repeated batches or all-error batches.
    pub(crate) fn observe_batch(
        &mut self,
        tool_uses: &[(String, String, Value)],
        outputs: &[(ToolOutput, u64)],
    ) {
        let fp = fingerprint_batch(tool_uses);
        if Some(fp) == self.last_batch_fp {
            self.score = self.score.saturating_add(3);
        }
        self.last_batch_fp = Some(fp);

        if !outputs.is_empty() && outputs.iter().all(|(o, _)| o.is_error) {
            self.score = self.score.saturating_add(2);
        } else if outputs.iter().any(|(o, _)| !o.is_error) {
            self.score = self.score.saturating_sub(1);
        }
    }
}

fn fingerprint_batch(tool_uses: &[(String, String, Value)]) -> u64 {
    let mut h = DefaultHasher::new();
    for (_id, name, inp) in tool_uses {
        name.hash(&mut h);
        serde_json::to_string(inp).unwrap_or_default().hash(&mut h);
    }
    h.finish()
}
