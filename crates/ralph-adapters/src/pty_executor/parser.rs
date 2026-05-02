//! Stream event dispatch and result-building helpers.
//!
//! This module hosts free functions that transform parsed backend stream
//! events into [`StreamHandler`] callbacks and pack the final
//! [`PtyExecutionResult`] at the end of a run.

use crate::claude_stream::{ClaudeStreamEvent, ContentBlock, UserContentBlock};
use crate::copilot_stream::{
    CopilotStreamParser, CopilotStreamState, dispatch_copilot_stream_event,
};
use crate::stream_handler::{SessionResult, StreamHandler};
use tracing::info;

use super::config::{PtyExecutionResult, TerminationType};
use super::io_events::strip_ansi;

/// Handles a single Copilot stream line, dispatching any parsed event to the
/// handler and returning a final [`SessionResult`] when the stream completes.
pub(super) fn handle_copilot_stream_line<H: StreamHandler>(
    line: &str,
    handler: &mut H,
    extracted_text: &mut String,
    copilot_state: &mut CopilotStreamState,
) -> Option<SessionResult> {
    let event = CopilotStreamParser::parse_line(line)?;
    dispatch_copilot_stream_event(event, handler, extracted_text, copilot_state)
}

/// Determines the final termination type, accounting for SIGINT exit code.
///
/// Exit code 130 indicates the process was killed by SIGINT (Ctrl+C forwarded to PTY).
pub(super) fn resolve_termination_type(
    exit_code: i32,
    default: TerminationType,
) -> TerminationType {
    if exit_code == 130 {
        info!("Child process killed by SIGINT");
        TerminationType::UserInterrupt
    } else {
        default
    }
}

/// Extracts the value associated with a CLI flag from an args vector.
///
/// Supports both split form (`--flag value`) and equals form (`--flag=value`),
/// and checks both the `long_flag` (e.g. `--model`) and `short_flag` (e.g. `-m`).
pub(super) fn extract_cli_flag_value(
    args: &[String],
    long_flag: &str,
    short_flag: &str,
) -> Option<String> {
    for (i, arg) in args.iter().enumerate() {
        if arg == long_flag || arg == short_flag {
            if let Some(value) = args.get(i + 1)
                && !value.starts_with('-')
            {
                return Some(value.clone());
            }
            continue;
        }

        if let Some(value) = arg.strip_prefix(&format!("{long_flag}="))
            && !value.is_empty()
        {
            return Some(value.to_string());
        }

        if let Some(value) = arg.strip_prefix(&format!("{short_flag}="))
            && !value.is_empty()
        {
            return Some(value.to_string());
        }
    }

    None
}

/// Dispatches a Claude stream event to the appropriate handler method.
/// Also accumulates text content into `extracted_text` for event parsing.
pub(super) fn dispatch_stream_event<H: StreamHandler>(
    event: ClaudeStreamEvent,
    handler: &mut H,
    extracted_text: &mut String,
) {
    match event {
        ClaudeStreamEvent::System { .. } => {
            // Session initialization - could log in verbose mode but not user-facing
        }
        ClaudeStreamEvent::Assistant { message, .. } => {
            for block in message.content {
                match block {
                    ContentBlock::Text { text } => {
                        handler.on_text(&text);
                        // Accumulate text for event parsing
                        extracted_text.push_str(&text);
                        extracted_text.push('\n');
                    }
                    ContentBlock::ToolUse { name, id, input } => {
                        handler.on_tool_call(&name, &id, &input)
                    }
                }
            }
        }
        ClaudeStreamEvent::User { message } => {
            for block in message.content {
                match block {
                    UserContentBlock::ToolResult {
                        tool_use_id,
                        content,
                    } => {
                        handler.on_tool_result(&tool_use_id, &content);
                    }
                }
            }
        }
        ClaudeStreamEvent::Result {
            duration_ms,
            total_cost_usd,
            num_turns,
            is_error,
        } => {
            if is_error {
                handler.on_error("Session ended with error");
            }
            handler.on_complete(&SessionResult {
                duration_ms,
                total_cost_usd,
                num_turns,
                is_error,
                ..Default::default()
            });
        }
    }
}

