//! Frames used only after a successful CuaAttach upgrade.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum CuaStreamFrame {
    Message { message: serde_json::Value },
    Closed { reason: String },
}
