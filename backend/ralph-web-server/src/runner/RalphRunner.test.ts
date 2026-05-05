/**
 * RalphRunner unit tests
 *
 * Focused unit tests for the RalphRunner subprocess manager.
 *
 * These complement RalphRunner.integration.test.ts by covering:
 * - Constructor defaults and option passthrough
 * - Initial state/accessor behavior
 * - Happy path (exit 0 → COMPLETED)
 * - Failure path (exit 1 → FAILED + error message)
 * - Event emission ordering
 * - reset() / dispose() semantics
 * - Prompt file contents for string vs PromptContent inputs
 * - AbortSignal wiring for cancellation
 *
 * All spawning tests use a dedicated `ProcessSupervisor` with a temp `runDir`
 * so the user's `~/.ralph/web/runs/` directory is never touched.
 */

import { describe, it, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { RalphRunner } from "./RalphRunner";
import { ProcessSupervisor } from "./ProcessSupervisor";
import { RunnerState, isTerminalRunnerState } from "./RunnerState";
import type { PromptContent } from "./PromptWriter";
import type { LogEntry } from "./LogStream";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

interface Harness {
  runner: RalphRunner;
  supervisor: ProcessSupervisor;
  runDir: string;
  dispose: () => void;
}

/**
 * Build a RalphRunner wired up to a temp-dir ProcessSupervisor.
 *
 * The returned `dispose()` stops the runner and removes the temp dir.
 */
function makeHarness(
  options: ConstructorParameters<typeof RalphRunner>[0] = {},
  label = "ralph-runner-test"
): Harness {
  const runDir = path.join(
    os.tmpdir(),
    `${label}-${Date.now()}-${Math.random().toString(36).slice(2)}`
  );
  const supervisor = new ProcessSupervisor({ runDir });
  const runner = new RalphRunner({
    ...options,
    supervisor,
  });

  return {
    runner,
    supervisor,
    runDir,
    dispose: () => {
      try {
        runner.dispose();
      } catch {
        // ignore double-dispose
      }
      if (fs.existsSync(runDir)) {
        fs.rmSync(runDir, { recursive: true, force: true });
      }
    },
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("RalphRunner", () => {
  // Track harnesses so each test cleans itself up even on failure.
  const harnesses: Harness[] = [];
  const track = <T extends Harness>(h: T): T => {
    harnesses.push(h);
    return h;
  };

  afterEach(() => {
    while (harnesses.length > 0) {
      harnesses.pop()!.dispose();
    }
  });

  // ---------------------------------------------------------------------
  // Constructor & initial state
  // ---------------------------------------------------------------------

  describe("constructor", () => {
    it("applies defaults for omitted options", () => {
      const { runner } = track(makeHarness());

      assert.equal(runner.state, RunnerState.IDLE);
      assert.equal(runner.pid, undefined);
      assert.equal(runner.isRunning(), false);
      assert.equal(runner.isTerminal(), false);
    });

    it("applies a custom taskId when provided", () => {
      const { runner, supervisor, runDir } = track(
        makeHarness({
          taskId: "custom-task-abc",
          command: "echo",
          baseArgs: ["hi"],
        })
      );

      // taskId is private but used for the supervisor task directory.
      // We can observe it indirectly by running and inspecting the dir.
      return runner.run("").then(() => {
        assert.ok(
          fs.existsSync(path.join(runDir, "custom-task-abc")),
          "ProcessSupervisor should use the explicit taskId for its task dir"
        );
        // reference the supervisor to keep it live until after assertions
        assert.ok(supervisor);
      });
    });

    it("generates a unique taskId when not provided", async () => {
      const h1 = track(makeHarness({ command: "echo", baseArgs: ["a"] }));
      const h2 = track(makeHarness({ command: "echo", baseArgs: ["b"] }));

      await Promise.all([h1.runner.run(""), h2.runner.run("")]);

      const listEntries = (dir: string) =>
        fs.existsSync(dir) ? fs.readdirSync(dir) : [];

      const entries1 = listEntries(h1.runDir);
      const entries2 = listEntries(h2.runDir);

      assert.equal(entries1.length, 1);
      assert.equal(entries2.length, 1);
      assert.notEqual(entries1[0], entries2[0], "Auto-generated taskIds must be unique");
    });

    it("returns empty output and zero line counts initially", () => {
      const { runner } = track(makeHarness());

      const out = runner.getOutput();
      assert.equal(out.stdout, "");
      assert.equal(out.stderr, "");
      assert.equal(out.combined, "");

      const counts = runner.getLineCount();
      assert.equal(counts.stdout, 0);
      assert.equal(counts.stderr, 0);
      assert.equal(counts.total, 0);
    });
  });

  // ---------------------------------------------------------------------
  // run() state-machine gating
  // ---------------------------------------------------------------------

  describe("run() preconditions", () => {
    it("rejects a second run() while already running", async () => {
      const { runner } = track(
        makeHarness({
          // A harmless short command so the first run completes deterministically.
          command: "echo",
          baseArgs: ["ok"],
        })
      );

      const first = runner.run("prompt");
      // Trigger second call while the first is still pending. The runner's
      // state is SPAWNING/RUNNING at this point, so it should throw.
      await assert.rejects(
        () => runner.run("again"),
        /Cannot start runner in state/
      );

      // Clean up the first call so cleanup doesn't leave it dangling.
      await first;
    });

    it("rejects run() after completion without reset()", async () => {
      const { runner } = track(makeHarness({ command: "echo", baseArgs: ["done"] }));

      const result = await runner.run("prompt");
      assert.equal(result.state, RunnerState.COMPLETED);

      await assert.rejects(() => runner.run("again"), /Cannot start runner in state/);
    });
  });

  // ---------------------------------------------------------------------
  // Happy path
  // ---------------------------------------------------------------------

  describe("successful execution", () => {
    it("transitions to COMPLETED and returns exit code 0 for exit-0 commands", async () => {
      const { runner } = track(
        makeHarness({ command: "echo", baseArgs: ["hello"] })
      );

      const result = await runner.run("prompt-text");

      assert.equal(result.state, RunnerState.COMPLETED);
      assert.equal(result.exitCode, 0);
      assert.equal(typeof result.durationMs, "number");
      assert.ok(result.durationMs >= 0, "durationMs should be non-negative");
      assert.equal(runner.isTerminal(), true);
      assert.equal(runner.isRunning(), false);
    });

    it("emits stateChange, spawned, and completed events in order", async () => {
      const { runner } = track(
        makeHarness({ command: "echo", baseArgs: ["events"] })
      );

      const stateTransitions: Array<[RunnerState, RunnerState]> = [];
      let spawnedPid: number | undefined;
      let completedSeen = false;

      runner.on("stateChange", (to, from) => {
        stateTransitions.push([from, to]);
      });
      runner.on("spawned", (pid) => {
        spawnedPid = pid;
      });
      runner.on("completed", () => {
        completedSeen = true;
      });

      await runner.run("prompt");

      // We expect at minimum: IDLE→SPAWNING, SPAWNING→RUNNING, RUNNING→COMPLETED
      assert.ok(
        stateTransitions.length >= 3,
        `Expected at least 3 transitions, got ${stateTransitions.length}: ${JSON.stringify(
          stateTransitions
        )}`
      );
      assert.deepEqual(stateTransitions[0], [RunnerState.IDLE, RunnerState.SPAWNING]);
      assert.deepEqual(stateTransitions[1], [RunnerState.SPAWNING, RunnerState.RUNNING]);
      assert.ok(
        isTerminalRunnerState(stateTransitions[stateTransitions.length - 1][1]),
        "Last transition should be to a terminal state"
      );

      assert.equal(typeof spawnedPid, "number");
      assert.ok(spawnedPid! > 0);
      assert.equal(completedSeen, true);
    });

    it("invokes the onOutput callback for captured stdout lines", async () => {
      const received: LogEntry[] = [];
      // Use `sh -c` so we can print AND sleep, keeping the process alive long
      // enough for fs.watch on stdout.log to fire before the child exits.
      // Short-lived processes like bare `echo` race the file watcher.
      const { runner } = track(
        makeHarness({
          command: "sh",
          baseArgs: ["-c", "echo onOutput-hit; sleep 0.6"],
          onOutput: (entry) => {
            received.push(entry);
          },
        })
      );

      await runner.run("prompt");

      const hit = received.find((entry) => entry.line.includes("onOutput-hit"));
      assert.ok(
        hit,
        `Expected onOutput to receive the echo'd line. Got: ${JSON.stringify(
          received.map((e) => e.line)
        )}`
      );
      assert.equal(hit!.source, "stdout");
    });

    it("writes a string prompt to prompt.txt verbatim", async () => {
      const { runner, runDir } = track(
        makeHarness({ command: "echo", baseArgs: ["ok"], taskId: "str-prompt" })
      );

      await runner.run("line one\nline two");

      const promptPath = path.join(runDir, "str-prompt", "prompt.txt");
      assert.ok(fs.existsSync(promptPath), "prompt.txt must exist");
      assert.equal(fs.readFileSync(promptPath, "utf-8"), "line one\nline two");
    });

    it("writes a PromptContent prompt as JSON to prompt.txt", async () => {
      const { runner, runDir } = track(
        makeHarness({ command: "echo", baseArgs: ["ok"], taskId: "struct-prompt" })
      );

      const content: PromptContent = {
        task: "do the thing",
        context: "some context",
        metadata: { tag: "unit-test" },
      };
      await runner.run(content);

      const promptPath = path.join(runDir, "struct-prompt", "prompt.txt");
      assert.ok(fs.existsSync(promptPath), "prompt.txt must exist");
      const stored = fs.readFileSync(promptPath, "utf-8");

      // ProcessSupervisor receives a JSON-stringified form for structured prompts.
      const parsed = JSON.parse(stored);
      assert.equal(parsed.task, "do the thing");
      assert.equal(parsed.context, "some context");
      assert.deepEqual(parsed.metadata, { tag: "unit-test" });
    });
  });

  // ---------------------------------------------------------------------
  // Failure path
  // ---------------------------------------------------------------------

  describe("failed execution", () => {
    it("transitions to FAILED with an error message when the subprocess exits non-zero", async () => {
      // `false` is guaranteed to be available on Linux and exits 1.
      const { runner } = track(makeHarness({ command: "false", baseArgs: [] }));

      const result = await runner.run("prompt");

      assert.equal(result.state, RunnerState.FAILED);
      assert.equal(result.exitCode, 1);
      assert.ok(result.error, "FAILED result should have a human-readable error message");
      assert.match(result.error!, /exit(ed|ed with code)/i);
      assert.equal(runner.isTerminal(), true);
    });

    it("emits a completed event even on failure", async () => {
      const { runner } = track(makeHarness({ command: "false", baseArgs: [] }));

      let completedResult: unknown = null;
      runner.on("completed", (result) => {
        completedResult = result;
      });

      await runner.run("prompt");

      assert.ok(
        completedResult,
        "completed event should fire regardless of success/failure"
      );
    });
  });

  // ---------------------------------------------------------------------
  // Cancellation via AbortSignal
  // ---------------------------------------------------------------------

  describe("cancellation", () => {
    it("stops a running process when the AbortSignal fires", async () => {
      // `sleep 30` keeps the child alive long enough for us to abort it.
      // If the signal path is broken, the test times out instead of passing silently.
      const { runner } = track(makeHarness({ command: "sleep", baseArgs: ["30"] }));

      const controller = new AbortController();

      const runPromise = runner.run("prompt", [], controller.signal);

      // Give the child a moment to actually spawn so stop() has something to kill.
      await new Promise((r) => setTimeout(r, 200));
      controller.abort();

      const result = await runPromise;

      assert.ok(
        result.state === RunnerState.CANCELLED || result.state === RunnerState.FAILED,
        `Expected CANCELLED (or FAILED on some exit paths), got ${result.state}`
      );
      assert.equal(runner.isTerminal(), true);
    });

    it("stops immediately when the signal is already aborted before run()", async () => {
      const { runner } = track(makeHarness({ command: "sleep", baseArgs: ["30"] }));

      const controller = new AbortController();
      controller.abort();

      const result = await runner.run("prompt", [], controller.signal);

      assert.ok(
        isTerminalRunnerState(result.state),
        `Expected a terminal state after pre-aborted signal, got ${result.state}`
      );
    });
  });

  // ---------------------------------------------------------------------
  // stop() on non-running runner
  // ---------------------------------------------------------------------

  describe("stop()", () => {
    it("is a no-op on an IDLE runner", async () => {
      const { runner } = track(makeHarness());
      // Should not throw.
      await runner.stop();
      assert.equal(runner.state, RunnerState.IDLE);
    });

    it("is a no-op on a terminal runner", async () => {
      const { runner } = track(makeHarness({ command: "echo", baseArgs: ["x"] }));
      await runner.run("prompt");
      assert.ok(runner.isTerminal());

      // Second stop() should not change state or throw.
      const before = runner.state;
      await runner.stop();
      assert.equal(runner.state, before);
    });
  });

  // ---------------------------------------------------------------------
  // reset() / dispose() semantics
  // ---------------------------------------------------------------------

  describe("reset()", () => {
    it("is a no-op on an IDLE runner", () => {
      const { runner } = track(makeHarness());
      runner.reset();
      assert.equal(runner.state, RunnerState.IDLE);
    });

    it("returns the runner to IDLE after terminal completion", async () => {
      const { runner } = track(makeHarness({ command: "echo", baseArgs: ["x"] }));

      await runner.run("prompt");
      assert.ok(runner.isTerminal());

      runner.reset();
      assert.equal(runner.state, RunnerState.IDLE);
      assert.equal(runner.isTerminal(), false);
      assert.equal(runner.isRunning(), false);
      assert.equal(runner.getOutput().combined, "");
    });

    it("allows reuse after reset()", async () => {
      const { runner } = track(makeHarness({ command: "echo", baseArgs: ["x"] }));

      const first = await runner.run("prompt-1");
      assert.equal(first.state, RunnerState.COMPLETED);

      runner.reset();

      const second = await runner.run("prompt-2");
      assert.equal(second.state, RunnerState.COMPLETED);
    });
  });

  describe("dispose()", () => {
    it("does not throw on an IDLE runner", () => {
      const { runner } = track(makeHarness());
      runner.dispose();
      // No assertion on post-state — runner is considered dead after dispose.
    });

    it("does not throw on a completed runner", async () => {
      const { runner } = track(makeHarness({ command: "echo", baseArgs: ["x"] }));
      await runner.run("prompt");
      runner.dispose();
    });

    it("detaches all event listeners", async () => {
      const { runner } = track(makeHarness({ command: "echo", baseArgs: ["x"] }));
      await runner.run("prompt");

      runner.on("stateChange", () => {
        // Would throw if invoked after dispose
      });
      assert.ok(runner.listenerCount("stateChange") >= 1);

      runner.dispose();
      assert.equal(runner.listenerCount("stateChange"), 0);
    });
  });
});
