//! `ralph tutorial` — interactive walkthrough of hats, hat collections, and workflow.

use crate::ColorMode;
use crate::display::colors;
use anyhow::{Context, Result};
use clap::Parser;
use std::io::{IsTerminal, Write, stdout};

/// Arguments for the tutorial subcommand.
#[derive(Parser, Debug)]
pub struct TutorialArgs {
    /// Skip prompts and print the tutorial in one pass
    #[arg(long)]
    pub no_input: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct TutorialStep {
    pub title: &'static str,
    pub body: &'static [&'static str],
}

const TUTORIAL_STEPS: &[TutorialStep] = &[
    TutorialStep {
        title: "Hats: Event-driven personas",
        body: &[
            "Hats are named personas that subscribe to events and publish new events.",
            "Each hat lists triggers (ex: task.start) and outputs (ex: build.task).",
            "Inspect hats with: ralph hats list",
            "Visualize the flow with: ralph hats graph --format ascii",
        ],
    },
    TutorialStep {
        title: "Hat collections: Swappable workflows",
        body: &[
            "Core config and hat collections are split.",
            "List built-in hat collections: ralph init --list-presets",
            "Create core config: ralph init --backend <name>",
            "Run with hats: ralph run -c ralph.yml -H builtin:code-assist",
        ],
    },
    TutorialStep {
        title: "Workflow: The loop lifecycle",
        body: &[
            "Write a prompt file (ex: PROMPT.md) or pass --prompt/--prompt-file.",
            "Run: ralph run -P PROMPT.md or ralph run -p \"...\"",
            "Ralph emits task.start, hats process events, and the loop ends on done events.",
            "Artifacts live in .ralph/agent (scratchpad, tasks, memories).",
            "Check open tasks with: ralph tools task ready",
        ],
    },
];

pub fn tutorial_steps() -> &'static [TutorialStep] {
    TUTORIAL_STEPS
}

/// Runs the interactive tutorial walkthrough.
pub fn run(color_mode: ColorMode, args: TutorialArgs) -> Result<()> {
    let use_colors = color_mode.should_use_colors();
    let interactive = !args.no_input && std::io::stdin().is_terminal();
    let steps = tutorial_steps();

    print_tutorial_intro(use_colors, interactive);

    for (index, step) in steps.iter().enumerate() {
        print_tutorial_step(index + 1, steps.len(), step, use_colors);
        if interactive && index + 1 < steps.len() {
            prompt_to_continue(use_colors)?;
        } else {
            println!();
        }
    }

    print_tutorial_outro(use_colors);
    Ok(())
}

fn print_tutorial_intro(use_colors: bool, interactive: bool) {
    if use_colors {
        println!(
            "{}{}Ralph Tutorial{}",
            colors::BOLD,
            colors::CYAN,
            colors::RESET
        );
        println!(
            "{}Interactive walkthrough of hats, hat collections, and workflow.{}",
            colors::DIM,
            colors::RESET
        );
    } else {
        println!("Ralph Tutorial");
        println!("Interactive walkthrough of hats, hat collections, and workflow.");
    }

    if !interactive {
        println!("Non-interactive mode: printing all steps.");
    }

    println!();
}

fn print_tutorial_step(index: usize, total: usize, step: &TutorialStep, use_colors: bool) {
    if use_colors {
        println!(
            "{}{}Step {}/{}: {}{}",
            colors::BOLD,
            colors::CYAN,
            index,
            total,
            step.title,
            colors::RESET
        );
    } else {
        println!("Step {}/{}: {}", index, total, step.title);
    }

    for line in step.body {
        println!("  - {}", line);
    }
}

fn prompt_to_continue(use_colors: bool) -> Result<()> {
    if use_colors {
        print!("{}Press Enter to continue...{}", colors::DIM, colors::RESET);
    } else {
        print!("Press Enter to continue...");
    }

    stdout().flush()?;
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read input")?;
    println!();
    Ok(())
}

fn print_tutorial_outro(use_colors: bool) {
    if use_colors {
        println!(
            "{}Tutorial complete. Next: ralph init --backend <name>, then ralph run -c ralph.yml -H builtin:code-assist.{}",
            colors::GREEN,
            colors::RESET
        );
    } else {
        println!(
            "Tutorial complete. Next: ralph init --backend <name>, then ralph run -c ralph.yml -H builtin:code-assist."
        );
    }
}
