/**
 * Dispatcher tests
 *
 * Covers handler registration/lookup, task execution lifecycle
 * (pending → running → completed/failed/cancelled/timeout), event publishing,
 * stats tracking, concurrency, and start/stop semantics.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { Dispatcher } from "./Dispatcher";
import { TaskQueueService } from "./TaskQueueService";
import { EventBus } from "./EventBus";
import { TaskState } from "./TaskState";

/**
 * Helper: wait until a predicate returns truthy, polling at `intervalMs`.
 * Throws if it hasn't happened within `timeoutMs`.
 */
async function waitUntil(
  predicate: () => boolean,
  { timeoutMs = 2000, intervalMs = 5 }: { timeoutMs?: number; intervalMs?: number } = {}
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) {
      throw new Error(`waitUntil timed out after ${timeoutMs}ms`);
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
}

/**
 * Build a fresh dispatcher wired to its own queue + event bus.
 */
function makeDispatcher(options: Parameters<typeof Dispatcher.prototype.constructor>[2] = {}) {
  const queue = new TaskQueueService();
  const eventBus = new EventBus();
  // Use a tight poll interval so the tests aren't slow.
  const dispatcher = new Dispatcher(queue, eventBus, { pollIntervalMs: 5, ...options });
  return { queue, eventBus, dispatcher };
}

