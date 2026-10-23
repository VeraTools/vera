//! MCP server: stdio event loop and message dispatch.
//!
//! Reads newline-delimited JSON-RPC messages from stdin and writes
//! responses to stdout. Logs and diagnostics go to stderr.
//!
//! The server handles:
//! - `initialize` — complete the handshake and advertise capabilities
//! - `notifications/initialized` — acknowledge client readiness
//! - `tools/list` — return available tool definitions
//! - `tools/call` — dispatch to tool handlers
//! - `ping` — respond with pong
//! - Unknown methods → JSON-RPC error response

use std::io::{self, BufRead, Write};

use serde_json::Value;

use crate::protocol::{
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, InitializeResult, METHOD_NOT_FOUND,
    PARSE_ERROR, PROTOCOL_VERSION, RpcError, RpcMessage, RpcResponse, ServerCapabilities,
    ServerInfo, ToolsCapability, ToolsListResult,
};
use crate::tools;

/// Maximum number of bytes in one newline-delimited JSON-RPC request frame.
const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

enum FrameRead {
    EndOfStream,
    Complete,
    TooLarge { drain_to_newline: bool },
}

/// Read one newline-delimited frame without allowing the input buffer to grow
/// beyond `MAX_FRAME_SIZE`.
fn read_frame(reader: &mut dyn BufRead, frame: &mut Vec<u8>) -> io::Result<FrameRead> {
    frame.clear();

    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(if frame.is_empty() {
                FrameRead::EndOfStream
            } else {
                FrameRead::Complete
            });
        }

        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let frame_bytes = newline + 1;
            if frame_bytes > MAX_FRAME_SIZE.saturating_sub(frame.len()) {
                reader.consume(newline);
                return Ok(FrameRead::TooLarge {
                    drain_to_newline: true,
                });
            }
            frame.extend_from_slice(&buffer[..frame_bytes]);
            reader.consume(frame_bytes);
            return Ok(FrameRead::Complete);
        }

        if buffer.len() > MAX_FRAME_SIZE.saturating_sub(frame.len()) {
            let consumed = buffer.len();
            reader.consume(consumed);
            // There is no delimiter in the buffered input, so waiting for one
            // could hang forever on a peer that keeps the frame open.
            return Ok(FrameRead::TooLarge {
                drain_to_newline: false,
            });
        }

        frame.extend_from_slice(buffer);
        let consumed = buffer.len();
        reader.consume(consumed);
    }
}

/// Discard the remainder of an oversized frame so the next newline-delimited
/// message can still be processed.
fn drain_frame(reader: &mut dyn BufRead) -> io::Result<bool> {
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(false);
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            reader.consume(newline + 1);
            return Ok(true);
        }
        let consumed = buffer.len();
        reader.consume(consumed);
    }
}

