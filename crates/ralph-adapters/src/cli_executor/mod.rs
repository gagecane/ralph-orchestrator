//! CLI executor for running prompts through backends.
//!
//! Executes prompts via CLI tools with real-time streaming output.
//! Supports optional execution timeout with graceful SIGTERM termination.
//!
//! The module is split into focused submodules:
//! - [`executor`] — the `CliExecutor` type and its execute/capture methods
//! - [`stream`] — stream event types and the per-stream line reader task
//! - [`env`] — Ralph-specific child process environment injection
//!
//! Only [`CliExecutor`] and [`ExecutionResult`] are re-exported; the rest is
//! crate-private implementation detail.

mod env;
mod executor;
mod stream;

pub use executor::{CliExecutor, ExecutionResult};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_backend::{CliBackend, OutputFormat, PromptMode};
    use std::time::Duration;

    #[tokio::test]
    async fn test_execute_echo() {
        // Use echo as a simple test backend
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("hello world", &mut output, None, true)
            .await
            .unwrap();

        assert!(result.success);
        assert!(!result.timed_out);
        assert!(result.output.contains("hello world"));
    }

    #[tokio::test]
    async fn test_execute_stdin() {
        // Use cat to test stdin mode
        let backend = CliBackend {
            command: "cat".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let result = executor.execute_capture("stdin test").await.unwrap();

        assert!(result.success);
        assert!(result.output.contains("stdin test"));
    }

    #[tokio::test]
    async fn test_execute_failure() {
        let backend = CliBackend {
            command: "false".to_string(), // Always exits with code 1
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let result = executor.execute_capture("").await.unwrap();

        assert!(!result.success);
        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(1));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        // Use sleep to test timeout behavior
        // The sleep command ignores stdin, so we use PromptMode::Stdin
        // to avoid appending the prompt as an argument
        let backend = CliBackend {
            command: "sleep".to_string(),
            args: vec!["10".to_string()],   // Sleep for 10 seconds
            prompt_mode: PromptMode::Stdin, // Use stdin mode so prompt doesn't interfere
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);

        // Execute with a 100ms timeout - should trigger timeout
        let timeout = Some(Duration::from_millis(100));
        let result = executor
            .execute_capture_with_timeout("", timeout)
            .await
            .unwrap();

        assert!(result.timed_out, "Expected execution to time out");
        assert!(
            !result.success,
            "Timed out execution should not be successful"
        );
    }

    #[tokio::test]
    async fn test_execute_timeout_resets_on_output_activity() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let timeout = Some(Duration::from_millis(300));
        let result = executor
            .execute_capture_with_timeout(
                "printf 'start\\n'; sleep 0.2; printf 'middle\\n'; sleep 0.2; printf 'done\\n'",
                timeout,
            )
            .await
            .unwrap();

        assert!(
            !result.timed_out,
            "Periodic output should reset the inactivity timeout"
        );
        assert!(result.success, "Periodic-output command should succeed");
        assert!(result.output.contains("start"));
        assert!(result.output.contains("middle"));
        assert!(result.output.contains("done"));
    }

    #[tokio::test]
    async fn test_execute_streams_output_before_inactivity_timeout() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "printf 'hello\\n'; sleep 10".to_string()],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();
        let result = executor
            .execute("", &mut output, Some(Duration::from_millis(200)), false)
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected inactivity timeout after output stops"
        );
        assert_eq!(String::from_utf8(output).unwrap(), "hello\n");
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_timeout_force_kills_processes_that_ignore_sigterm() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "trap '' TERM; while :; do sleep 1; done".to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let started = std::time::Instant::now();
        let result = executor
            .execute_capture_with_timeout("", Some(Duration::from_millis(100)))
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected ignored-SIGTERM command to time out"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Executor should force-kill ignored-SIGTERM processes instead of hanging"
        );
    }

    #[tokio::test]
    async fn test_execute_uses_short_post_event_grace_timeout() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'Event emitted: task.done\\n'; sleep 30".to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let started = std::time::Instant::now();
        let result = executor
            .execute_capture_with_timeout("", Some(Duration::from_secs(30)))
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected lingering post-event process to be terminated"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Event-emitting backends should use the short post-event grace timeout instead of the full inactivity timeout"
        );
        assert!(result.output.contains("Event emitted: task.done"));
    }

    #[tokio::test]
    async fn test_execute_post_event_deadline_does_not_reset_on_output_activity() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'Event emitted: task.done\\n'; while :; do printf 'heartbeat\\n'; sleep 1; done"
                    .to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let started = std::time::Instant::now();
        let result = executor
            .execute_capture_with_timeout("", Some(Duration::from_secs(30)))
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected noisy post-event process to be terminated"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Event-emitting backends should respect the fixed post-event grace deadline even if they keep producing output"
        );
        assert!(result.output.contains("Event emitted: task.done"));
        assert!(result.output.contains("heartbeat"));
    }

    #[tokio::test]
    async fn test_execute_no_timeout_when_fast() {
        // Use echo which completes immediately
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);

        // Execute with a generous timeout - should complete before timeout
        let timeout = Some(Duration::from_secs(10));
        let result = executor
            .execute_capture_with_timeout("fast", timeout)
            .await
            .unwrap();

        assert!(!result.timed_out, "Fast command should not time out");
        assert!(result.success);
        assert!(result.output.contains("fast"));
    }

    #[tokio::test]
    async fn test_execute_copilot_stream_writes_extracted_text() {
        let backend = CliBackend {
            command: "printf".to_string(),
            args: vec![
                "%s\n%s\n".to_string(),
                r#"{"type":"assistant.turn_start","data":{"turnId":"0"}}"#.to_string(),
                r#"{"type":"assistant.message","data":{"content":"hello from copilot"}}"#
                    .to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::CopilotStreamJson,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("ignored", &mut output, None, false)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"assistant.message\""));
        assert_eq!(String::from_utf8(output).unwrap(), "hello from copilot\n");
    }
}
