//! Parses and persists command execution output.
use crate::{
    cursor::{protocol::proto::agent::v1 as pb, tools::tool_call_result::images::ToolImage},
    model::ToolCall,
    Error, Result,
};

/// Model-visible Tool output: its text, error state, and any returned images.
pub(super) struct Output {
    pub(super) content: String,
    pub(super) is_error: bool,
    pub(super) images: Vec<ToolImage>,
}

impl Output {
    fn text(content: impl Into<String>, is_error: bool) -> Self {
        Self {
            content: content.into(),
            is_error,
            images: Vec::new(),
        }
    }
}

pub(super) fn output(
    message: &pb::exec_client_message::Message,
    call: &ToolCall,
) -> Result<Output> {
    use pb::exec_client_message::Message;
    Ok(match message {
        Message::ShellResult(value) | Message::MiniSweAgentBashResult(value) => {
            let (content, is_error) = shell(value)?;
            Output::text(content, is_error)
        }
        Message::ReadResult(value) | Message::RedactedReadResult(value) => read(value)?,
        Message::WriteResult(value) => {
            let (content, is_error) = write(value)?;
            Output::text(content, is_error)
        }
        Message::DeleteResult(value) => {
            let (content, is_error) = delete(value)?;
            Output::text(content, is_error)
        }
        Message::GrepResult(value) => {
            let (content, is_error) = grep(value)?;
            Output::text(content, is_error)
        }
        Message::DiagnosticsResult(value) => diagnostics(value, call)?,
        Message::McpResult(value) => mcp(value)?,
        Message::ReadMcpResourceExecResult(value) => read_mcp(value)?,
        Message::SubagentResult(value) => task(value, call)?,
        _ => {
            return Err(Error::Protocol(
                "unsupported terminal ExecClientMessage".into(),
            ))
        }
    })
}

fn shell(value: &pb::ShellResult) -> Result<(String, bool)> {
    use pb::shell_result::Result as R;
    let output = match value.result.as_ref().ok_or_else(|| missing("shell"))? {
        R::Success(success) if value.is_background == Some(true) => {
            let mut fields = vec![format!("shell_id={}", success.shell_id.unwrap_or_default())];
            if let Some(pid) = success.pid.or(value.pid) {
                fields.push(format!("pid={pid}"));
            }
            if let Some(folder) = value.terminals_folder.as_deref().filter(|v| !v.is_empty()) {
                fields.push(format!("terminals_folder={folder}"));
            }
            let output = streams(&success.stdout, &success.stderr);
            let prefix = format!("shell running in background {}", fields.join(" "));
            return Ok((
                if output == "shell completed without output" {
                    prefix
                } else {
                    format!("{prefix}\n{output}")
                },
                false,
            ));
        }
        R::Success(success) => return Ok((streams(&success.stdout, &success.stderr), false)),
        R::Failure(failure) => streams(&failure.stdout, &failure.stderr),
        R::Timeout(timeout) => format!(
            "shell timed out after {}ms in {}",
            timeout.timeout_ms, timeout.working_directory
        ),
        R::Rejected(rejected) => rejected.reason.clone(),
        R::SpawnError(error) => error.error.clone(),
        R::PermissionDenied(denied) => denied.error.clone(),
    };
    Ok((output, true))
}

fn streams(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n\n<stderr>\n{stderr}\n</stderr>"),
        (false, true) => stdout.into(),
        (true, false) => stderr.into(),
        (true, true) => "shell completed without output".into(),
    }
}

