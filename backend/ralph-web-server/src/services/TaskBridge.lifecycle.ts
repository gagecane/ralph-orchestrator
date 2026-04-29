/**
 * TaskBridge process lifecycle helpers.
 *
 * These functions handle recovery, reconnection, and cancellation of running
 * tasks. They live here so the main `TaskBridge` class stays focused on
 * enqueueing and status queries.
 */

import type { TaskRepository } from "../repositories";
import type { ProcessSupervisor } from "../runner/ProcessSupervisor";
import type { FileOutputStreamer } from "../runner/FileOutputStreamer";
import type { EventBus } from "../queue/EventBus";

import type { EnqueueResult } from "./TaskBridge.types";

/**
 * Mark every task in `running` state as failed. Used when the server restarts
 * and we want to avoid stale "running" tasks sitting forever in the DB.
 *
 * Returns the number of tasks recovered.
 */
export function recoverStuckTasks(taskRepository: TaskRepository): number {
  const runningTasks = taskRepository.findAll("running");
  let recoveredCount = 0;

  for (const task of runningTasks) {
    taskRepository.update(task.id, {
      status: "failed",
      completedAt: new Date(),
      errorMessage: "Execution interrupted: Server restarted",
      exitCode: 1,
    });
    recoveredCount++;
  }

  return recoveredCount;
}

/**
 * Dependencies required to reconnect to running tasks after a server restart.
 */
export interface ReconnectDeps {
  taskRepository: TaskRepository;
  processSupervisor: ProcessSupervisor;
  outputStreamer: FileOutputStreamer;
  eventBus: EventBus;
}

/**
 * Reconnect to any tasks left in 'running' state after a server restart.
 *
 * For each running task:
 *   - If the process is still alive, resume output streaming.
 *   - If it has died, mark the task as failed with a descriptive error.
 *   - If state is corrupted, treat as failed (AC-5.5).
 *
 * Returns a breakdown of reconnected vs failed counts.
 */
export function reconnectRunningTasks(
  deps: ReconnectDeps
): { reconnected: number; failed: number } {
  const { taskRepository, processSupervisor, outputStreamer, eventBus } = deps;
  const runningTasks = taskRepository.findAll("running");
  let reconnectedCount = 0;
  let failedCount = 0;

  for (const task of runningTasks) {
    try {
      const handle = processSupervisor.reconnect(task.id);

      if (handle && handle.isAlive) {
        console.log(`Reconnected to task ${task.id} (PID ${handle.pid})`);

        // Resume output streaming
        outputStreamer.stream(task.id, handle.taskDir, (line, source) => {
          eventBus.publish("task.output", {
            taskId: task.id,
            line,
            source,
          });
        });

        reconnectedCount++;
      } else {
        // Process is dead, mark task as failed
        const status = processSupervisor.getStatus(task.id);
        const error = status?.error || "Process died during server restart";

        taskRepository.update(task.id, {
          status: "failed",
          completedAt: new Date(),
          errorMessage: error,
          exitCode: status?.exitCode ?? 1,
        });

        console.log(`Task ${task.id} process died, marked as failed`);
        failedCount++;
      }
    } catch (err) {
      // Handle corrupted state (AC-5.5)
      console.warn(`Failed to reconnect task ${task.id}:`, err);
      taskRepository.update(task.id, {
        status: "failed",
        completedAt: new Date(),
        errorMessage: "Corrupted task state",
        exitCode: 1,
      });
      failedCount++;
    }
  }

  return { reconnected: reconnectedCount, failed: failedCount };
}

/**
 * Dependencies required to cancel a running task.
 */
export interface CancelTaskDeps {
  taskRepository: TaskRepository;
  processSupervisor?: ProcessSupervisor;
  /** The bridge's queuedTaskId → dbTaskId map; cleaned up on cancellation. */
  taskIdMap: Map<string, string>;
}

/**
 * Cancel a running task by stopping its underlying process.
 *
 * Handles the "Process already terminated" edge case by treating it as a
 * successful cancellation (the end state — task is no longer running — matches
 * the caller's intent).
 */
export function cancelTask(
  deps: CancelTaskDeps,
  dbTaskId: string
): EnqueueResult {
  const { taskRepository, processSupervisor, taskIdMap } = deps;
  const dbTask = taskRepository.findById(dbTaskId);

  if (!dbTask) {
    return { success: false, error: "Task not found" };
  }

  if (dbTask.status !== "running") {
    return { success: false, error: "Only running tasks can be cancelled" };
  }

  if (!processSupervisor) {
    return { success: false, error: "Process supervisor not available" };
  }

  const stopResult = processSupervisor.stop(dbTaskId);

  if (!stopResult.success) {
    // Special case: process already terminated means the task ended unexpectedly.
    // Update status to reflect reality and return success.
    if (stopResult.error === "Process already terminated") {
      console.warn(
        `[TaskBridge] Task ${dbTaskId}: Process already terminated, marking as failed`
      );
      taskRepository.update(dbTaskId, {
        status: "failed",
        completedAt: new Date(),
        errorMessage: "Process terminated unexpectedly",
        exitCode: -1,
      });

      if (dbTask.queuedTaskId) {
        taskIdMap.delete(dbTask.queuedTaskId);
      }

      return { success: true };
    }

    return {
      success: false,
      error: stopResult.error || "Failed to stop process",
    };
  }

  taskRepository.update(dbTaskId, {
    status: "failed",
    completedAt: new Date(),
    errorMessage: `Task cancelled by user (signal: ${stopResult.signal})`,
    exitCode: 143, // Standard exit code for SIGTERM (128 + 15)
  });

  if (dbTask.queuedTaskId) {
    taskIdMap.delete(dbTask.queuedTaskId);
  }

  return { success: true };
}
