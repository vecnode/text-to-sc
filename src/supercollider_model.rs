//! Serializable sketch of SuperCollider server / graph state (placeholder until OSC bridge exists).
//!
//! MCP tools will eventually populate this from real scsynth data; for now it anchors future API shape.

use serde::{Deserialize, Serialize};

/// High-level representation of the SuperCollider server state.
/// This is currently just a placeholder; it will be filled in when
/// we start exporting the node tree, buses, buffers, etc. from SC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupercolliderServerState {
    pub sample_rate: Option<f32>,
    pub num_output_channels: Option<u32>,
    pub nodes: Vec<SupercolliderNodeSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupercolliderNodeSummary {
    pub id: i32,
    pub name: Option<String>,
    pub kind: SupercolliderNodeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SupercolliderNodeKind {
    Group,
    Synth,
}

impl SupercolliderServerState {
    /// Build a tiny placeholder snapshot so the model is exercised end-to-end
    /// before OSC wiring is implemented.
    pub fn bootstrap_placeholder() -> Self {
        Self {
            sample_rate: None,
            num_output_channels: None,
            nodes: vec![
                SupercolliderNodeSummary {
                    id: 0,
                    name: Some("root".to_string()),
                    kind: SupercolliderNodeKind::Group,
                },
                SupercolliderNodeSummary {
                    id: 1000,
                    name: Some("example_synth".to_string()),
                    kind: SupercolliderNodeKind::Synth,
                },
            ],
        }
    }
}

// TODO: this module will ingest and normalize JSON snapshots sent by a future
// SuperCollider bridge process (communicating via OSC).
// TODO: add conversion utilities from raw SC graph dumps into `SupercolliderServerState`.