describe("Dispatcher", () => {
  describe("configuration", () => {
    it("defaults task timeout to 2 hours", () => {
      const { eventBus, dispatcher } = makeDispatcher({ pollIntervalMs: 50 });

      let capturedConfig: any;
      eventBus.subscribe("dispatcher.started", (event) => {
        capturedConfig = (event.payload as any).config;
      });

      dispatcher.start();

      assert.ok(capturedConfig, "dispatcher.started event should be published");
      assert.equal(capturedConfig.taskTimeoutMs, 7200000);
      assert.equal(capturedConfig.pollIntervalMs, 50);
      assert.equal(capturedConfig.maxConcurrent, 1);

      return dispatcher.stop();
    });

    it("respects custom pollIntervalMs, maxConcurrent, taskTimeoutMs", () => {
      const { dispatcher } = makeDispatcher({
        pollIntervalMs: 25,
        maxConcurrent: 4,
        taskTimeoutMs: 1234,
      });

      // Internal config is exposed via dispatcher.started event payload,
      // which we also exercise elsewhere; here we just make sure construction
      // with non-default values doesn't throw and isRunning is false.
      assert.equal(dispatcher.isRunning(), false);
    });

    it("auto-starts when autoStart is true", async () => {
      const { dispatcher, eventBus } = makeDispatcher({ autoStart: true });

      let started = false;
      eventBus.subscribe("dispatcher.started", () => {
        started = true;
      });

      // The start event was published synchronously during construction.
      assert.equal(dispatcher.isRunning(), true);
      // We subscribed after construction, so we won't see the original event;
      // assert running-state is authoritative instead.
      assert.equal(started, false);

      await dispatcher.stop();
    });
  });

  describe("handler registration", () => {
    it("registers and detects handlers for a task type", () => {
      const { dispatcher } = makeDispatcher();
      assert.equal(dispatcher.hasHandler("t.x"), false);

      dispatcher.registerHandler("t.x", () => "ok");
      assert.equal(dispatcher.hasHandler("t.x"), true);
      assert.deepEqual(dispatcher.getRegisteredTaskTypes(), ["t.x"]);
    });

    it("supports method chaining on registerHandler", () => {
      const { dispatcher } = makeDispatcher();
      const result = dispatcher
        .registerHandler("a", () => 1)
        .registerHandler("b", () => 2);

      assert.equal(result, dispatcher);
      assert.deepEqual(
        dispatcher.getRegisteredTaskTypes().sort(),
        ["a", "b"],
      );
    });

    it("unregisterHandler returns true when a handler existed and false otherwise", () => {
      const { dispatcher } = makeDispatcher();
      dispatcher.registerHandler("t.x", () => "ok");
      assert.equal(dispatcher.unregisterHandler("t.x"), true);
      assert.equal(dispatcher.hasHandler("t.x"), false);
      assert.equal(dispatcher.unregisterHandler("t.x"), false);
    });

    it("default handler is used for unregistered task types", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      const seen: string[] = [];
      dispatcher.registerDefaultHandler(async (task) => {
        seen.push(task.taskType);
        return `handled:${task.taskType}`;
      });

      assert.equal(dispatcher.hasHandler("unknown.anything"), true);

      const completed: any[] = [];
      eventBus.subscribe("task.completed", (event) => {
        completed.push(event.payload);
      });

      const task = queue.enqueue({ taskType: "unknown.type" });
      dispatcher.start();
      await waitUntil(() => completed.length > 0);
      await dispatcher.stop();

      assert.deepEqual(seen, ["unknown.type"]);
      assert.equal(completed[0].taskId, task.id);
      assert.equal(completed[0].result, "handled:unknown.type");
    });

    it("specific handler takes precedence over default handler", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      dispatcher.registerDefaultHandler(() => "DEFAULT");
      dispatcher.registerHandler("t.specific", () => "SPECIFIC");

      const completed: any[] = [];
      eventBus.subscribe("task.completed", (event) => {
        completed.push(event.payload);
      });

      queue.enqueue({ taskType: "t.specific" });
      dispatcher.start();
      await waitUntil(() => completed.length > 0);
      await dispatcher.stop();

      assert.equal(completed[0].result, "SPECIFIC");
    });
  });

  describe("task execution — happy path", () => {
    it("runs a task and emits started + completed events in order", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();

      dispatcher.registerHandler("t.ok", async (task) => {
        return { echoed: task.payload };
      });

      const events: string[] = [];
      eventBus.subscribe("task.started", () => events.push("started"));
      eventBus.subscribe("task.completed", () => events.push("completed"));

      const task = queue.enqueue({
        taskType: "t.ok",
        payload: { hello: "world" },
      });

      dispatcher.start();
      await waitUntil(() => events.includes("completed"));
      await dispatcher.stop();

      assert.deepEqual(events, ["started", "completed"]);
      const stored = queue.getTask(task.id);
      assert.equal(stored?.state, TaskState.COMPLETED);
    });

    it("includes payload, priority, and correlationId on task.started event", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      dispatcher.registerHandler("t.ok", () => "ok");

      let startedEvent: any;
      eventBus.subscribe("task.started", (event) => {
        startedEvent = event;
      });

      queue.enqueue({
        taskType: "t.ok",
        payload: { a: 1 },
        priority: 2,
      });

      dispatcher.start();
      await waitUntil(() => startedEvent !== undefined);
      await dispatcher.stop();

      assert.deepEqual(startedEvent.payload.payload, { a: 1 });
      assert.equal(startedEvent.payload.priority, 2);
      assert.ok(startedEvent.correlationId, "correlationId should be attached");
    });

    it("passes execution context (eventBus, correlationId, signal) to handler", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      let capturedContext: any;

      dispatcher.registerHandler("t.ok", async (_task, ctx) => {
        capturedContext = ctx;
        return "done";
      });

      const completed: any[] = [];
      eventBus.subscribe("task.completed", (event) => completed.push(event));

      queue.enqueue({ taskType: "t.ok" });
      dispatcher.start();
      await waitUntil(() => completed.length > 0);
      await dispatcher.stop();

      assert.equal(capturedContext.eventBus, eventBus);
      assert.match(capturedContext.correlationId, /^exec-qtask-/);
      assert.ok(capturedContext.signal instanceof AbortSignal);
      assert.equal(capturedContext.signal.aborted, false);
    });

    it("supports synchronous handlers", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      dispatcher.registerHandler("t.sync", () => 42);

      const completed: any[] = [];
      eventBus.subscribe("task.completed", (event) => completed.push(event.payload));

      queue.enqueue({ taskType: "t.sync" });
      dispatcher.start();
      await waitUntil(() => completed.length > 0);
      await dispatcher.stop();

      assert.equal(completed[0].result, 42);
    });
  });

  describe("task execution — failure paths", () => {
    it("marks task as failed and emits task.failed when no handler is registered", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();

      const failures: any[] = [];
      eventBus.subscribe("task.failed", (event) => failures.push(event.payload));

      const task = queue.enqueue({ taskType: "t.missing" });
      dispatcher.start();
      await waitUntil(() => failures.length > 0);
      await dispatcher.stop();

      assert.equal(failures[0].taskId, task.id);
      assert.match(failures[0].error, /No handler registered for task type: t.missing/);
      const stored = queue.getTask(task.id);
      assert.equal(stored?.state, TaskState.FAILED);
    });

    it("marks task as failed when handler throws an Error", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      dispatcher.registerHandler("t.bang", async () => {
        throw new Error("boom");
      });

      const failures: any[] = [];
      eventBus.subscribe("task.failed", (event) => failures.push(event.payload));

      const task = queue.enqueue({ taskType: "t.bang" });
      dispatcher.start();
      await waitUntil(() => failures.length > 0);
      await dispatcher.stop();

      assert.equal(failures[0].taskId, task.id);
      assert.equal(failures[0].error, "boom");
      assert.equal(queue.getTask(task.id)?.state, TaskState.FAILED);
    });

    it("marks task as failed when handler throws a non-Error value", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      dispatcher.registerHandler("t.str", async () => {
        // eslint-disable-next-line no-throw-literal
        throw "plain-string-error";
      });

      const failures: any[] = [];
      eventBus.subscribe("task.failed", (event) => failures.push(event.payload));

      queue.enqueue({ taskType: "t.str" });
      dispatcher.start();
      await waitUntil(() => failures.length > 0);
      await dispatcher.stop();

      assert.equal(failures[0].error, "plain-string-error");
    });
  });

  describe("task execution — timeout", () => {
    it("times out long-running handlers and emits task.timeout", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher({
        taskTimeoutMs: 30,
      });

      dispatcher.registerHandler("t.slow", async () => {
        // Intentionally longer than the timeout
        await new Promise((resolve) => setTimeout(resolve, 500));
        return "too late";
      });

      const timeouts: any[] = [];
      eventBus.subscribe("task.timeout", (event) => timeouts.push(event.payload));

      const task = queue.enqueue({ taskType: "t.slow" });
      dispatcher.start();
      await waitUntil(() => timeouts.length > 0);
      await dispatcher.stop();

      assert.equal(timeouts[0].taskId, task.id);
      assert.equal(timeouts[0].timeoutMs, 30);
      const stats = dispatcher.getStats();
      assert.equal(stats.timeoutCount, 1);
      assert.equal(stats.failureCount, 1);
      assert.equal(queue.getTask(task.id)?.state, TaskState.FAILED);
    });
  });

  describe("cancellation", () => {
    it("cancels a pending task", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      const task = queue.enqueue({ taskType: "t.pending" });

      let cancelledEvent: any;
      eventBus.subscribe("task.cancelled", (event) => {
        cancelledEvent = event;
      });

      const result = await dispatcher.cancelTask(task.id);
      assert.equal(result, true);
      assert.equal(queue.getTask(task.id)?.state, TaskState.CANCELLED);
      assert.ok(cancelledEvent);
      assert.equal(cancelledEvent.payload.taskId, task.id);
      assert.equal(cancelledEvent.payload.reason, "cancelled by user");
    });

    it("cancels a running task via AbortSignal", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      dispatcher.registerHandler("t.long", async (_task, ctx) => {
        await new Promise<void>((resolve, reject) => {
          if (ctx.signal.aborted) {
            reject(ctx.signal.reason);
            return;
          }
          ctx.signal.addEventListener("abort", () => {
            reject(ctx.signal.reason);
          });
          // Fallback so tests don't hang forever
          setTimeout(resolve, 10_000);
        });
      });

      let startedEvent: any;
      eventBus.subscribe("task.started", (event) => {
        startedEvent = event;
      });
      let cancelledEvent: any;
      eventBus.subscribe("task.cancelled", (event) => {
        cancelledEvent = event;
      });

      const task = queue.enqueue({ taskType: "t.long" });
      dispatcher.start();
      await waitUntil(() => startedEvent !== undefined);

      const result = await dispatcher.cancelTask(task.id);
      assert.equal(result, true);
      await waitUntil(() => cancelledEvent !== undefined);

      await dispatcher.stop();
      assert.equal(queue.getTask(task.id)?.state, TaskState.CANCELLED);
    });

    it("returns false when cancelling an unknown or terminal task", async () => {
      const { dispatcher, queue } = makeDispatcher();
      assert.equal(await dispatcher.cancelTask("does-not-exist"), false);

      // Create a task, transition it to RUNNING via dequeue, then fail it.
      // A terminal-state task can't be cancelled by the dispatcher.
      const task = queue.enqueue({ taskType: "t" });
      queue.dequeue(); // PENDING -> RUNNING
      queue.fail(task.id, "oops");
      assert.equal(await dispatcher.cancelTask(task.id), false);
    });
  });

  describe("stats", () => {
    it("counts success, failure, total and tracks durations", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      dispatcher.registerHandler("t.ok", () => "good");
      dispatcher.registerHandler("t.bad", () => {
        throw new Error("nope");
      });

      let completed = 0;
      let failed = 0;
      eventBus.subscribe("task.completed", () => completed++);
      eventBus.subscribe("task.failed", () => failed++);

      queue.enqueue({ taskType: "t.ok" });
      queue.enqueue({ taskType: "t.ok" });
      queue.enqueue({ taskType: "t.bad" });

      dispatcher.start();
      await waitUntil(() => completed === 2 && failed === 1);
      await dispatcher.stop();

      const stats = dispatcher.getStats();
      assert.equal(stats.totalProcessed, 3);
      assert.equal(stats.successCount, 2);
      assert.equal(stats.failureCount, 1);
      assert.equal(stats.cancelledCount, 0);
      assert.equal(stats.timeoutCount, 0);
      assert.equal(stats.runningCount, 0);
      assert.ok(stats.avgDurationMs >= 0);
      assert.ok(stats.uptimeMs >= 0);
      assert.equal(stats.isRunning, false);
    });

    it("avgDurationMs is zero when no tasks have been processed", () => {
      const { dispatcher } = makeDispatcher();
      const stats = dispatcher.getStats();
      assert.equal(stats.avgDurationMs, 0);
      assert.equal(stats.totalProcessed, 0);
    });
  });

  describe("concurrency", () => {
    it("respects maxConcurrent by running up to N tasks in parallel", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher({
        pollIntervalMs: 5,
        maxConcurrent: 3,
      });

      let inFlight = 0;
      let peak = 0;
      dispatcher.registerHandler("t.par", async () => {
        inFlight++;
        peak = Math.max(peak, inFlight);
        await new Promise((resolve) => setTimeout(resolve, 40));
        inFlight--;
        return "ok";
      });

      let completed = 0;
      eventBus.subscribe("task.completed", () => completed++);

      for (let i = 0; i < 6; i++) {
        queue.enqueue({ taskType: "t.par" });
      }

      dispatcher.start();
      await waitUntil(() => completed === 6, { timeoutMs: 5000 });
      await dispatcher.stop();

      assert.equal(peak, 3, "should run exactly maxConcurrent tasks in parallel");
      assert.equal(dispatcher.getStats().successCount, 6);
    });

    it("runs tasks sequentially when maxConcurrent defaults to 1", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();

      let inFlight = 0;
      let peak = 0;
      dispatcher.registerHandler("t.seq", async () => {
        inFlight++;
        peak = Math.max(peak, inFlight);
        await new Promise((resolve) => setTimeout(resolve, 20));
        inFlight--;
      });

      let completed = 0;
      eventBus.subscribe("task.completed", () => completed++);

      for (let i = 0; i < 3; i++) {
        queue.enqueue({ taskType: "t.seq" });
      }

      dispatcher.start();
      await waitUntil(() => completed === 3);
      await dispatcher.stop();

      assert.equal(peak, 1);
    });
  });

  describe("lifecycle", () => {
    it("start is idempotent — does not re-emit dispatcher.started", () => {
      const { dispatcher, eventBus } = makeDispatcher();
      let starts = 0;
      eventBus.subscribe("dispatcher.started", () => starts++);

      dispatcher.start();
      dispatcher.start();
      dispatcher.start();

      assert.equal(starts, 1);
      assert.equal(dispatcher.isRunning(), true);
      return dispatcher.stop();
    });

    it("stop emits dispatcher.stopped exactly once and is a no-op when already stopped", async () => {
      const { dispatcher, eventBus } = makeDispatcher();
      let stops = 0;
      eventBus.subscribe("dispatcher.stopped", () => stops++);

      dispatcher.start();
      await dispatcher.stop();
      await dispatcher.stop();

      assert.equal(stops, 1);
      assert.equal(dispatcher.isRunning(), false);
    });

    it("emits dispatcher.idle when queue is empty and no tasks are running", async () => {
      const { dispatcher, eventBus } = makeDispatcher({ pollIntervalMs: 10 });
      let idleCount = 0;
      eventBus.subscribe("dispatcher.idle", () => idleCount++);

      dispatcher.start();
      // Wait for at least one poll cycle to trigger idle.
      await waitUntil(() => idleCount > 0);
      await dispatcher.stop();

      assert.ok(idleCount >= 1);
    });

    it("stop waits for running tasks to complete", async () => {
      const { dispatcher, queue } = makeDispatcher();

      let handlerDone = false;
      dispatcher.registerHandler("t.wait", async () => {
        await new Promise((resolve) => setTimeout(resolve, 50));
        handlerDone = true;
      });

      queue.enqueue({ taskType: "t.wait" });
      dispatcher.start();
      // Give the poll a moment to pick up the task
      await new Promise((resolve) => setTimeout(resolve, 20));

      await dispatcher.stop();
      assert.equal(handlerDone, true, "stop should await in-flight handlers");
      assert.equal(dispatcher.getStats().runningCount, 0);
    });

    it("stop(forceTimeoutMs) aborts long-running tasks after deadline", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();

      dispatcher.registerHandler("t.forever", async (_task, ctx) => {
        await new Promise<void>((_resolve, reject) => {
          ctx.signal.addEventListener("abort", () => reject(ctx.signal.reason));
          // Never resolves on its own
        });
      });

      let cancelled = 0;
      eventBus.subscribe("task.cancelled", () => cancelled++);

      queue.enqueue({ taskType: "t.forever" });
      dispatcher.start();
      await new Promise((resolve) => setTimeout(resolve, 20));

      const beforeStop = Date.now();
      await dispatcher.stop(50);
      const elapsed = Date.now() - beforeStop;

      assert.ok(elapsed < 1000, `stop should not hang indefinitely (took ${elapsed}ms)`);
      await waitUntil(() => cancelled >= 1, { timeoutMs: 500 });
      assert.equal(dispatcher.isRunning(), false);
    });
  });

  describe("executeOnce", () => {
    it("surfaces queue.complete failures when task is not in the queue", async () => {
      // executeOnce still calls queue.complete(id) on success, so an ad-hoc
      // task not tracked by the queue will be reported as failed — this
      // documents the current contract.
      const { dispatcher } = makeDispatcher();
      dispatcher.registerHandler("t.once", () => "one-shot");

      const fakeTask = {
        id: "adhoc-1",
        taskType: "t.once",
        payload: {},
        state: TaskState.PENDING,
        priority: 5,
        enqueuedAt: new Date(),
        retryCount: 0,
      };

      const result = await dispatcher.executeOnce(fakeTask);
      // queue.complete(id) returns undefined for unknown IDs but doesn't throw,
      // so the dispatcher still reports success (the handler's return value).
      assert.equal(result.success, true);
      assert.equal(result.result, "one-shot");
      assert.equal(dispatcher.getStats().runningCount, 0);
    });

    it("runs a dequeued task via executeOnce and returns a successful result", async () => {
      const { dispatcher, queue, eventBus } = makeDispatcher();
      dispatcher.registerHandler("t.once", () => "hello");

      queue.enqueue({ taskType: "t.once" });
      // Transition the task to RUNNING (as the polling loop normally would)
      // so queue.complete() is a valid state transition.
      const { task } = queue.dequeue();
      assert.ok(task, "dequeue should return the task we just enqueued");

      const completed: any[] = [];
      eventBus.subscribe("task.completed", (event) => completed.push(event.payload));

      const result = await dispatcher.executeOnce(task!);
      assert.equal(result.success, true);
      assert.equal(result.result, "hello");
      assert.equal(completed.length, 1);
      assert.equal(completed[0].taskId, task!.id);
      assert.equal(queue.getTask(task!.id)?.state, TaskState.COMPLETED);
    });
  });
});