fn read(value: &pb::ReadResult) -> Result<Output> {
    use pb::{read_result::Result as R, read_success::Output as ReadOutput};
    let image = match value.result.as_ref() {
        Some(R::Success(success)) => match success.output.as_ref() {
            Some(ReadOutput::Data(data)) => ToolImage::new(data.clone(), success.path.clone()),
            _ => None,
        },
        _ => None,
    };
    let (content, is_error) = match value.result.as_ref().ok_or_else(|| missing("read"))? {
        R::Success(success) => (
            match success.output.as_ref() {
                Some(ReadOutput::Content(text)) => text.clone(),
                Some(ReadOutput::Data(bytes)) => format!("read binary bytes={}", bytes.len()),
                None => format!("read success path={}", success.path),
            },
            false,
        ),
        R::Error(error) => (error.error.clone(), true),
        R::Rejected(rejected) => (rejected.reason.clone(), true),
        R::FileNotFound(value) => (format!("file not found: {}", value.path), true),
        R::PermissionDenied(value) => (format!("permission denied: {}", value.path), true),
        R::InvalidFile(value) => (value.reason.clone(), true),
    };
    Ok(Output {
        content,
        is_error,
        images: image.into_iter().collect(),
    })
}

fn write(value: &pb::WriteResult) -> Result<(String, bool)> {
    use pb::write_result::Result as R;
    match value.result.as_ref().ok_or_else(|| missing("write"))? {
        R::Success(success) => Ok((
            success.file_content_after_write.clone().unwrap_or_else(|| {
                format!(
                    "write success path={} lines={}",
                    success.path, success.lines_created
                )
            }),
            false,
        )),
        R::PermissionDenied(value) => Ok((value.error.clone(), true)),
        R::NoSpace(value) => Ok((format!("no space left: {}", value.path), true)),
        R::Error(value) => Ok((value.error.clone(), true)),
        R::Rejected(value) => Ok((value.reason.clone(), true)),
    }
}

fn delete(value: &pb::DeleteResult) -> Result<(String, bool)> {
    use pb::delete_result::Result as R;
    match value.result.as_ref().ok_or_else(|| missing("delete"))? {
        R::Success(value) => Ok((format!("delete success path={}", value.path), false)),
        R::FileNotFound(value) => Ok((format!("file not found: {}", value.path), true)),
        R::NotFile(value) => Ok((format!("not file: {}", value.path), true)),
        R::PermissionDenied(value) => Ok((value.client_visible_error.clone(), true)),
        R::FileBusy(value) => Ok((format!("file busy: {}", value.path), true)),
        R::Rejected(value) => Ok((value.reason.clone(), true)),
        R::Error(value) => Ok((value.error.clone(), true)),
    }
}

fn grep(value: &pb::GrepResult) -> Result<(String, bool)> {
    use pb::grep_result::Result as R;
    match value.result.as_ref().ok_or_else(|| missing("grep"))? {
        R::Success(value) => Ok((grep_success(value), false)),
        R::Error(value) => Ok((value.error.clone(), true)),
    }
}

fn grep_success(value: &pb::GrepSuccess) -> String {
    let mut lines = Vec::new();
    if let Some(result) = &value.active_editor_result {
        grep_union(result, &mut lines);
    }
    let mut workspaces = value.workspace_results.iter().collect::<Vec<_>>();
    workspaces.sort_unstable_by_key(|(name, _)| *name);
    for (_, result) in workspaces {
        grep_union(result, &mut lines);
    }
    if lines.is_empty() {
        format!(
            "No matches found for pattern `{}` in {}",
            value.pattern, value.path
        )
    } else {
        lines.join("\n")
    }
}

fn grep_union(value: &pb::GrepUnionResult, lines: &mut Vec<String>) {
    use pb::grep_union_result::Result as R;
    match value.result.as_ref() {
        Some(R::Files(value)) => {
            lines.extend(value.files.iter().cloned());
            grep_truncation(
                value.client_truncated,
                value.ripgrep_truncated,
                value.total_files,
                "files",
                lines,
            );
        }
        Some(R::Count(value)) => {
            lines.extend(
                value
                    .counts
                    .iter()
                    .map(|count| format!("{}:{}", count.file, count.count)),
            );
            grep_truncation(
                value.client_truncated,
                value.ripgrep_truncated,
                value.total_matches,
                "matches",
                lines,
            );
        }
        Some(R::Content(value)) => {
            for file in &value.matches {
                lines.extend(file.matches.iter().map(|matched| {
                    let separator = if matched.is_context_line { '-' } else { ':' };
                    let truncated = if matched.content_truncated {
                        " [line truncated]"
                    } else {
                        ""
                    };
                    format!(
                        "{}{separator}{}{separator}{}{truncated}",
                        file.file, matched.line_number, matched.content
                    )
                }));
            }
            grep_truncation(
                value.client_truncated,
                value.ripgrep_truncated,
                value.total_matched_lines,
                "matched lines",
                lines,
            );
        }
        None => {}
    }
}

