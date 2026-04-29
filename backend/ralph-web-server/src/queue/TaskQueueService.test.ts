/**
 * TaskQueueService tests
 *
 * Covers enqueue/dequeue ordering (priority + FIFO), state transitions,
 * terminal filters, stats, removal, and clear semantics.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { TaskQueueService } from "./TaskQueueService";
import { TaskState } from "./TaskState";

describe("TaskQueueService", () => {
  describe("enqueue", () => {
    it("creates a task in PENDING state with defaults", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "test.task" });

      assert.equal(task.state, TaskState.PENDING);
      assert.equal(task.taskType, "test.task");
      assert.deepEqual(task.payload, {});
      assert.equal(task.priority, 5);
      assert.equal(task.retryCount, 0);
      assert.ok(task.enqueuedAt instanceof Date);
      assert.equal(task.startedAt, undefined);
      assert.equal(task.completedAt, undefined);
      assert.equal(task.error, undefined);
      assert.match(task.id, /^qtask-\d+-[0-9a-f]+$/);
    });

    it("honors explicit payload and priority", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({
        taskType: "test.task",
        payload: { foo: "bar" },
        priority: 1,
      });

      assert.deepEqual(task.payload, { foo: "bar" });
      assert.equal(task.priority, 1);
    });

    it("generates unique IDs for sequential tasks", () => {
      const queue = new TaskQueueService();
      const a = queue.enqueue({ taskType: "t" });
      const b = queue.enqueue({ taskType: "t" });
      const c = queue.enqueue({ taskType: "t" });

      assert.notEqual(a.id, b.id);
      assert.notEqual(b.id, c.id);
      assert.notEqual(a.id, c.id);
    });
  });

  describe("dequeue", () => {
    it("returns undefined task and zero remaining when empty", () => {
      const queue = new TaskQueueService();
      const result = queue.dequeue();
      assert.equal(result.task, undefined);
      assert.equal(result.remaining, 0);
    });

    it("transitions the dequeued task from PENDING to RUNNING", () => {
      const queue = new TaskQueueService();
      const enqueued = queue.enqueue({ taskType: "test" });

      const { task, remaining } = queue.dequeue();

      assert.ok(task);
      assert.equal(task!.id, enqueued.id);
      assert.equal(task!.state, TaskState.RUNNING);
      assert.ok(task!.startedAt instanceof Date);
      assert.equal(remaining, 0);
    });

    it("respects priority (lower number first)", () => {
      const queue = new TaskQueueService();
      queue.enqueue({ taskType: "low", priority: 9 });
      const high = queue.enqueue({ taskType: "high", priority: 1 });
      queue.enqueue({ taskType: "mid", priority: 5 });

      const first = queue.dequeue();
      assert.equal(first.task?.id, high.id);
      assert.equal(first.remaining, 2);
    });

    it("uses FIFO ordering within a single priority", async () => {
      const queue = new TaskQueueService();
      const first = queue.enqueue({ taskType: "a" });
      // Force a distinguishable timestamp so the comparator has a stable order.
      await new Promise((r) => setTimeout(r, 2));
      const second = queue.enqueue({ taskType: "b" });

      assert.equal(queue.dequeue().task?.id, first.id);
      assert.equal(queue.dequeue().task?.id, second.id);
    });

    it("skips tasks already in non-PENDING states", () => {
      const queue = new TaskQueueService();
      const a = queue.enqueue({ taskType: "a" });
      queue.transitionState(a.id, TaskState.CANCELLED);
      const b = queue.enqueue({ taskType: "b" });

      const result = queue.dequeue();
      assert.equal(result.task?.id, b.id);
      assert.equal(result.remaining, 0);
    });

    it("reports remaining as number still pending after dequeue", () => {
      const queue = new TaskQueueService();
      queue.enqueue({ taskType: "a" });
      queue.enqueue({ taskType: "b" });
      queue.enqueue({ taskType: "c" });

      const { remaining } = queue.dequeue();
      assert.equal(remaining, 2);
    });
  });

  describe("transitionState", () => {
    it("returns undefined for unknown task", () => {
      const queue = new TaskQueueService();
      const result = queue.transitionState("missing", TaskState.RUNNING);
      assert.equal(result, undefined);
    });

    it("throws on invalid transition", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });

      assert.throws(
        () => queue.transitionState(task.id, TaskState.COMPLETED),
        /Invalid state transition: PENDING -> COMPLETED/
      );
    });

    it("sets startedAt on transition to RUNNING", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });

      const updated = queue.transitionState(task.id, TaskState.RUNNING);
      assert.ok(updated?.startedAt instanceof Date);
      assert.equal(updated?.completedAt, undefined);
    });

    it("sets completedAt and error for FAILED", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });
      queue.transitionState(task.id, TaskState.RUNNING);

      const updated = queue.transitionState(task.id, TaskState.FAILED, "boom");
      assert.equal(updated?.state, TaskState.FAILED);
      assert.ok(updated?.completedAt instanceof Date);
      assert.equal(updated?.error, "boom");
    });

    it("sets completedAt for COMPLETED without touching error", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });
      queue.transitionState(task.id, TaskState.RUNNING);

      const updated = queue.transitionState(task.id, TaskState.COMPLETED);
      assert.equal(updated?.state, TaskState.COMPLETED);
      assert.ok(updated?.completedAt instanceof Date);
      assert.equal(updated?.error, undefined);
    });
  });

  describe("convenience transitions", () => {
    it("complete() moves RUNNING -> COMPLETED", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });
      queue.transitionState(task.id, TaskState.RUNNING);

      const done = queue.complete(task.id);
      assert.equal(done?.state, TaskState.COMPLETED);
    });

    it("fail() moves RUNNING -> FAILED and records the error", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });
      queue.transitionState(task.id, TaskState.RUNNING);

      const failed = queue.fail(task.id, "nope");
      assert.equal(failed?.state, TaskState.FAILED);
      assert.equal(failed?.error, "nope");
    });

    it("cancel() moves PENDING -> CANCELLED", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });

      const cancelled = queue.cancel(task.id);
      assert.equal(cancelled?.state, TaskState.CANCELLED);
    });

    it("cancel() moves RUNNING -> CANCELLED", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });
      queue.transitionState(task.id, TaskState.RUNNING);

      const cancelled = queue.cancel(task.id);
      assert.equal(cancelled?.state, TaskState.CANCELLED);
    });
  });

  describe("getters and stats", () => {
    it("getTask returns the stored record", () => {
      const queue = new TaskQueueService();
      const task = queue.enqueue({ taskType: "t" });
      assert.equal(queue.getTask(task.id)?.id, task.id);
      assert.equal(queue.getTask("missing"), undefined);
    });

    it("getPendingTasks / getRunningTasks / getCompletedTasks partition correctly", () => {
      const queue = new TaskQueueService();
      const a = queue.enqueue({ taskType: "a" });
      const b = queue.enqueue({ taskType: "b" });
      const c = queue.enqueue({ taskType: "c" });

      queue.transitionState(b.id, TaskState.RUNNING);

      queue.transitionState(c.id, TaskState.RUNNING);
      queue.complete(c.id);

      const pending = queue.getPendingTasks().map((t) => t.id);
      const running = queue.getRunningTasks().map((t) => t.id);
      const completed = queue.getCompletedTasks().map((t) => t.id);

      assert.deepEqual(pending, [a.id]);
      assert.deepEqual(running, [b.id]);
      assert.deepEqual(completed, [c.id]);
    });

    it("getCompletedTasks includes FAILED and CANCELLED", () => {
      const queue = new TaskQueueService();
      const a = queue.enqueue({ taskType: "a" });
      const b = queue.enqueue({ taskType: "b" });
      const c = queue.enqueue({ taskType: "c" });

      queue.transitionState(a.id, TaskState.RUNNING);
      queue.complete(a.id);

      queue.transitionState(b.id, TaskState.RUNNING);
      queue.fail(b.id, "err");

      queue.cancel(c.id);

      const terminalIds = queue.getCompletedTasks().map((t) => t.id).sort();
      assert.deepEqual(terminalIds, [a.id, b.id, c.id].sort());
    });

    it("getAllTasks returns every task", () => {
      const queue = new TaskQueueService();
      queue.enqueue({ taskType: "a" });
      queue.enqueue({ taskType: "b" });
      assert.equal(queue.getAllTasks().length, 2);
    });

    it("getStats reports counts by state", () => {
      const queue = new TaskQueueService();
      const a = queue.enqueue({ taskType: "a" });
      const b = queue.enqueue({ taskType: "b" });
      const c = queue.enqueue({ taskType: "c" });
      const d = queue.enqueue({ taskType: "d" });

      queue.transitionState(b.id, TaskState.RUNNING);

      queue.transitionState(c.id, TaskState.RUNNING);
      queue.complete(c.id);

      queue.transitionState(d.id, TaskState.RUNNING);
      queue.fail(d.id, "err");

      assert.deepEqual(queue.getStats(), {
        pending: 1,
        running: 1,
        completed: 1,
        failed: 1,
        total: 4,
      });
      // Touch `a` so lint doesn't treat it as unused.
      assert.equal(queue.getTask(a.id)?.state, TaskState.PENDING);
    });

    it("countByState counts only the requested state", () => {
      const queue = new TaskQueueService();
      queue.enqueue({ taskType: "a" });
      queue.enqueue({ taskType: "b" });
      const c = queue.enqueue({ taskType: "c" });
      queue.transitionState(c.id, TaskState.RUNNING);

      assert.equal(queue.countByState(TaskState.PENDING), 2);
      assert.equal(queue.countByState(TaskState.RUNNING), 1);
      assert.equal(queue.countByState(TaskState.COMPLETED), 0);
    });

    it("hasPending / hasRunning / isIdle reflect queue state", () => {
      const queue = new TaskQueueService();
      assert.equal(queue.isIdle(), true);
      assert.equal(queue.hasPending(), false);
      assert.equal(queue.hasRunning(), false);

      const t = queue.enqueue({ taskType: "t" });
      assert.equal(queue.hasPending(), true);
      assert.equal(queue.isIdle(), false);

      queue.transitionState(t.id, TaskState.RUNNING);
      assert.equal(queue.hasPending(), false);
      assert.equal(queue.hasRunning(), true);
      assert.equal(queue.isIdle(), false);

      queue.complete(t.id);
      assert.equal(queue.hasRunning(), false);
      assert.equal(queue.isIdle(), true);
    });
  });

  describe("remove", () => {
    it("returns false for unknown task", () => {
      const queue = new TaskQueueService();
      assert.equal(queue.remove("missing"), false);
    });

    it("throws when removing a non-terminal task", () => {
      const queue = new TaskQueueService();
      const pending = queue.enqueue({ taskType: "p" });
      const running = queue.enqueue({ taskType: "r" });
      queue.transitionState(running.id, TaskState.RUNNING);

      assert.throws(() => queue.remove(pending.id), /must be in terminal state/);
      assert.throws(() => queue.remove(running.id), /must be in terminal state/);
    });

    it("removes terminal tasks", () => {
      const queue = new TaskQueueService();
      const t = queue.enqueue({ taskType: "t" });
      queue.transitionState(t.id, TaskState.RUNNING);
      queue.complete(t.id);

      assert.equal(queue.remove(t.id), true);
      assert.equal(queue.getTask(t.id), undefined);
    });
  });

  describe("clear", () => {
    it("clears only non-running tasks by default", () => {
      const queue = new TaskQueueService();
      queue.enqueue({ taskType: "pending" });
      const r = queue.enqueue({ taskType: "running" });
      queue.transitionState(r.id, TaskState.RUNNING);
      const c = queue.enqueue({ taskType: "done" });
      queue.transitionState(c.id, TaskState.RUNNING);
      queue.complete(c.id);

      const cleared = queue.clear();
      assert.equal(cleared, 2);
      assert.equal(queue.getAllTasks().length, 1);
      assert.equal(queue.getTask(r.id)?.state, TaskState.RUNNING);
    });

    it("clears running tasks when includeRunning is true", () => {
      const queue = new TaskQueueService();
      const r = queue.enqueue({ taskType: "running" });
      queue.transitionState(r.id, TaskState.RUNNING);

      const cleared = queue.clear(true);
      assert.equal(cleared, 1);
      assert.equal(queue.getAllTasks().length, 0);
    });
  });
});
