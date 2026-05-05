/**
 * RalphTaskHandler unit tests
 *
 * Focused unit tests for the factory produced by `createRalphTaskHandler`.
 *
 * The handler is the glue between Dispatcher-dispatched tasks, RalphRunner
 * subprocess execution, and the WebSocket LogBroadcaster. These tests verify:
 *
 * - The factory returns a TaskHandler-compatible function.
 * - Successful executions broadcast a terminal status and return the result.
 * - Failed executions (non-zero exit) cause the handler to throw — this is
 *   important because Dispatcher uses the thrown error to fire `task.failed`
 *   instead of `task.completed`.
 * - The handler uses `payload.dbTaskId` as the broadcastId when provided, and
 *   falls back to the queued `task.id` otherwise.
 * - Broadcasts are routed to subscribers of the broadcastId.
 *
 * We test against the real singleton LogBroadcaster by subscribing a fake
 * WebSocket client and inspecting the messages it receives. The
 * RalphTaskHandler receives a custom `command`, `baseArgs`, and
 * `ProcessSupervisor` so no real `ralph` binary is invoked.
 *
 * node --test runs each test file in its own process (default isolation),
 * so the singleton state does not leak across files.
 */

import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { WebSocket, OPEN } from "ws";
import { createRalphTaskHandler } from "./RalphTaskHandler";
import { ProcessSupervisor } from "./ProcessSupervisor";
import { RunnerState } from "./RunnerState";
import { getLogBroadcaster, resetLogBroadcaster } from "../api/LogBroadcaster";
import type { QueuedTask, TaskExecutionContext, EventBus } from "../queue";
import { TaskState } from "../queue";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

interface CapturedMessage {
  type: string;
  taskId: string;
  data: unknown;
}

/**
 * Minimal WebSocket stub that records every message send.
 *
 * LogBroadcaster only calls `send()`, `on('close', ...)`, `on('error', ...)`,
 * and reads `readyState`. We stub those and nothing else.
 */
function createFakeSocket(): { socket: WebSocket; messages: CapturedMessage[] } {
  const messages: CapturedMessage[] = [];

  const handlers: Record<string, (() => void) | undefined> = {};

  const fake = {
    readyState: OPEN,
    send: (json: string) => {
      try {
        messages.push(JSON.parse(json) as CapturedMessage);
      } catch {
        // Swallow unparseable frames; the broadcaster only sends JSON.
      }
    },
    on: (event: string, handler: () => void) => {
      handlers[event] = handler;
    },
    close: () => {
      handlers.close?.();
    },
  } as unknown as WebSocket;

  return { socket: fake, messages };
}

/**
 * Build a QueuedTask literal. The payload is forced to a
 * Record<string, unknown> because RalphTaskHandler re-casts it to
 * RalphTaskPayload internally.
 */
