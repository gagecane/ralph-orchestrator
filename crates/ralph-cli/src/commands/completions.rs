//! `ralph completions` — generate shell completion scripts.

use crate::Cli;
use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use std::io::Write;

/// Arguments for the completions subcommand.
#[derive(Parser, Debug)]
pub struct CompletionsArgs {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

pub fn run(args: CompletionsArgs) -> Result<()> {
    use std::io::ErrorKind;

    let mut cli = Cli::command();

    // Generate into a buffer first so we can handle broken pipe errors
    // from shell consumers like `| head` without surfacing a panic.
    let mut output = Vec::new();
    generate(args.shell, &mut cli, "ralph", &mut output);

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&output).or_else(|e| {
        if e.kind() == ErrorKind::BrokenPipe {
            Ok(())
        } else {
            Err(e)
        }
    })?;

    Ok(())
}
