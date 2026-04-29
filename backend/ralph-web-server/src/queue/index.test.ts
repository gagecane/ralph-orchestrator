/**
 * Queue module barrel export tests
 *
 * Guards against accidental breakage of the public surface area in
 * `src/queue/index.ts`. Every value export we rely on should resolve
 * to the same binding as the originating module.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import * as queue from "./index";
import {
  TaskState as TaskStateDirect,
  isTerminalState as isTerminalStateDirect,
  isValidTransition as isValidTransitionDirect,
  getAllowedTransitions as getAllowedTransitionsDirect,
} from "./TaskState";
import { TaskQueueService as TaskQueueServiceDirect } from "./TaskQueueService";
import { EventBus as EventBusDirect } from "./EventBus";
import { Dispatcher as DispatcherDirect } from "./Dispatcher";

describe("queue/index barrel", () => {
  it("re-exports the TaskState helpers", () => {
    assert.strictEqual(queue.TaskState, TaskStateDirect);
    assert.strictEqual(queue.isTerminalState, isTerminalStateDirect);
    assert.strictEqual(queue.isValidTransition, isValidTransitionDirect);
    assert.strictEqual(queue.getAllowedTransitions, getAllowedTransitionsDirect);
  });

  it("re-exports the queue classes", () => {
    assert.strictEqual(queue.TaskQueueService, TaskQueueServiceDirect);
    assert.strictEqual(queue.EventBus, EventBusDirect);
    assert.strictEqual(queue.Dispatcher, DispatcherDirect);
  });

  it("exports instantiable classes that produce working objects", () => {
    const q = new queue.TaskQueueService();
    const task = q.enqueue({ taskType: "barrel.test" });
    assert.equal(task.state, queue.TaskState.PENDING);

    const bus = new queue.EventBus();
    assert.equal(bus.hasSubscribers("anything"), false);
  });

  it("does not expose unexpected top-level names", () => {
    const expected = new Set([
      "TaskState",
      "isTerminalState",
      "isValidTransition",
      "getAllowedTransitions",
      "TaskQueueService",
      "EventBus",
      "Dispatcher",
    ]);

    const actual = Object.keys(queue).filter((key) => queue[key as keyof typeof queue] !== undefined);
    for (const name of actual) {
      assert.ok(expected.has(name), `unexpected export from queue/index: ${name}`);
    }
    // Make sure every expected value is actually present.
    for (const name of expected) {
      assert.ok(actual.includes(name), `missing export from queue/index: ${name}`);
    }
  });
});
