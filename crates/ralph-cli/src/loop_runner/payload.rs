//! Builders for hook payload inputs.
//!
//! These helpers produce [`HookPayloadBuilderInput`] values that are handed
//! to the hook engine when phase events fire. Each builder fills in the
//! fields relevant to its phase (loop start, iteration start, plan created,
//! human interact, loop termination).

use ralph_core::{
    HookPayloadBuilderInput, HookPayloadContextInput, LoopContext, TerminationReason,
};

pub(super) fn build_loop_start_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    accumulated_metadata: &serde_json::Map<String, serde_json::Value>,
) -> HookPayloadBuilderInput {
    HookPayloadBuilderInput {
        loop_id: loop_id.to_string(),
        is_primary: ctx.is_primary(),
        workspace: ctx.workspace().to_path_buf(),
        repo_root: ctx.repo_root().to_path_buf(),
        pid: std::process::id(),
        iteration_current,
        iteration_max: max_iterations,
        context: HookPayloadContextInput {
            active_hat,
            metadata: accumulated_metadata.clone(),
            ..HookPayloadContextInput::default()
        },
    }
}

pub(super) fn build_iteration_start_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
    accumulated_metadata: &serde_json::Map<String, serde_json::Value>,
) -> HookPayloadBuilderInput {
    HookPayloadBuilderInput {
        loop_id: loop_id.to_string(),
        is_primary: ctx.is_primary(),
        workspace: ctx.workspace().to_path_buf(),
        repo_root: ctx.repo_root().to_path_buf(),
        pid: std::process::id(),
        iteration_current,
        iteration_max: max_iterations,
        context: HookPayloadContextInput {
            active_hat,
            selected_hat,
            selected_task,
            metadata: accumulated_metadata.clone(),
            ..HookPayloadContextInput::default()
        },
    }
}

pub(super) fn build_plan_created_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
    accumulated_metadata: &serde_json::Map<String, serde_json::Value>,
) -> HookPayloadBuilderInput {
    HookPayloadBuilderInput {
        loop_id: loop_id.to_string(),
        is_primary: ctx.is_primary(),
        workspace: ctx.workspace().to_path_buf(),
        repo_root: ctx.repo_root().to_path_buf(),
        pid: std::process::id(),
        iteration_current,
        iteration_max: max_iterations,
        context: HookPayloadContextInput {
            active_hat,
            selected_hat,
            selected_task,
            metadata: accumulated_metadata.clone(),
            ..HookPayloadContextInput::default()
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_human_interact_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
    human_interact: Option<serde_json::Value>,
    accumulated_metadata: &serde_json::Map<String, serde_json::Value>,
) -> HookPayloadBuilderInput {
    HookPayloadBuilderInput {
        loop_id: loop_id.to_string(),
        is_primary: ctx.is_primary(),
        workspace: ctx.workspace().to_path_buf(),
        repo_root: ctx.repo_root().to_path_buf(),
        pid: std::process::id(),
        iteration_current,
        iteration_max: max_iterations,
        context: HookPayloadContextInput {
            active_hat,
            selected_hat,
            selected_task,
            human_interact,
            metadata: accumulated_metadata.clone(),
            ..HookPayloadContextInput::default()
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_loop_termination_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
    termination_reason: &TerminationReason,
    accumulated_metadata: &serde_json::Map<String, serde_json::Value>,
) -> HookPayloadBuilderInput {
    HookPayloadBuilderInput {
        loop_id: loop_id.to_string(),
        is_primary: ctx.is_primary(),
        workspace: ctx.workspace().to_path_buf(),
        repo_root: ctx.repo_root().to_path_buf(),
        pid: std::process::id(),
        iteration_current,
        iteration_max: max_iterations,
        context: HookPayloadContextInput {
            active_hat,
            selected_hat,
            selected_task,
            termination_reason: Some(termination_reason.as_str().to_string()),
            metadata: accumulated_metadata.clone(),
            ..HookPayloadContextInput::default()
        },
    }
}
