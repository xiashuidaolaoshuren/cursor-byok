//! Converts completed Tool work into canonical Tool results.
mod exec;
mod gate;
mod images;
mod interaction;
mod local;
mod mcp;
mod mcp_state;
mod search;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    cursor::protocol::proto::agent::v1 as pb,
    model::{ToolCall, ToolImageReference, ToolResult},
    store::BlobId,
    Error, Result,
};

use super::runtime::now_ms;

pub(crate) use exec::{edit_failure, from_exec};
pub(crate) use images::ToolImage;
pub(crate) use interaction::{complete_web_fetch, complete_web_search, from_interaction};
pub(crate) use local::{local, subagents_disabled, todo_items};
pub(crate) use mcp::failure as mcp_failure;
pub(crate) use search::complete as semble;

#[derive(Clone, Debug)]
pub struct ToolCompletion {
    result: ToolResult,
    tool_call: pb::ToolCall,
    images: Vec<ToolImage>,
}

impl ToolCompletion {
    pub fn result(&self) -> &ToolResult {
        &self.result
    }

    pub fn tool_call(&self) -> &pb::ToolCall {
        &self.tool_call
    }

    pub(super) fn with_images(mut self, images: Vec<ToolImage>) -> Self {
        self.images = images;
        self
    }

    pub(crate) fn take_images(&mut self) -> Vec<ToolImage> {
        std::mem::take(&mut self.images)
    }

    /// Attaches stored Blobs to the canonical result. Read keeps a Cursor
    /// presentation detail: the client renders the file from the BlobID
    /// instead of receiving the bytes inline a second time.
    pub(crate) fn persist_images(&mut self, stored: &[(BlobId, ToolImage)]) -> Result<()> {
        self.result.images = stored
            .iter()
            .map(|(blob_id, image)| ToolImageReference {
                blob_id: blob_id.to_base64(),
                mime_type: image.mime_type.clone(),
                path: image.path.clone(),
            })
            .collect();
        let [(blob_id, _)] = stored else {
            return Ok(());
        };
        let Some(pb::tool_call::Tool::ReadToolCall(call)) = self.tool_call.tool.as_mut() else {
            return Ok(());
        };
        let Some(pb::read_tool_result::Result::Success(success)) = call
            .result
            .as_mut()
            .and_then(|result| result.result.as_mut())
        else {
            return Err(Error::Protocol(
                "Read image completion has no success state".into(),
            ));
        };
        success.output = Some(pb::read_tool_success::Output::DataBlobId(
            blob_id.as_bytes().to_vec(),
        ));
        Ok(())
    }

    pub(crate) fn new(
        call: &ToolCall,
        started_at_ms: u64,
        mut result: ToolResult,
        mut tool: pb::tool_call::Tool,
    ) -> Self {
        // Apply the model-visible size gate once, at the tool completion
        // boundary. Canonical history and every provider projection then
        // carry the same bounded result without reprocessing it.
        gate::tool_completion(&call.name, &mut tool, &mut result.content);
        Self {
            result,
            tool_call: pb::ToolCall {
                tool_call_id: Some(call.call_id.clone()),
                started_at_ms: Some(started_at_ms),
                completed_at_ms: Some(now_ms()),
                tool: Some(tool),
                hook_additional_contexts: Vec::new(),
            },
            images: Vec::new(),
        }
    }

    pub(super) fn from_rendered(
        call: &ToolCall,
        started_at_ms: u64,
        output: String,
        is_error: bool,
        rendered: pb::ToolCall,
    ) -> Result<Self> {
        let tool = rendered.tool.ok_or_else(|| {
            Error::Protocol(format!("tool {} has no Cursor representation", call.name))
        })?;
        Ok(Self::new(
            call,
            started_at_ms,
            ToolResult {
                call_id: call.call_id.clone(),
                content: output,
                is_error,
                images: Vec::new(),
            },
            tool,
        ))
    }
}

#[derive(Clone)]
pub struct ToolResultSender(mpsc::UnboundedSender<Result<ToolCompletion>>);
pub struct ToolResultReceiver(mpsc::UnboundedReceiver<Result<ToolCompletion>>);

pub fn tool_result_channel() -> (ToolResultSender, ToolResultReceiver) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (ToolResultSender(sender), ToolResultReceiver(receiver))
}