/// Builds a [`PtyExecutionResult`] from the accumulated output and exit status.
///
/// # Arguments
/// * `output` - Raw bytes from PTY
/// * `success` - Whether process exited successfully
/// * `exit_code` - Process exit code if available
/// * `termination` - How the process was terminated
/// * `extracted_text` - Text extracted from NDJSON stream (for Claude's stream-json)
pub(super) fn build_result(
    output: &[u8],
    success: bool,
    exit_code: Option<i32>,
    termination: TerminationType,
    extracted_text: String,
    session_result: Option<&SessionResult>,
) -> PtyExecutionResult {
    let (total_cost_usd, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens) =
        if let Some(result) = session_result {
            (
                result.total_cost_usd,
                result.input_tokens,
                result.output_tokens,
                result.cache_read_tokens,
                result.cache_write_tokens,
            )
        } else {
            (0.0, 0, 0, 0, 0)
        };

    PtyExecutionResult {
        output: String::from_utf8_lossy(output).to_string(),
        stripped_output: strip_ansi(output),
        extracted_text,
        success,
        exit_code,
        termination,
        total_cost_usd,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_stream::{AssistantMessage, ContentBlock, UserContentBlock, UserMessage};

    #[test]
    fn test_build_result_includes_extracted_text() {
        // Test that build_result properly handles extracted_text
        let output = b"raw output";
        let extracted = "extracted text with <event topic=\"test\">payload</event>";
        let result = build_result(
            output,
            true,
            Some(0),
            TerminationType::Natural,
            extracted.to_string(),
            None,
        );

        assert_eq!(result.extracted_text, extracted);
        assert!(result.stripped_output.contains("raw output"));
    }

    #[test]
    fn test_resolve_termination_type_handles_sigint_exit_code() {
        let termination = resolve_termination_type(130, TerminationType::Natural);
        assert_eq!(termination, TerminationType::UserInterrupt);

        let termination = resolve_termination_type(0, TerminationType::ForceKill);
        assert_eq!(termination, TerminationType::ForceKill);
    }

    #[test]
    fn test_extract_cli_flag_value_supports_split_and_equals_syntax() {
        let args = vec![
            "--provider".to_string(),
            "anthropic".to_string(),
            "--model=claude-sonnet-4".to_string(),
        ];

        assert_eq!(
            extract_cli_flag_value(&args, "--provider", "-p"),
            Some("anthropic".to_string())
        );
        assert_eq!(
            extract_cli_flag_value(&args, "--model", "-m"),
            Some("claude-sonnet-4".to_string())
        );
        assert_eq!(extract_cli_flag_value(&args, "--foo", "-f"), None);
    }

    use super::super::test_support::CapturingHandler;

    #[test]
    fn test_dispatch_stream_event_routes_text_and_tool_calls() {
        let mut handler = CapturingHandler::default();
        let mut extracted_text = String::new();

        let event = ClaudeStreamEvent::Assistant {
            message: AssistantMessage {
                content: vec![
                    ContentBlock::Text {
                        text: "Hello".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "Read".to_string(),
                        input: serde_json::json!({"path": "README.md"}),
                    },
                ],
            },
            usage: None,
        };

        dispatch_stream_event(event, &mut handler, &mut extracted_text);

        assert_eq!(handler.texts, vec!["Hello".to_string()]);
        assert_eq!(handler.tool_calls.len(), 1);
        assert!(extracted_text.contains("Hello"));
        assert!(extracted_text.ends_with('\n'));
    }

    #[test]
    fn test_dispatch_stream_event_routes_tool_results_and_completion() {
        let mut handler = CapturingHandler::default();
        let mut extracted_text = String::new();

        let event = ClaudeStreamEvent::User {
            message: UserMessage {
                content: vec![UserContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "done".to_string(),
                }],
            },
        };

        dispatch_stream_event(event, &mut handler, &mut extracted_text);
        assert_eq!(handler.tool_results.len(), 1);
        assert_eq!(handler.tool_results[0].0, "tool-1");
        assert_eq!(handler.tool_results[0].1, "done");

        let event = ClaudeStreamEvent::Result {
            duration_ms: 12,
            total_cost_usd: 0.01,
            num_turns: 2,
            is_error: true,
        };

        dispatch_stream_event(event, &mut handler, &mut extracted_text);
        assert_eq!(handler.errors.len(), 1);
        assert_eq!(handler.completions.len(), 1);
        assert!(handler.completions[0].is_error);
    }

    #[test]
    fn test_dispatch_stream_event_system_noop() {
        let mut handler = CapturingHandler::default();
        let mut extracted_text = String::new();

        let event = ClaudeStreamEvent::System {
            session_id: "session-1".to_string(),
            model: "claude-test".to_string(),
            tools: Vec::new(),
        };

        dispatch_stream_event(event, &mut handler, &mut extracted_text);

        assert!(handler.texts.is_empty());
        assert!(handler.tool_calls.is_empty());
        assert!(handler.tool_results.is_empty());
        assert!(handler.errors.is_empty());
        assert!(handler.completions.is_empty());
        assert!(extracted_text.is_empty());
    }

    #[test]
    fn test_build_result_populates_fields() {
        let output = b"\x1b[31mHello\x1b[0m\n";
        let extracted = "extracted text".to_string();

        let result = build_result(
            output,
            true,
            Some(0),
            TerminationType::Natural,
            extracted.clone(),
            None,
        );

        assert_eq!(result.output, String::from_utf8_lossy(output));
        assert!(result.stripped_output.contains("Hello"));
        assert!(!result.stripped_output.contains("\x1b["));
        assert_eq!(result.extracted_text, extracted);
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.termination, TerminationType::Natural);
    }
}
