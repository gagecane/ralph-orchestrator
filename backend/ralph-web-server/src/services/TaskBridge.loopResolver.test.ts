/**
 * Tests for TaskBridge loop-id polling helper.
 *
 * Covers the state machine extracted from `TaskBridge.scheduleLoopIdResolution`
 * so polling behavior is testable without spinning up a full bridge.
 */

import { test, describe } from "node:test";
import assert from "node:assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { scheduleLoopIdResolution } from "./TaskBridge.loopResolver";
import { TaskRepository } from "../repositories";
import { initializeDatabase, getDatabase } from "../db/connection";
import { tasks } from "../db/schema";

/**
 * A deterministic fake scheduler: captures every scheduled callback and
 * exposes them for the test to invoke on demand. This lets us drive polling
 * step-by-step without real timers.
 */
function makeFakeScheduler() {
  const pending: Array<{ cb: () => void; ms: number }> = [];
  const setTimeoutFn = (cb: () => void, ms: number) => {
    pending.push({ cb, ms });
    return 0;
  };
  const runNext = () => {
    const entry = pending.shift();
    if (!entry) throw new Error("No pending callback to run");
    entry.cb();
  };
  return { setTimeoutFn, pending, runNext };
}

function setupRepo() {
  initializeDatabase(getDatabase(":memory:"));
  const db = getDatabase();
  db.delete(tasks).run();
  return new TaskRepository(db);
}

function makeRepoTmpDir(): { dir: string; cleanup: () => void } {
  // Create a throwaway directory that looks like a git repo (so
  // getGitRepoRoot resolves to it) with a populated .ralph/loops.json.
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "tb-looprs-"));
  fs.mkdirSync(path.join(dir, ".git"));
  fs.mkdirSync(path.join(dir, ".ralph"));
  return {
    dir,
    cleanup: () => {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        // best effort
      }
    },
  };
}

describe("scheduleLoopIdResolution", () => {
  test("is a no-op when the task does not exist", () => {
    const repo = setupRepo();
    const { setTimeoutFn, pending } = makeFakeScheduler();

    scheduleLoopIdResolution(
      { taskRepository: repo, defaultCwd: os.tmpdir(), setTimeoutFn },
      "does-not-exist"
    );

    assert.strictEqual(pending.length, 0, "no polling should be scheduled");
  });

  test("updates the task's loopId as soon as the loop appears", () => {
    const repo = setupRepo();
    const { dir, cleanup } = makeRepoTmpDir();
    try {
      const task = repo.create({
        id: "task-loop-found",
        title: "Write a limerick",
        status: "running",
        priority: 1,
      });

      const { setTimeoutFn, pending, runNext } = makeFakeScheduler();

      // Write loops.json with a matching prompt BEFORE polling starts.
      fs.writeFileSync(
        path.join(dir, ".ralph", "loops.json"),
        JSON.stringify({
          loops: [
            { id: "loop-abc", prompt: "Write a limerick", started: "2026-01-01T00:00:00Z" },
          ],
        })
      );

      scheduleLoopIdResolution(
        { taskRepository: repo, defaultCwd: dir, setTimeoutFn },
        task.id,
        { maxAttempts: 3, intervalMs: 10 }
      );

      assert.strictEqual(pending.length, 1, "initial poll should be scheduled");
      runNext();

      const updated = repo.findById(task.id);
      assert.strictEqual(updated?.loopId, "loop-abc");
      assert.strictEqual(
        pending.length,
        0,
        "polling should stop after resolution"
      );
    } finally {
      cleanup();
    }
  });

  test("re-polls until maxAttempts when no loop is found", () => {
    const repo = setupRepo();
    const { dir, cleanup } = makeRepoTmpDir();
    try {
      const task = repo.create({
        id: "task-loop-missing",
        title: "Never appears",
        status: "running",
        priority: 1,
      });

      const { setTimeoutFn, pending, runNext } = makeFakeScheduler();

      scheduleLoopIdResolution(
        { taskRepository: repo, defaultCwd: dir, setTimeoutFn },
        task.id,
        { maxAttempts: 3, intervalMs: 10 }
      );

      // Attempt 1 → not found, re-schedule
      runNext();
      assert.strictEqual(pending.length, 1);
      // Attempt 2 → not found, re-schedule
      runNext();
      assert.strictEqual(pending.length, 1);
      // Attempt 3 → not found, no more re-schedules (hit maxAttempts)
      runNext();
      assert.strictEqual(pending.length, 0);

      const final = repo.findById(task.id);
      assert.strictEqual(final?.loopId ?? null, null);
    } finally {
      cleanup();
    }
  });

  test("does not overwrite a loopId already set on the task", () => {
    const repo = setupRepo();
    const { dir, cleanup } = makeRepoTmpDir();
    try {
      const task = repo.create({
        id: "task-prewired",
        title: "Already has loop",
        status: "running",
        priority: 1,
      });
      repo.update(task.id, { loopId: "already-set" });

      fs.writeFileSync(
        path.join(dir, ".ralph", "loops.json"),
        JSON.stringify({
          loops: [
            {
              id: "newly-resolved",
              prompt: "Already has loop",
              started: "2026-01-01T00:00:00Z",
            },
          ],
        })
      );

      const { setTimeoutFn, runNext } = makeFakeScheduler();

      scheduleLoopIdResolution(
        { taskRepository: repo, defaultCwd: dir, setTimeoutFn },
        task.id,
        { maxAttempts: 2, intervalMs: 10 }
      );
      runNext();

      const after = repo.findById(task.id);
      assert.strictEqual(after?.loopId, "already-set");
    } finally {
      cleanup();
    }
  });
});
