/**
 * TaskBridge loop-id resolution.
 *
 * Maps a task title to the Ralph loop ID that ran it by inspecting
 * `.ralph/loops.json` (worktree loops) and `.ralph/loop.lock` (primary loop).
 * Extracted from TaskBridge.ts so the lookup logic can be unit-tested in
 * isolation and the class stays focused on DB/queue coordination.
 */

import * as fs from "fs";
import * as path from "path";
import type { TaskRepository } from "../repositories";
import { getGitRepoRoot } from "./TaskBridge.helpers";

/**
 * Resolve the loop ID for a task by matching its title against loop prompts.
 * Returns the loop ID or null if not found.
 */
export function resolveLoopId(defaultCwd: string, taskTitle: string): string | null {
  try {
    const repoRoot = getGitRepoRoot(defaultCwd);

    // Check worktree loops in loops.json
    const loopsPath = path.join(repoRoot, ".ralph", "loops.json");
    if (fs.existsSync(loopsPath)) {
      const loopsData = JSON.parse(fs.readFileSync(loopsPath, "utf-8"));
      const loops: Array<{ id: string; prompt: string; started: string }> =
        loopsData.loops ?? [];

      const matches = loops.filter((loop) => loop.prompt === taskTitle);

      if (matches.length > 0) {
        // If multiple matches, pick the most recently started one
        if (matches.length > 1) {
          matches.sort(
            (a, b) => new Date(b.started).getTime() - new Date(a.started).getTime()
          );
        }
        return matches[0].id;
      }
    }

    // Check primary loop via lock file
    const lockPath = path.join(repoRoot, ".ralph", "loop.lock");
    if (fs.existsSync(lockPath)) {
      const lockData = JSON.parse(fs.readFileSync(lockPath, "utf-8"));
      if (lockData.prompt === taskTitle) {
        return "(primary)";
      }
    }

    return null;
  } catch (err) {
    console.warn(`[TaskBridge] Failed to resolve loop ID: ${err}`);
    return null;
  }
}

/**
 * Dependencies for the loop-ID polling helper.
 */
export interface ScheduleLoopIdResolutionDeps {
  taskRepository: TaskRepository;
  defaultCwd: string;
  /**
   * Scheduler used to defer polling. Defaults to global `setTimeout` but is
   * injectable so tests can drive polling deterministically.
   */
  setTimeoutFn?: (cb: () => void, ms: number) => unknown;
}

/**
 * Poll for loop ID resolution after a task starts.
 *
 * The loop entry in `.ralph/loops.json` may appear with a slight delay after
 * the CLI process spawns, so we re-check a bounded number of times with a
 * fixed interval. As soon as a loop ID is found (and the task hasn't been
 * assigned one in the meantime), the DB is updated and polling stops.
 *
 * Extracted from `TaskBridge` so the bridge class stays focused on DB/queue
 * orchestration and so the polling state machine is independently testable.
 *
 * @param deps - Dependencies (repository, working dir, optional scheduler)
 * @param dbTaskId - The database task ID to update once the loop ID is found
 * @param options - Polling tuning (mainly for tests)
 */
export function scheduleLoopIdResolution(
  deps: ScheduleLoopIdResolutionDeps,
  dbTaskId: string,
  options: { maxAttempts?: number; intervalMs?: number } = {}
): void {
  const { taskRepository, defaultCwd } = deps;
  const schedule = deps.setTimeoutFn ?? setTimeout;
  const maxAttempts = options.maxAttempts ?? 5;
  const intervalMs = options.intervalMs ?? 2000;

  const dbTask = taskRepository.findById(dbTaskId);
  if (!dbTask) return;

  const taskTitle = dbTask.title;
  let attempts = 0;

  const poll = (): void => {
    attempts++;
    const loopId = resolveLoopId(defaultCwd, taskTitle);

    if (loopId) {
      // Verify the task still exists and doesn't already have a loopId
      const current = taskRepository.findById(dbTaskId);
      if (current && !current.loopId) {
        taskRepository.update(dbTaskId, { loopId });
      }
      return; // Done
    }

    if (attempts < maxAttempts) {
      schedule(poll, intervalMs);
    }
  };

  // Start polling after an initial delay to give the CLI time to register the loop
  schedule(poll, intervalMs);
}