impl ToolResultSender {
    pub fn send(&self, result: ToolCompletion) {
        let _ = self.0.send(Ok(result));
    }

    pub fn send_error(&self, error: Error) {
        let _ = self.0.send(Err(error));
    }
}

impl ToolResultReceiver {
    pub async fn recv(&mut self) -> Option<Result<ToolCompletion>> {
        self.0.recv().await
    }
}

pub(super) fn prost_json(value: &prost_types::Value) -> Value {
    use prost_types::value::Kind;
    match value.kind.as_ref() {
        None | Some(Kind::NullValue(_)) => Value::Null,
        Some(Kind::NumberValue(value)) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Some(Kind::StringValue(value)) => Value::String(value.clone()),
        Some(Kind::BoolValue(value)) => Value::Bool(*value),
        Some(Kind::StructValue(value)) => Value::Object(
            value
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), prost_json(value)))
                .collect(),
        ),
        Some(Kind::ListValue(value)) => Value::Array(value.values.iter().map(prost_json).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str) -> ToolCall {
        ToolCall {
            index: 0,
            call_id: "call-1".into(),
            model_call_id: "model:0".into(),
            name: name.into(),
            arguments_text: String::new(),
            arguments: json!({}),
            argument_error: None,
        }
    }

    fn image(path: &str) -> ToolImage {
        ToolImage {
            data: images::TEST_PNG.to_vec(),
            mime_type: "image/png".into(),
            path: path.into(),
        }
    }

    fn completion(name: &str, tool: pb::tool_call::Tool) -> ToolCompletion {
        ToolCompletion::new(
            &call(name),
            0,
            ToolResult {
                call_id: "call-1".into(),
                content: "tool text".into(),
                is_error: false,
                images: Vec::new(),
            },
            tool,
        )
    }

    fn read_tool() -> pb::tool_call::Tool {
        pb::tool_call::Tool::ReadToolCall(pb::ReadToolCall {
            args: None,
            result: Some(pb::ReadToolResult {
                result: Some(pb::read_tool_result::Result::Success(pb::ReadToolSuccess {
                    path: "/tmp/frame.png".into(),
                    output: Some(pb::read_tool_success::Output::Data(
                        images::TEST_PNG.to_vec(),
                    )),
                    ..Default::default()
                })),
            }),
        })
    }

    #[test]
    fn read_images_are_attached_and_rendered_from_the_blob() {
        let mut completion = completion("Read", read_tool());
        let image = image("/tmp/frame.png");
        let blob_id = BlobId::digest(&image.data);
        completion
            .persist_images(&[(blob_id.clone(), image)])
            .unwrap();

        assert_eq!(completion.result().images.len(), 1);
        assert_eq!(completion.result().images[0].blob_id, blob_id.to_base64());
        let Some(pb::tool_call::Tool::ReadToolCall(read)) = completion.tool_call().tool.as_ref()
        else {
            panic!("expected Read tool state")
        };
        let Some(pb::read_tool_result::Result::Success(success)) = read
            .result
            .as_ref()
            .and_then(|result| result.result.as_ref())
        else {
            panic!("expected Read success")
        };
        assert_eq!(
            success.output,
            Some(pb::read_tool_success::Output::DataBlobId(
                blob_id.as_bytes().to_vec()
            ))
        );
    }

    #[test]
    fn mcp_completions_attach_every_image_without_rewriting_tool_state() {
        let mut completion = completion(
            "look",
            pb::tool_call::Tool::McpToolCall(pb::McpToolCall {
                args: None,
                result: None,
                description: None,
            }),
        );
        let first = image("");
        let second = ToolImage {
            data: images::TEST_GIF.to_vec(),
            mime_type: "image/gif".into(),
            path: String::new(),
        };
        let stored = vec![
            (BlobId::digest(&first.data), first),
            (BlobId::digest(&second.data), second),
        ];
        completion.persist_images(&stored).unwrap();

        assert_eq!(completion.result().images.len(), 2);
        assert_eq!(completion.result().images[1].mime_type, "image/gif");
        let Some(pb::tool_call::Tool::McpToolCall(mcp)) = completion.tool_call().tool.as_ref()
        else {
            panic!("expected MCP tool state")
        };
        assert!(
            mcp.result.is_none(),
            "persisting images must not fabricate a tool result"
        );
    }
}
