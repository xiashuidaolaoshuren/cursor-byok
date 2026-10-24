//! Defines Tool calls, results, and Tool round identities.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ProviderReplayState;

pub fn normalize_tool_name(name: &str) -> String {
    let normalized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if normalized.is_empty() {
        "_".into()
    } else {
        normalized
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub index: usize,
    pub call_id: String,
    pub model_call_id: String,
    pub name: String,
    pub arguments_text: String,
    pub arguments: Value,
    pub argument_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
    /// Images returned by the Tool, in the order the Tool produced them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolImageReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolImageReference {
    pub blob_id: String,
    pub mime_type: String,
    /// Source path when the Tool read a file, otherwise empty.
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolRoundAssistant {
    pub text: String,
    pub thinking: String,
    pub model_call_id: String,
    pub replay_state: Option<ProviderReplayState>,
}