/// Run the MCP server on the given reader/writer pair.
///
/// This is the main entry point. For production use, pass `stdin`/`stdout`.
/// For testing, pass any `BufRead`/`Write` pair.
///
/// The server processes messages one at a time (no concurrency) and never
/// panics — all errors are returned as JSON-RPC error responses.
pub fn run_server(reader: &mut dyn BufRead, writer: &mut dyn Write) {
    let mut initialized = false;
    let mut frame = Vec::with_capacity(MAX_FRAME_SIZE.min(8 * 1024));

    loop {
        match read_frame(reader, &mut frame) {
            Ok(FrameRead::EndOfStream) => {
                // EOF — client closed stdin.
                tracing::info!("Client closed connection (EOF)");
                break;
            }
            Ok(FrameRead::Complete) => {}
            Ok(FrameRead::TooLarge { drain_to_newline }) => {
                tracing::warn!("Rejected oversized JSON-RPC message");
                let err = RpcError::new(Value::Null, PARSE_ERROR, "Parse error: message too large");
                write_message(writer, &err);
                if !drain_to_newline {
                    break;
                }
                if !drain_frame(reader).unwrap_or(false) {
                    break;
                }
                continue;
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to read from stdin");
                break;
            }
        }

        let trimmed = match std::str::from_utf8(&frame) {
            Ok(line) => line.trim(),
            Err(e) => {
                tracing::warn!(error = %e, "JSON-RPC message was not valid UTF-8");
                let err = RpcError::new(Value::Null, PARSE_ERROR, "Parse error: invalid UTF-8");
                write_message(writer, &err);
                continue;
            }
        };
        if trimmed.is_empty() {
            continue;
        }

        // Parse JSON-RPC message.
        let msg: RpcMessage = match serde_json::from_str(trimmed) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse JSON-RPC message");
                let err = RpcError::new(Value::Null, PARSE_ERROR, format!("Parse error: {e}"));
                write_message(writer, &err);
                continue;
            }
        };

        // Validate jsonrpc version.
        if msg.jsonrpc.as_deref() != Some("2.0") {
            if let Some(id) = msg.id {
                let err = RpcError::new(id, INVALID_REQUEST, "Invalid JSON-RPC version");
                write_message(writer, &err);
            }
            continue;
        }

        let method = match msg.method {
            Some(ref m) => m.as_str(),
            None => {
                // No method — not a valid request or notification.
                if let Some(id) = msg.id {
                    let err = RpcError::new(id, INVALID_REQUEST, "Missing method");
                    write_message(writer, &err);
                }
                continue;
            }
        };

        // Handle notifications (no id) — fire and forget.
        if msg.id.is_none() {
            handle_notification(method, &mut initialized);
            continue;
        }

        let id = msg.id.unwrap();
        let params = msg.params.unwrap_or(Value::Null);

        // Dispatch request by method.
        match method {
            "initialize" => {
                let result = handle_initialize(&params);
                initialized = true;
                let resp = RpcResponse::new(id, serde_json::to_value(result).unwrap());
                write_message(writer, &resp);
            }
            "ping" => {
                let resp = RpcResponse::new(id, serde_json::json!({}));
                write_message(writer, &resp);
            }
            "tools/list" => {
                if !initialized {
                    let err = RpcError::new(id, INVALID_REQUEST, "Server not initialized");
                    write_message(writer, &err);
                    continue;
                }
                let defs = tools::tool_definitions();
                let result = ToolsListResult { tools: defs };
                let resp = RpcResponse::new(id, serde_json::to_value(result).unwrap());
                write_message(writer, &resp);
            }
            "tools/call" => {
                if !initialized {
                    let err = RpcError::new(id, INVALID_REQUEST, "Server not initialized");
                    write_message(writer, &err);
                    continue;
                }

                let tool_name = params.get("name").and_then(|v| v.as_str());
                let tool_args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                let call_result = match tool_name {
                    Some(name) => tools::handle_tool_call(name, &tool_args),
                    None => {
                        let err = RpcError::new(
                            id,
                            INVALID_PARAMS,
                            "Missing 'name' in tools/call params",
                        );
                        write_message(writer, &err);
                        continue;
                    }
                };

                let resp = RpcResponse::new(id, serde_json::to_value(call_result).unwrap());
                write_message(writer, &resp);
            }
            _ => {
                let err = RpcError::new(id, METHOD_NOT_FOUND, format!("Unknown method: {method}"));
                write_message(writer, &err);
            }
        }
    }
}

/// Handle the `initialize` request.
fn handle_initialize(_params: &Value) -> InitializeResult {
    let version = env!("CARGO_PKG_VERSION");
    InitializeResult {
        protocol_version: PROTOCOL_VERSION.to_string(),
        capabilities: ServerCapabilities {
            tools: ToolsCapability {
                list_changed: false,
            },
        },
        server_info: ServerInfo {
            name: "vera".to_string(),
            version: version.to_string(),
        },
    }
}

/// Handle notifications (messages without an `id`).
fn handle_notification(method: &str, initialized: &mut bool) {
    match method {
        "notifications/initialized" => {
            tracing::info!("Client sent initialized notification");
            *initialized = true;
        }
        "notifications/cancelled" => {
            tracing::debug!("Received cancellation notification (ignored)");
        }
        _ => {
            tracing::debug!(method = %method, "Received unknown notification");
        }
    }
}