fn grep_truncation(
    client_truncated: bool,
    ripgrep_truncated: bool,
    total: i32,
    unit: &str,
    lines: &mut Vec<String>,
) {
    if client_truncated || ripgrep_truncated {
        lines.push(format!("[Results truncated; {total} total {unit}]"));
    }
}

fn diagnostics(value: &pb::DiagnosticsResult, call: &ToolCall) -> Result<Output> {
    use pb::diagnostics_result::Result as R;
    Ok(
        match value
            .result
            .as_ref()
            .ok_or_else(|| missing("diagnostics"))?
        {
            R::Success(value) => Output::text(diagnostics_success(value, call), false),
            R::Error(value) => Output::text(value.error.clone(), true),
            R::Rejected(value) => Output::text(value.reason.clone(), true),
            R::FileNotFound(value) => Output::text(format!("file not found: {}", value.path), true),
            R::PermissionDenied(value) => {
                Output::text(format!("permission denied: {}", value.path), true)
            }
        },
    )
}

/// `codec::request` encodes only `paths[0]` into the `DiagnosticsArgs` exec, and
/// `DiagnosticsSuccess` carries a single `path`, so a multi-path ReadLints call
/// only ever inspects the first entry. Name the rest instead of letting a clean
/// result for one file read as a clean bill of health for all of them.
fn unchecked_lint_paths(call: &ToolCall) -> Vec<&str> {
    call.arguments
        .get("paths")
        .and_then(serde_json::Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .skip(1)
                .filter_map(serde_json::Value::as_str)
                .filter(|path| !path.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn diagnostics_success(value: &pb::DiagnosticsSuccess, call: &ToolCall) -> String {
    let unchecked = unchecked_lint_paths(call);
    let notice = (!unchecked.is_empty()).then(|| {
        format!(
            "[Only {} was checked; ReadLints reads one path per call. Not checked: {}]",
            value.path,
            unchecked.join(", ")
        )
    });
    if value.diagnostics.is_empty() {
        let clean = format!("No diagnostics found in {}", value.path);
        return match notice {
            Some(notice) => format!("{clean}\n{notice}"),
            None => clean,
        };
    }
    let mut lines = value
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let location = diagnostic_location(&value.path, diagnostic.range.as_ref());
            let mut labels = vec![diagnostic_severity(diagnostic.severity)];
            if !diagnostic.source.is_empty() {
                labels.push(diagnostic.source.as_str());
            }
            if !diagnostic.code.is_empty() {
                labels.push(diagnostic.code.as_str());
            }
            if diagnostic.is_stale {
                labels.push("stale");
            }
            format!(
                "{}: [{}] {}",
                location,
                labels.join(" "),
                diagnostic.message
            )
        })
        .collect::<Vec<_>>();
    if value.total_diagnostics != value.diagnostics.len() as i32 {
        lines.push(format!(
            "[Reported {} diagnostics; received {} details]",
            value.total_diagnostics,
            value.diagnostics.len()
        ));
    }
    lines.extend(notice);
    lines.join("\n")
}

fn diagnostic_location(path: &str, range: Option<&pb::Range>) -> String {
    let Some(range) = range else {
        return path.into();
    };
    let Some(start) = &range.start else {
        return path.into();
    };
    let mut location = format!(
        "{}:{}:{}",
        path,
        start.line.saturating_add(1),
        start.column.saturating_add(1)
    );
    if let Some(end) = &range.end {
        location.push_str(&format!(
            "-{}:{}",
            end.line.saturating_add(1),
            end.column.saturating_add(1)
        ));
    }
    location
}

fn diagnostic_severity(value: i32) -> &'static str {
    match pb::DiagnosticSeverity::try_from(value) {
        Ok(pb::DiagnosticSeverity::Error) => "error",
        Ok(pb::DiagnosticSeverity::Warning) => "warning",
        Ok(pb::DiagnosticSeverity::Information) => "information",
        Ok(pb::DiagnosticSeverity::Hint) => "hint",
        Ok(pb::DiagnosticSeverity::Unspecified) | Err(_) => "diagnostic",
    }
}