function makeQueuedTask(
  payload: Record<string, unknown>,
  idSuffix = "test"
): QueuedTask {
  return {
    id: `qtask-${idSuffix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    taskType: "ralph.run",
    payload,
    state: TaskState.RUNNING,
    priority: 5,
    enqueuedAt: new Date(),
    startedAt: new Date(),
    retryCount: 0,
  };
}

/**
 * Build a minimal TaskExecutionContext. The EventBus is not touched by
 * RalphTaskHandler — it just forwards to the runner — so a dummy stub works.
 */
function makeExecutionContext(): TaskExecutionContext {
  return {
    eventBus: {} as EventBus,
    correlationId: `corr-${Date.now()}`,
    signal: new AbortController().signal,
  };
}

interface HandlerHarness {
  runDir: string;
  supervisor: ProcessSupervisor;
  broadcastId: string;
  socket: WebSocket;
  messages: CapturedMessage[];
  subscribedTaskId: string;
  dispose: () => void;
}

/**
 * Set up a fake WebSocket subscribed to the broadcastId via the singleton
 * broadcaster, a ProcessSupervisor with a temp rundir, and return everything
 * the test needs.
 */
function makeHarness(broadcastId: string, label = "ralph-handler-test"): HandlerHarness {
  const runDir = path.join(
    os.tmpdir(),
    `${label}-${Date.now()}-${Math.random().toString(36).slice(2)}`
  );
  const supervisor = new ProcessSupervisor({ runDir });

  const broadcaster = getLogBroadcaster();
  const { socket, messages } = createFakeSocket();
  const clientId = broadcaster.addClient(socket);
  broadcaster.subscribe(clientId, broadcastId);

  return {
    runDir,
    supervisor,
    broadcastId,
    socket,
    messages,
    subscribedTaskId: broadcastId,
    dispose: () => {
      try {
        broadcaster.removeClient(clientId);
      } catch {
        /* ignore */
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

describe("createRalphTaskHandler", () => {
  beforeEach(() => {
    // Fresh singleton for each test so client/subscriber state doesn't leak.
    resetLogBroadcaster();
  });

  afterEach(() => {
    resetLogBroadcaster();
  });

  it("returns a TaskHandler function", () => {
    const handler = createRalphTaskHandler();
    assert.equal(typeof handler, "function");
  });

  // ---------------------------------------------------------------------
  // Success path
  // ---------------------------------------------------------------------

  describe("successful execution", () => {
    it("runs the configured command, returns the RunnerResult, and broadcasts terminal status", async () => {
      const broadcastId = "success-task-1";
      const harness = makeHarness(broadcastId, "handler-success");
      try {
        const handler = createRalphTaskHandler({
          command: "echo",
          baseArgs: ["handler-success"],
          supervisor: harness.supervisor,
        });

        const task = makeQueuedTask({ prompt: "hi", dbTaskId: broadcastId }, "success");
        const result = await handler(task, makeExecutionContext());

        assert.equal(result.state, RunnerState.COMPLETED);
        assert.equal(result.exitCode, 0);

        // The handler broadcasts "starting" first, then the final state.
        const statusMessages = harness.messages.filter((m) => m.type === "status");
        const statusValues = statusMessages
          .map((m) => (m.data as { status?: string }).status)
          .filter(Boolean) as string[];

        assert.ok(
          statusValues.includes("starting"),
          `Expected a 'starting' status broadcast. Saw: ${statusValues.join(", ")}`
        );
        assert.ok(
          statusValues.some((s) => s === RunnerState.COMPLETED),
          `Expected final COMPLETED status broadcast. Saw: ${statusValues.join(", ")}`
        );
      } finally {
        harness.dispose();
      }
    });

    it("uses payload.dbTaskId as the broadcast ID (not task.id)", async () => {
      const dbTaskId = "db-task-42";
      const harness = makeHarness(dbTaskId, "handler-dbid");
      try {
        const handler = createRalphTaskHandler({
          command: "echo",
          baseArgs: ["ok"],
          supervisor: harness.supervisor,
        });

        const task = makeQueuedTask({ prompt: "x", dbTaskId }, "dbid");
        await handler(task, makeExecutionContext());

        // All captured messages are for the subscribed broadcastId (which
        // equals dbTaskId). If the handler had used task.id, our fake
        // subscriber would see nothing.
        assert.ok(
          harness.messages.length > 0,
          "Subscriber should have received broadcasts when dbTaskId matches"
        );
        for (const msg of harness.messages) {
          assert.equal(
            msg.taskId,
            dbTaskId,
            `Every message should carry broadcastId=${dbTaskId}`
          );
        }
      } finally {
        harness.dispose();
      }
    });

    it("falls back to task.id when dbTaskId is omitted", async () => {
      const task = makeQueuedTask({ prompt: "fallback" }, "fallback");
      const harness = makeHarness(task.id, "handler-fallback");
      try {
        const handler = createRalphTaskHandler({
          command: "echo",
          baseArgs: ["ok"],
          supervisor: harness.supervisor,
        });

        await handler(task, makeExecutionContext());

        assert.ok(
          harness.messages.length > 0,
          "Subscriber using task.id should receive broadcasts when dbTaskId missing"
        );
        for (const msg of harness.messages) {
          assert.equal(
            msg.taskId,
            task.id,
            "Every message should carry the queued task.id as broadcastId"
          );
        }
      } finally {
        harness.dispose();
      }
    });

    it("honors payload.cwd over defaultCwd", async () => {
      const broadcastId = "cwd-override-task";
      const harness = makeHarness(broadcastId, "handler-cwd");
      try {
        const customCwd = fs.mkdtempSync(path.join(os.tmpdir(), "handler-cwd-override-"));
        try {
          // `pwd` prints its working directory, which ProcessSupervisor captures
          // to stdout.log. We assert on the file contents to confirm cwd wiring.
          const handler = createRalphTaskHandler({
            command: "sh",
            baseArgs: ["-c", "pwd; sleep 0.6"],
            supervisor: harness.supervisor,
            defaultCwd: os.tmpdir(),
          });

          const task = makeQueuedTask(
            { prompt: "x", dbTaskId: broadcastId, cwd: customCwd },
            "cwd"
          );
          await handler(task, makeExecutionContext());

          // taskId written by ProcessSupervisor is payload.dbTaskId.
          const stdoutLog = path.join(harness.runDir, broadcastId, "stdout.log");
          assert.ok(fs.existsSync(stdoutLog), "stdout.log must exist");
          const stdout = fs.readFileSync(stdoutLog, "utf-8");

          // On macOS the tmp dir may resolve through /private/... and pwd may
          // normalize it; a substring match is enough.
          const customLeaf = path.basename(customCwd);
          assert.ok(
            stdout.includes(customLeaf),
            `Expected pwd output to mention ${customLeaf}, got: ${stdout}`
          );
        } finally {
          if (fs.existsSync(customCwd)) {
            fs.rmSync(customCwd, { recursive: true, force: true });
          }
        }
      } finally {
        harness.dispose();
      }
    });

    it("uses defaultCwd when payload.cwd is not set", async () => {
      const broadcastId = "cwd-default-task";
      const harness = makeHarness(broadcastId, "handler-cwd-default");
      try {
        const defaultCwd = fs.mkdtempSync(path.join(os.tmpdir(), "handler-cwd-default-"));
        try {
          const handler = createRalphTaskHandler({
            command: "sh",
            baseArgs: ["-c", "pwd; sleep 0.6"],
            supervisor: harness.supervisor,
            defaultCwd,
          });

          const task = makeQueuedTask({ prompt: "x", dbTaskId: broadcastId }, "defaultCwd");
          await handler(task, makeExecutionContext());

          const stdoutLog = path.join(harness.runDir, broadcastId, "stdout.log");
          const stdout = fs.readFileSync(stdoutLog, "utf-8");
          assert.ok(
            stdout.includes(path.basename(defaultCwd)),
            `Expected pwd output to mention defaultCwd basename. Got: ${stdout}`
          );
        } finally {
          if (fs.existsSync(defaultCwd)) {
            fs.rmSync(defaultCwd, { recursive: true, force: true });
          }
        }
      } finally {
        harness.dispose();
      }
    });
  });

  // ---------------------------------------------------------------------
  // Failure path (non-zero exit)
  // ---------------------------------------------------------------------

  describe("failed execution", () => {
    it("throws when the underlying runner exits non-zero (drives Dispatcher's task.failed path)", async () => {
      const broadcastId = "fail-task-1";
      const harness = makeHarness(broadcastId, "handler-fail");
      try {
        const handler = createRalphTaskHandler({
          // `false` is guaranteed to exit 1 on Linux.
          command: "false",
          baseArgs: [],
          supervisor: harness.supervisor,
        });

        const task = makeQueuedTask({ prompt: "hi", dbTaskId: broadcastId }, "fail");

        await assert.rejects(
          () => handler(task, makeExecutionContext()),
          (err: Error) => {
            assert.ok(err instanceof Error, "handler must throw an Error on failure");
            assert.match(
              err.message,
              /Process exited with code|exited with code/i,
              "Thrown error should describe the non-zero exit"
            );
            return true;
          }
        );

        // After the failure we expect a `failed` status and an error frame
        // to have been broadcast to subscribers.
        const statusValues = harness.messages
          .filter((m) => m.type === "status")
          .map((m) => (m.data as { status?: string }).status);
        const errorMessages = harness.messages.filter((m) => m.type === "error");

        assert.ok(
          statusValues.includes("failed"),
          `Expected a 'failed' status broadcast. Saw: ${statusValues.join(", ")}`
        );
        assert.ok(
          errorMessages.length > 0,
          "Expected at least one error frame broadcast to subscribers"
        );
        const firstErrorBody = errorMessages[0].data as { error?: string };
        assert.ok(
          typeof firstErrorBody.error === "string" && firstErrorBody.error.length > 0,
          "Error frame should carry a non-empty error message"
        );
      } finally {
        harness.dispose();
      }
    });
  });
});