/// Serialize and write a JSON-RPC message followed by a newline.
fn write_message<T: serde::Serialize>(writer: &mut dyn Write, msg: &T) {
    match serde_json::to_string(msg) {
        Ok(json) => {
            if let Err(e) = writeln!(writer, "{json}") {
                tracing::error!(error = %e, "Failed to write response");
            }
            if let Err(e) = writer.flush() {
                tracing::error!(error = %e, "Failed to flush output");
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize response");
            // Last resort: write a raw error.
            let fallback = format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{{\"code\":{INTERNAL_ERROR},\"message\":\"Serialization error\"}}}}"
            );
            let _ = writeln!(writer, "{fallback}");
            let _ = writer.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Helper: send a series of JSON-RPC messages and collect responses.
    fn run_session(messages: &[&str]) -> Vec<Value> {
        let input = messages.join("\n") + "\n";
        let mut reader = std::io::BufReader::new(Cursor::new(input));
        let mut output = Vec::new();

        run_server(&mut reader, &mut output);

        let output_str = String::from_utf8(output).unwrap();
        output_str
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn initialize_handshake() {
        let responses = run_session(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ]);

        assert_eq!(responses.len(), 1);
        let resp = &responses[0];
        assert_eq!(resp["id"], 1);
        assert!(resp["result"]["protocolVersion"].is_string());
        assert_eq!(resp["result"]["serverInfo"]["name"], "vera");
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_after_init() {
        let responses = run_session(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        ]);

        assert_eq!(responses.len(), 2);
        let tools_resp = &responses[1];
        assert_eq!(tools_resp["id"], 2);

        let tools = tools_resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 9);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search_code"));
        assert!(names.contains(&"get_stats"));
        assert!(names.contains(&"get_overview"));
        assert!(names.contains(&"regex_search"));
        assert!(names.contains(&"structural_search"));
        assert!(names.contains(&"find_references"));
        assert!(names.contains(&"explain_path"));
    }

    #[test]
    fn tools_list_before_init_rejected() {
        let responses =
            run_session(&[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#]);

        assert_eq!(responses.len(), 1);
        assert!(responses[0]["error"].is_object());
        assert!(
            responses[0]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not initialized")
        );
    }

    #[test]
    fn tools_call_missing_name() {
        let responses = run_session(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{}}"#,
        ]);

        assert_eq!(responses.len(), 2);
        let err = &responses[1];
        assert!(err["error"].is_object());
        assert!(err["error"]["message"].as_str().unwrap().contains("name"));
    }

    #[test]
    fn tools_call_unknown_tool() {
        let responses = run_session(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nonexistent","arguments":{}}}"#,
        ]);

        assert_eq!(responses.len(), 2);
        let result = &responses[1];
        assert_eq!(result["id"], 2);
        // Unknown tool returns a tool execution error (isError: true), not protocol error.
        assert_eq!(result["result"]["isError"], true);
        assert!(
            result["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Unknown tool")
        );
    }

    #[test]
    fn tools_call_search_missing_query() {
        let responses = run_session(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_code","arguments":{}}}"#,
        ]);

        assert_eq!(responses.len(), 2);
        let result = &responses[1];
        assert_eq!(result["result"]["isError"], true);
        assert!(
            result["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Missing required parameter")
        );
    }

    #[test]
    fn unknown_method_returns_error() {
        let responses = run_session(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"unknown/method","params":{}}"#,
        ]);

        assert_eq!(responses.len(), 2);
        assert!(responses[1]["error"].is_object());
        assert_eq!(responses[1]["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let responses = run_session(&[
            r#"not valid json"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        ]);

        // Should get parse error + init response.
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], PARSE_ERROR);
        assert_eq!(responses[1]["id"], 1);
    }

    #[test]
    fn oversized_frame_returns_parse_error_without_consuming_unbounded_input() {
        let input = "x".repeat(MAX_FRAME_SIZE + 1);
        let mut reader = std::io::BufReader::new(Cursor::new(input));
        let mut output = Vec::new();

        run_server(&mut reader, &mut output);

        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["error"]["code"], PARSE_ERROR);
        assert_eq!(
            responses[0]["error"]["message"],
            "Parse error: message too large"
        );
    }

    #[test]
    fn oversized_frame_is_drained_before_processing_next_request() {
        let input = format!(
            "{}\n{}\n",
            "x".repeat(MAX_FRAME_SIZE + 1),
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#
        );
        let mut reader = std::io::BufReader::new(Cursor::new(input));
        let mut output = Vec::new();

        run_server(&mut reader, &mut output);

        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], PARSE_ERROR);
        assert_eq!(responses[1]["id"], 1);
    }

    #[test]
    fn ping_responds_with_empty_result() {
        let responses = run_session(&[r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#]);

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], 1);
        assert!(responses[0]["result"].is_object());
    }

    #[test]
    fn server_survives_errors_and_continues() {
        let responses = run_session(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            // Invalid JSON.
            r#"GARBAGE"#,
            // Valid request after error.
            r#"{"jsonrpc":"2.0","id":3,"method":"ping","params":{}}"#,
            // Malformed tool call.
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_code","arguments":{}}}"#,
            // Valid tool list after error.
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/list","params":{}}"#,
        ]);

        // init response + parse error + ping + search error + tools list = 5.
        assert_eq!(responses.len(), 5);

        // init response.
        assert_eq!(responses[0]["id"], 1);
        assert!(responses[0]["result"].is_object());

        // parse error.
        assert_eq!(responses[1]["error"]["code"], PARSE_ERROR);

        // ping still works.
        assert_eq!(responses[2]["id"], 3);
        assert!(responses[2]["result"].is_object());

        // tool error (missing query).
        assert_eq!(responses[3]["result"]["isError"], true);

        // tools list still works.
        assert_eq!(responses[4]["id"], 5);
        let tools = responses[4]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 9);
    }

    #[test]
    fn empty_lines_are_ignored() {
        let responses = run_session(&[
            "",
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#,
            "",
            "",
        ]);

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], 1);
    }

    #[test]
    fn invalid_jsonrpc_version_rejected() {
        let responses = run_session(&[r#"{"jsonrpc":"1.0","id":1,"method":"ping","params":{}}"#]);

        assert_eq!(responses.len(), 1);
        assert!(responses[0]["error"].is_object());
        assert!(
            responses[0]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("JSON-RPC version")
        );
    }

    #[test]
    fn get_stats_via_mcp_returns_error_for_bad_path() {
        let responses = run_session(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_stats","arguments":{"path":"/nonexistent/path"}}}"#,
        ]);

        assert_eq!(responses.len(), 2);
        let result = &responses[1];
        assert_eq!(result["result"]["isError"], true);
    }
}