fn mcp(value: &pb::McpResult) -> Result<Output> {
    use pb::mcp_result::Result as R;
    Ok(match value.result.as_ref().ok_or_else(|| missing("mcp"))? {
        R::Success(value) => mcp_output(value)?,
        R::Error(value) => Output::text(value.error.clone(), true),
        R::Rejected(value) => Output::text(value.reason.clone(), true),
        R::PermissionDenied(value) => Output::text(value.error.clone(), true),
        R::ToolNotFound(value) => Output::text(format!("MCP tool not found: {}", value.name), true),
        R::ServerNotFound(value) => {
            Output::text(format!("MCP server not found: {}", value.name), true)
        }
        R::Approved(_) => return Err(Error::Protocol("MCP approval is not terminal".into())),
    })
}

/// Keeps the Tool's text and its images together. Images that fail validation
/// stay as an explicit text notice so the model never assumes it saw pixels.
fn mcp_output(success: &pb::McpSuccess) -> Result<Output> {
    let mut content = Vec::new();
    let mut images = Vec::new();
    for item in &success.content {
        match item.content.as_ref() {
            Some(pb::mcp_tool_result_content_item::Content::Text(text)) => {
                if !text.text.is_empty() {
                    content.push(text.text.clone());
                }
                if let Some(location) = &text.output_location {
                    content.push(format!(
                        "MCP output file: {} ({} bytes, {} lines)",
                        location.file_path, location.size_bytes, location.line_count
                    ));
                }
            }
            Some(pb::mcp_tool_result_content_item::Content::Image(image)) => {
                let bytes = image.data.len();
                match ToolImage::new(image.data.clone(), "") {
                    Some(image) => {
                        content.push(format!("MCP image: {} ({} bytes)", image.mime_type, bytes));
                        images.push(image);
                    }
                    None => content.push(format!(
                        "MCP image rejected: {} ({} bytes)",
                        image.mime_type, bytes
                    )),
                }
            }
            None => {}
        }
    }
    if let Some(structured) = &success.structured_content {
        let value = serde_json::Value::Object(
            structured
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), super::super::prost_json(value)))
                .collect(),
        );
        content.push(serde_json::to_string_pretty(&value)?);
    }
    Ok(Output {
        content: if content.is_empty() {
            "MCP tool completed without content".into()
        } else {
            content.join("\n\n")
        },
        is_error: success.is_error,
        images,
    })
}

fn read_mcp(value: &pb::ReadMcpResourceExecResult) -> Result<Output> {
    use pb::read_mcp_resource_exec_result::Result as R;
    Ok(
        match value
            .result
            .as_ref()
            .ok_or_else(|| missing("read MCP resource"))?
        {
            R::Success(value) => match value.content.as_ref() {
                Some(pb::read_mcp_resource_success::Content::Text(text)) => {
                    Output::text(text.clone(), false)
                }
                Some(pb::read_mcp_resource_success::Content::Blob(blob)) => {
                    match ToolImage::new(blob.clone(), value.uri.clone()) {
                        Some(image) => Output {
                            content: format!(
                                "read MCP resource image: {} ({}, {} bytes)",
                                value.uri,
                                image.mime_type,
                                image.data.len()
                            ),
                            is_error: false,
                            images: vec![image],
                        },
                        None => {
                            Output::text(format!("read MCP resource blob={}", blob.len()), false)
                        }
                    }
                }
                None => Output::text(format!("read MCP resource uri={}", value.uri), false),
            },
            R::Error(value) => Output::text(value.error.clone(), true),
            R::Rejected(value) => Output::text(value.reason.clone(), true),
            R::NotFound(value) => {
                Output::text(format!("MCP resource not found: {}", value.uri), true)
            }
        },
    )
}

