//! CLI command handlers — one submodule per subcommand group.
//!
//! Each submodule exposes its `Args` struct and a `run` entry point that the
//! top-level dispatcher in `main.rs` invokes.

pub mod clean;
pub mod completions;
pub mod emit;
pub mod events;
pub mod init;
pub mod plan;
pub mod run;
pub mod tutorial;
