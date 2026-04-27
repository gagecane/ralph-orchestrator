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