fn task(value: &pb::SubagentResult, call: &ToolCall) -> Result<Output> {
    use pb::subagent_result::Result as R;
    Ok(
        match value.result.as_ref().ok_or_else(|| missing("subagent"))? {
            R::Success(value) if creates_subagent(call) => {
                let name = call
                    .arguments
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| Error::Protocol("Task call is missing description".into()))?;
                if value.agent_id.is_empty() {
                    return Err(Error::Protocol("Task result is missing agent_id".into()));
                }
                let identity = format!("Subagent name: {name}\nSubagent ID: {}", value.agent_id);
                let content = value
                    .final_message
                    .as_deref()
                    .filter(|message| !message.is_empty())
                    .map_or(identity.clone(), |message| {
                        format!("{identity}\n\n{message}")
                    });
                Output::text(content, false)
            }
            R::Success(value) => {
                Output::text(value.final_message.clone().unwrap_or_default(), false)
            }
            R::Error(value) => Output::text(value.error.clone(), true),
        },
    )
}

fn creates_subagent(call: &ToolCall) -> bool {
    matches!(
        call.arguments
            .get("resume")
            .and_then(serde_json::Value::as_str),
        None | Some("self")
    )
}

fn missing(name: &str) -> Error {
    Error::Protocol(format!("{name} returned no result"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read_lints(paths: serde_json::Value) -> ToolCall {
        ToolCall {
            index: 0,
            call_id: "lints".into(),
            model_call_id: "model:0".into(),
            name: "ReadLints".into(),
            arguments_text: String::new(),
            arguments: json!({ "paths": paths }),
            argument_error: None,
        }
    }

    fn clean_result(path: &str) -> pb::exec_client_message::Message {
        pb::exec_client_message::Message::DiagnosticsResult(pb::DiagnosticsResult {
            result: Some(pb::diagnostics_result::Result::Success(
                pb::DiagnosticsSuccess {
                    path: path.into(),
                    diagnostics: Vec::new(),
                    total_diagnostics: 0,
                },
            )),
        })
    }

    fn mcp_call() -> ToolCall {
        ToolCall {
            index: 0,
            call_id: "mcp-call".into(),
            model_call_id: "model:0".into(),
            name: "look".into(),
            arguments_text: String::new(),
            arguments: json!({}),
            argument_error: None,
        }
    }

    fn mcp_text(text: &str) -> pb::McpToolResultContentItem {
        pb::McpToolResultContentItem {
            content: Some(pb::mcp_tool_result_content_item::Content::Text(
                pb::McpTextContent {
                    text: text.into(),
                    output_location: None,
                },
            )),
        }
    }

    fn mcp_image(mime_type: &str, data: &[u8]) -> pb::McpToolResultContentItem {
        pb::McpToolResultContentItem {
            content: Some(pb::mcp_tool_result_content_item::Content::Image(
                pb::McpImageContent {
                    data: data.to_vec(),
                    mime_type: mime_type.into(),
                },
            )),
        }
    }

    fn mcp_result(content: Vec<pb::McpToolResultContentItem>) -> pb::exec_client_message::Message {
        pb::exec_client_message::Message::McpResult(pb::McpResult {
            result: Some(pb::mcp_result::Result::Success(pb::McpSuccess {
                content,
                is_error: false,
                structured_content: None,
            })),
        })
    }

    /// The Blender `look` shape: text and image items interleaved. Every valid
    /// image must reach the model, in the order the server returned it.
    #[test]
    fn mcp_images_follow_their_text_in_order() {
        let message = mcp_result(vec![
            mcp_text("first"),
            mcp_image(
                "image/png",
                crate::cursor::tools::tool_call_result::images::TEST_PNG,
            ),
            mcp_text("second"),
            mcp_image(
                "image/gif",
                crate::cursor::tools::tool_call_result::images::TEST_GIF,
            ),
        ]);
        let output = output(&message, &mcp_call()).unwrap();
        assert_eq!(
            output.content,
            format!(
                "first\n\nMCP image: image/png ({} bytes)\n\nsecond\n\nMCP image: image/gif ({} bytes)",
                crate::cursor::tools::tool_call_result::images::TEST_PNG.len(),
                crate::cursor::tools::tool_call_result::images::TEST_GIF.len(),
            )
        );
        assert_eq!(output.images.len(), 2);
        assert_eq!(output.images[0].mime_type, "image/png");
        assert_eq!(output.images[1].mime_type, "image/gif");
    }

    /// A declared MIME type is not proof of pixels; the model must not be told
    /// it received an image it cannot decode.
    #[test]
    fn undecodable_mcp_images_are_reported_as_rejected() {
        let message = mcp_result(vec![
            mcp_image("image/png", b"not an image"),
            mcp_image(
                "image/png",
                crate::cursor::tools::tool_call_result::images::TEST_PNG,
            ),
        ]);
        let output = output(&message, &mcp_call()).unwrap();
        assert!(output
            .content
            .contains("MCP image rejected: image/png (12 bytes)"));
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].mime_type, "image/png");
    }

    #[test]
    fn text_only_mcp_results_keep_their_text_and_attach_no_images() {
        let message = mcp_result(vec![mcp_text("navigated")]);
        let output = output(&message, &mcp_call()).unwrap();
        assert_eq!(output.content, "navigated");
        assert!(output.images.is_empty());
    }

    #[test]
    fn mcp_resource_image_blobs_are_attached() {
        let message = pb::exec_client_message::Message::ReadMcpResourceExecResult(
            pb::ReadMcpResourceExecResult {
                result: Some(pb::read_mcp_resource_exec_result::Result::Success(
                    pb::ReadMcpResourceSuccess {
                        uri: "blender://viewport".into(),
                        mime_type: Some("image/png".into()),
                        content: Some(pb::read_mcp_resource_success::Content::Blob(
                            crate::cursor::tools::tool_call_result::images::TEST_PNG.to_vec(),
                        )),
                        ..Default::default()
                    },
                )),
            },
        );
        let output = output(&message, &mcp_call()).unwrap();
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].mime_type, "image/png");
        assert_eq!(output.images[0].path, "blender://viewport");
        assert!(output.content.contains("read MCP resource image"));
    }

    /// The codec encodes only `paths[0]` into the DiagnosticsArgs exec, so a
    /// clean result for that one path must not read as "these files are all
    /// clean" for the paths that were never looked at.
    #[test]
    fn read_lints_names_the_paths_it_did_not_check() {
        let call = read_lints(json!(["a.ts", "b.ts", "c.ts"]));
        let output = output(&clean_result("a.ts"), &call).unwrap();
        assert!(!output.is_error);
        let content = output.content;
        assert!(content.contains("a.ts"));
        assert!(
            content.contains("b.ts") && content.contains("c.ts"),
            "unchecked paths must be reported, got: {content}"
        );
    }

    /// The overwhelmingly common single-path call must be byte-for-byte
    /// unchanged.
    #[test]
    fn read_lints_single_path_result_is_unchanged() {
        let call = read_lints(json!(["a.ts"]));
        let output = output(&clean_result("a.ts"), &call).unwrap();
        assert_eq!(output.content, "No diagnostics found in a.ts");
    }

    /// The notice belongs on a result that did report diagnostics too: those
    /// diagnostics are still only a.ts's.
    #[test]
    fn read_lints_reports_unchecked_paths_alongside_diagnostics() {
        let call = read_lints(json!(["a.ts", "b.ts"]));
        let message = pb::exec_client_message::Message::DiagnosticsResult(pb::DiagnosticsResult {
            result: Some(pb::diagnostics_result::Result::Success(
                pb::DiagnosticsSuccess {
                    path: "a.ts".into(),
                    diagnostics: vec![pb::Diagnostic {
                        message: "unused import".into(),
                        severity: pb::DiagnosticSeverity::Warning as i32,
                        ..Default::default()
                    }],
                    total_diagnostics: 1,
                },
            )),
        });
        let content = output(&message, &call).unwrap().content;
        assert!(content.contains("unused import"));
        assert!(
            content.contains("Not checked: b.ts"),
            "unchecked paths must be reported, got: {content}"
        );
    }
}
