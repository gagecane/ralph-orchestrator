/**
 * TaskBridge event handlers.
 *
 * Split out of TaskBridge.ts to keep the service class focused on orchestration
 * while event-to-DB handling logic lives here as a set of narrow functions.
 */

import * as fs from "fs";
import * as path from "path";
import stripAnsi from "strip-ansi";
import type { TaskRepository } from "../repositories";
import type { Event, EventBus, Subscription } from "../queue/EventBus";

import { getGitRepoRoot, extractSummaryFromOutput } from "./TaskBridge.helpers";
import { resolveLoopId } from "./TaskBridge.loopResolver";
import type {
  TaskCompletedPayload,
  TaskFailedPayload,
  TaskStartedPayload,
  TaskTimeoutPayload,
  RunnerResultPayload,
} from "./TaskBridge.types";

/**
 * Dependencies shared by every event handler.
 *
 * The bridge owns the subscription lifecycle and the `taskIdMap`, so we pass
 * them in rather than mutating globals. This keeps handlers side-effect-free
 * with respect to construction and trivially testable in isolation.
 */
export interface EventHandlerDeps {
  taskRepository: TaskRepository;
  /** Map from queuedTaskId → dbTaskId. Mutated by handlers on completion/failure. */
  taskIdMap: Map<string, string>;
  /** Working directory used to locate `.agent/` summary files. */
  defaultCwd: string;
  /**
   * Schedule background polling to resolve loop ID after task.started fires.
   * Kept as an injected hook so tests can stub it and so the polling
   * implementation (which uses setTimeout) stays colocated with the bridge.
   */
  scheduleLoopIdResolution: (dbTaskId: string) => void;
}

/**
 * Read and normalize the execution summary for a completed task.
 *
 * Preference order:
 *   1. `.agent/scratchpad.md` (internal monologue — best for UX)
 *   2. `.agent/summary.md`     (fallback)
 *   3. Tail of combined stdout/stderr (least informative)
 *
 * Any ANSI codes are stripped before the summary is returned.
 */
export function readExecutionSummary(
  defaultCwd: string,
  result: RunnerResultPayload
): string | null {
  let summary: string | null = null;

  const repoRoot = getGitRepoRoot(defaultCwd);
  const scratchpadPath = path.join(repoRoot, ".agent", "scratchpad.md");
  const summaryPath = path.join(repoRoot, ".agent", "summary.md");

  try {
    if (fs.existsSync(scratchpadPath)) {
      summary = fs.readFileSync(scratchpadPath, "utf-8");
    }
  } catch (err) {
    console.warn(`Could not read scratchpad: ${err}`);
  }

  if (!summary) {
    try {
      if (fs.existsSync(summaryPath)) {
        summary = fs.readFileSync(summaryPath, "utf-8");
      }
    } catch (err) {
      console.warn(`Could not read execution summary: ${err}`);
    }
  }

  if (!summary) {
    summary = extractSummaryFromOutput(result);
  }

  if (summary) {
    summary = stripAnsi(summary);
  }

  return summary;
}

/**
 * Handle task.started event — update DB task to 'running' and kick off
 * loop-ID polling.
 */
export function handleTaskStarted(
  deps: EventHandlerDeps,
  event: Event<TaskStartedPayload>
): void {
  const { taskId: queuedTaskId } = event.payload;
  const dbTaskId = deps.taskIdMap.get(queuedTaskId);

  if (!dbTaskId) {
    // Task was not enqueued via TaskBridge (possibly a direct queue addition)
    return;
  }

  deps.taskRepository.update(dbTaskId, {
    status: "running",
    startedAt: new Date(),
  });

  deps.scheduleLoopIdResolution(dbTaskId);
}

/**
 * Handle task.completed event — update DB task to 'closed', record the
 * execution summary, and attempt a final loop-ID fallback resolution.
 */
export function handleTaskCompleted(
  deps: EventHandlerDeps,
  event: Event<TaskCompletedPayload>
): void {
  const { taskId: queuedTaskId, durationMs, result } = event.payload;
  const dbTaskId = deps.taskIdMap.get(queuedTaskId);

  if (!dbTaskId) {
    return;
  }

  const executionSummary = readExecutionSummary(deps.defaultCwd, result);

  // Attempt loop ID resolution as a fallback (in case polling didn't find it yet)
  const dbTask = deps.taskRepository.findById(dbTaskId);
  let loopId: string | null = null;
  if (dbTask && !dbTask.loopId) {
    loopId = resolveLoopId(deps.defaultCwd, dbTask.title);
  }

  deps.taskRepository.update(dbTaskId, {
    status: "closed",
    completedAt: new Date(),
    executionSummary,
    exitCode: result.exitCode ?? 0,
    durationMs,
    ...(loopId ? { loopId } : {}),
  });

  deps.taskIdMap.delete(queuedTaskId);
}

/**
 * Handle task.failed event — update DB task to 'failed'.
 */
export function handleTaskFailed(
  deps: EventHandlerDeps,
  event: Event<TaskFailedPayload>
): void {
  const { taskId: queuedTaskId, error, durationMs } = event.payload;
  const dbTaskId = deps.taskIdMap.get(queuedTaskId);

  if (!dbTaskId) {
    return;
  }

  deps.taskRepository.update(dbTaskId, {
    status: "failed",
    completedAt: new Date(),
    errorMessage: error,
    exitCode: 1, // Non-zero indicates failure
    durationMs,
  });

  deps.taskIdMap.delete(queuedTaskId);
}

/**
 * Handle task.timeout event — update DB task to 'failed' with timeout metadata.
 */
export function handleTaskTimeout(
  deps: EventHandlerDeps,
  event: Event<TaskTimeoutPayload>
): void {
  const { taskId: queuedTaskId, timeoutMs, durationMs } = event.payload;
  const dbTaskId = deps.taskIdMap.get(queuedTaskId);

  if (!dbTaskId) {
    return;
  }

  deps.taskRepository.update(dbTaskId, {
    status: "failed",
    completedAt: new Date(),
    errorMessage: `Task timed out after ${timeoutMs}ms`,
    exitCode: 124, // Standard timeout exit code
    durationMs,
  });

  deps.taskIdMap.delete(queuedTaskId);
}

/**
 * Subscribe the lifecycle handlers to the event bus and return the resulting
 * subscriptions so the caller can dispose them during teardown.
 */
export function subscribeLifecycleEvents(
  eventBus: EventBus,
  deps: EventHandlerDeps
): Subscription[] {
  return [
    eventBus.subscribe<TaskStartedPayload>("task.started", (event) => {
      handleTaskStarted(deps, event);
    }),
    eventBus.subscribe<TaskCompletedPayload>("task.completed", (event) => {
      handleTaskCompleted(deps, event);
    }),
    eventBus.subscribe<TaskFailedPayload>("task.failed", (event) => {
      handleTaskFailed(deps, event);
    }),
    eventBus.subscribe<TaskTimeoutPayload>("task.timeout", (event) => {
      handleTaskTimeout(deps, event);
    }),
  ];
}
