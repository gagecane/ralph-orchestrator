# Ralph Hat Collections

This directory contains the canonical built-in hat collections Ralph still ships and supports.

Built-ins are embedded into the CLI from these files and exposed through `ralph init --list-presets`.

## Layout

```
presets/
├── *.yml          # Full hat collections (public builtins + internal helpers)
└── minimal/       # Small per-backend and single-hat starter configs
```

The two groups serve different purposes and are NOT duplicates of each other. See [Top-level vs `minimal/`](#top-level-vs-minimal) below for details.

## Supported Builtins

These are the public hat collections exposed by `ralph init --list-presets` and embedded in the CLI binary:

| Collection | Source | Best for |
|---|---|---|
| `autoresearch` | `presets/autoresearch.yml` | Autonomous experiment loop for any measurable improvement |
| `code-assist` | `presets/code-assist.yml` | Default implementation workflow |
| `debug` | `presets/debug.yml` | Investigation and fix verification |
| `research` | `presets/research.yml` | Read-only exploration and synthesis |
| `review` | `presets/review.yml` | Adversarial code review |
| `pdd-to-code-assist` | `presets/pdd-to-code-assist.yml` | Advanced end-to-end idea-to-code workflow |

## Internal Presets

These remain loadable for Ralph internals or testing, but are intentionally hidden from normal builtin listings:

- `hatless-baseline` (`presets/hatless-baseline.yml`) — control preset with no hats; validates the core loop.
- `merge-loop` (`crates/ralph-cli/presets/merge-loop.yml`) — used by parallel-loop merge workflows. Lives under the crate directory because it is embedded via `include_str!` and has no canonical copy in this directory.
- `wave-review` (`presets/wave-review.yml`) — example scatter-gather code review using `concurrency`/`aggregate`. Loaded by path, not by builtin name.

## Product Positioning

- `code-assist` is the recommended default for implementation work.
- `pdd-to-code-assist` is intentionally kept as an advanced, fun example. It is slower, more expensive, and less predictable than `code-assist`.
- Other historical presets are now treated as documentation examples instead of supported builtins.

## Quick Start

```bash
ralph init --backend claude
ralph init --list-presets

ralph run -c ralph.yml -H builtin:autoresearch -p "Improve test coverage in src/core/"
ralph run -c ralph.yml -H builtin:code-assist -p "Add OAuth login"
ralph run -c ralph.yml -H builtin:debug -p "Investigate intermittent timeout"
ralph run -c ralph.yml -H builtin:research -p "Map auth architecture"
ralph run -c ralph.yml -H builtin:review -p "Review changes in src/api/"
ralph run -c ralph.yml -H builtin:pdd-to-code-assist -p "Build a new import pipeline"
```

## Top-level vs `minimal/`

The top-level preset YAMLs and the files under `presets/minimal/` look similar at a glance, but they are different artifacts with different purposes.

### Top-level `presets/*.yml`

**Full hat collections.** Each one is a complete multi-hat workflow (planner, builder, critic, finalizer, etc.) with a `completion_promise`, `required_events`, and detailed `instructions` on every hat. These are the canonical public builtins listed in the "Supported Builtins" table above.

- Listed by `ralph init --list-presets`.
- Embedded into the `ralph` binary via `include_str!` in `crates/ralph-cli/src/presets.rs`.
- Mirrored into `crates/ralph-cli/presets/*.yml` by `./scripts/sync-embedded-files.sh` so the crate can be published to crates.io.

### `presets/minimal/*.yml`

**Small, single-purpose starter configs.** None of these are exposed as builtins — they are loaded by explicit path (`ralph run -c presets/minimal/<name>.yml`). They fall into two groups:

1. **Per-backend minimal examples** — ready-to-use configs that show the canonical settings for a given backend:
   - `amp.yml`, `claude.yml`, `codex.yml`, `gemini.yml`, `kiro.yml`, `opencode.yml`, `roo.yml`.
   - These illustrate the recommended `cli.backend`, `prompt_mode`, `pty_mode`, and `idle_timeout_secs` for each supported CLI.
   - **Note:** `ralph init --backend <name>` does **not** load these files; it generates a short inline template (see `crates/ralph-cli/src/init.rs::generate_template`). The minimal/ backend files are examples you can copy into `ralph.yml` and extend by hand.

2. **Small single-hat and harness configs** — minimal YAMLs used for testing and tooling:
   - `builder.yml` — single-hat builder, no planning phase. Best for small, well-defined tasks.
   - `code-assist.yml` — single-hat TDD builder that defers to `.sops/code-assist.sop.md`. **Not** the same preset as the top-level `presets/code-assist.yml` (which is the full 500+ line multi-hat TDD workflow).
   - `smoke.yml` — fast/cheap config used by Ralph's smoke tests.
   - `test.yml` — minimal config used by integration tests.
   - `preset-evaluator.yml` — meta-evaluator that tests other presets with Kiro.

### Name overlap: `code-assist.yml`

The only filename that appears in both places is `code-assist.yml`. They are intentionally different files:

| File | Size | What it is |
|---|---|---|
| `presets/code-assist.yml` | ~500 lines | Full multi-hat TDD workflow: Planner → Builder → Critic → Finalizer. The public builtin named `code-assist`. |
| `presets/minimal/code-assist.yml` | ~40 lines | Single-hat builder that defers all workflow details to the code-assist SOP. Used as a lightweight starter template. |

When `ralph init --list-presets` or `ralph run -H builtin:code-assist` picks up `code-assist`, it uses the **top-level** file. The `minimal/` variant is only loaded when you pass its explicit path.

## Examples Instead of Builtins

Example workflow patterns now live in the docs rather than as shipped preset files. See:

- `docs/examples/`
- `presets/COLLECTION.md`

## Source Of Truth

- Canonical builtins: `presets/*.yml`
- Builtin index: `presets/index.json`
- Embedded CLI mirror: `crates/ralph-cli/presets/*.yml` (including `crates/ralph-cli/presets/minimal/*.yml`)
- Sync script: `./scripts/sync-embedded-files.sh`

When adding a new preset, update `scripts/sync-embedded-files.sh` and, if it should be a public builtin, register it in `crates/ralph-cli/src/presets.rs` with `public: true`.
