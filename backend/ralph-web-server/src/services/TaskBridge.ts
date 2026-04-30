/**
 * TaskBridge
 *
 * Bridges the database task system (TaskRepository) with the execution queue (TaskQueueService).
 * This service:
 * 1. Enqueues database tasks into the execution queue
 * 2. Subscribes to EventBus events for execution lifecycle
 * 3. Syncs execution status back to the database
 *
 * Architecture:
 * ```
 * UI → tRPC task.run → TaskBridge.enqueueTask() → TaskQueueService
 *                                                       ↓
 *                                                Dispatcher polls
 *                                                       ↓
 *                                             RalphTaskHandler executes
 *                                                       ↓
 *                                             EventBus publishes events
 *                                                       ↓
 *               TaskBridge subscribes → updates DB → UI refreshes
 * ```
 *
 * Supporting modules:
 * - `TaskBridge.types.ts`         — event payload and public result types
 * - `TaskBridge.helpers.ts`       — `getGitRepoRoot`, `extractSummaryFromOutput`
 * - `TaskBridge.configResolver.ts` — preset/config → `-c <path>` CLI args
 * - `TaskBridge.loopResolver.ts`  — loop ID lookup + post-start polling helper
 */

import * as fs from "fs";
import * as path from "path";
import stripAnsi from "strip-ansi";
import { TaskRepository } from "../repositories";
import { ProcessSupervisor } from "../runner/ProcessSupervisor";
import { FileOutputStreamer } from "../runner/FileOutputStreamer";
import { CollectionService } from "./CollectionService";
import { ConfigMerger } from "./ConfigMerger";
import { TaskQueueService } from "../queue/TaskQueueService";
import { EventBus, Event, Subscription } from "../queue/EventBus";
import { Task } from "../db/schema";

import { getGitRepoRoot, extractSummaryFromOutput } from "./TaskBridge.helpers";
import { resolveConfigArgs } from "./TaskBridge.configResolver";
import {
  resolveLoopId,
  scheduleLoopIdResolution,
} from "./TaskBridge.loopResolver";
import type {
  EnqueueAllResult,
  EnqueueResult,
  ExecutionStatus,
  TaskCompletedPayload,
  TaskFailedPayload,
  TaskStartedPayload,
  TaskTimeoutPayload,
} from "./TaskBridge.types";

// Re-export public types so existing `from "./TaskBridge"` imports keep working.
export type {
  EnqueueAllResult,
  EnqueueResult,
  ExecutionStatus,
} from "./TaskBridge.types";

/**
 * TaskBridge configuration options
 */
export interface TaskBridgeOptions {
  /** Default working directory for task execution */
  defaultCwd: string;
  /** Task type to use for queue (default: 'ralph.run') */
  taskType?: string;
  /** Process supervisor for reconnection (optional) */
  processSupervisor?: ProcessSupervisor;
  /** Output streamer for reconnection (optional) */
  outputStreamer?: FileOutputStreamer;
  /** Default config path to use when no preset is specified */
  defaultConfigPath?: string;
  /** Collection service for exporting collection presets to YAML */
  collectionService?: CollectionService;
  /** Config merger for combining base config with preset hats */
  configMerger?: ConfigMerger;
}

/**
 * TaskBridge
 *
 * Coordinates between the database task system and the execution queue.
 */
export class TaskBridge {
  private readonly taskRepository: TaskRepository;
  private readonly taskQueue: TaskQueueService;
  private readonly eventBus: EventBus;
  private readonly defaultCwd: string;
  private readonly taskType: string;
  private readonly processSupervisor?: ProcessSupervisor;
  private readonly outputStreamer?: FileOutputStreamer;
  private readonly defaultConfigPath?: string;
  private readonly collectionService?: CollectionService;
  private readonly configMerger?: ConfigMerger;

  /** Map from queuedTaskId to dbTaskId for correlation */
  private readonly taskIdMap: Map<string, string> = new Map();

  /** Event subscriptions for cleanup */
  private readonly subscriptions: Subscription[] = [];

  constructor(
    taskRepository: TaskRepository,
    taskQueue: TaskQueueService,
    eventBus: EventBus,
    options: TaskBridgeOptions
  ) {
    this.taskRepository = taskRepository;
    this.taskQueue = taskQueue;
    this.eventBus = eventBus;
    this.defaultCwd = options.defaultCwd;
    this.taskType = options.taskType ?? "ralph.run";
    this.processSupervisor = options.processSupervisor;
    this.outputStreamer = options.outputStreamer;
    this.defaultConfigPath = options.defaultConfigPath;
    this.collectionService = options.collectionService;
    this.configMerger = options.configMerger;

    // Subscribe to execution lifecycle events
    this.subscribeToEvents();
  }

  /**
   * Subscribe to EventBus events for task lifecycle updates
   */
  private subscribeToEvents(): void {
    this.subscriptions.push(
      this.eventBus.subscribe<TaskStartedPayload>("task.started", (event) => {
        this.handleTaskStarted(event);
      })
    );
    this.subscriptions.push(
      this.eventBus.subscribe<TaskCompletedPayload>("task.completed", (event) => {
        this.handleTaskCompleted(event);
      })
    );
    this.subscriptions.push(
      this.eventBus.subscribe<TaskFailedPayload>("task.failed", (event) => {
        this.handleTaskFailed(event);
      })
    );
    this.subscriptions.push(
      this.eventBus.subscribe<TaskTimeoutPayload>("task.timeout", (event) => {
        this.handleTaskTimeout(event);
      })
    );
  }

  /**
   * Handle task.started event - update DB task to 'running'
   */
  private handleTaskStarted(event: Event<TaskStartedPayload>): void {
    const { taskId: queuedTaskId } = event.payload;
    const dbTaskId = this.taskIdMap.get(queuedTaskId);

    if (!dbTaskId) {
      // Task was not enqueued via TaskBridge (possibly a direct queue addition)
      return;
    }

    this.taskRepository.update(dbTaskId, {
      status: "running",
      startedAt: new Date(),
    });

    // Start polling for loop ID in the background
    this.scheduleLoopIdResolution(dbTaskId);
  }

  /**
   * Handle task.completed event - update DB task to 'closed'
   * Reads execution summary from .agent/scratchpad.md (preferred) or .agent/summary.md (fallback).
   * The scratchpad contains the internal monologue which is more informative for UX.
   */
  private handleTaskCompleted(event: Event<TaskCompletedPayload>): void {
    const { taskId: queuedTaskId, durationMs, result } = event.payload;
    const dbTaskId = this.taskIdMap.get(queuedTaskId);

    if (!dbTaskId) {
      return;
    }

    let executionSummary: string | null = null;

    // Try to read from .agent/scratchpad.md first (internal monologue - better UX)
    const repoRoot = getGitRepoRoot(this.defaultCwd);
    const scratchpadPath = path.join(repoRoot, ".agent", "scratchpad.md");
    const summaryPath = path.join(repoRoot, ".agent", "summary.md");

    try {
      if (fs.existsSync(scratchpadPath)) {
        executionSummary = fs.readFileSync(scratchpadPath, "utf-8");
      }
    } catch (err) {
      console.warn(`Could not read scratchpad: ${err}`);
    }

    // Fallback to .agent/summary.md if no scratchpad
    if (!executionSummary) {
      try {
        if (fs.existsSync(summaryPath)) {
          executionSummary = fs.readFileSync(summaryPath, "utf-8");
        }
      } catch (err) {
        console.warn(`Could not read execution summary: ${err}`);
      }
    }

    // Final fallback: extract from stdout (least informative)
    if (!executionSummary) {
      executionSummary = extractSummaryFromOutput(result);
    }

    // Strip ANSI codes if summary exists
    if (executionSummary) {
      executionSummary = stripAnsi(executionSummary);
    }

    // Attempt loop ID resolution as a fallback (in case polling didn't find it yet)
    const dbTask = this.taskRepository.findById(dbTaskId);
    let loopId: string | null = null;
    if (dbTask && !dbTask.loopId) {
      loopId = resolveLoopId(this.defaultCwd, dbTask.title);
    }

    this.taskRepository.update(dbTaskId, {
      status: "closed",
      completedAt: new Date(),
      executionSummary,
      exitCode: result.exitCode ?? 0,
      durationMs,
      ...(loopId ? { loopId } : {}),
    });

    // Clean up the mapping
    this.taskIdMap.delete(queuedTaskId);
  }

  /**
   * Handle task.failed event - update DB task to 'failed'
   */
  private handleTaskFailed(event: Event<TaskFailedPayload>): void {
    const { taskId: queuedTaskId, error, durationMs } = event.payload;
    const dbTaskId = this.taskIdMap.get(queuedTaskId);

    if (!dbTaskId) {
      return;
    }

    this.taskRepository.update(dbTaskId, {
      status: "failed",
      completedAt: new Date(),
      errorMessage: error,
      exitCode: 1, // Non-zero indicates failure
      durationMs,
    });

    this.taskIdMap.delete(queuedTaskId);
  }

  /**
   * Handle task.timeout event - update DB task to 'failed' with timeout message
   */
  private handleTaskTimeout(event: Event<TaskTimeoutPayload>): void {
    const { taskId: queuedTaskId, timeoutMs, durationMs } = event.payload;
    const dbTaskId = this.taskIdMap.get(queuedTaskId);

    if (!dbTaskId) {
      return;
    }

    this.taskRepository.update(dbTaskId, {
      status: "failed",
      completedAt: new Date(),
      errorMessage: `Task timed out after ${timeoutMs}ms`,
      exitCode: 124, // Standard timeout exit code
      durationMs,
    });

    this.taskIdMap.delete(queuedTaskId);
  }

  /**
   * Enqueue a database task for execution.
   * Uses the task's title as the execution prompt.
   *
   * @param dbTask - Database task to enqueue
   * @param preset - Optional preset ID to use for execution (e.g., "builtin:code-assist" or collection ID)
   * @returns Result with success status and queued task ID
   */
  enqueueTask(dbTask: Task, preset?: string): EnqueueResult {
    try {
      // Check if task is already running or queued
      if (dbTask.status === "running") {
        return { success: false, error: "Task is already running" };
      }

      if (dbTask.queuedTaskId && this.taskQueue.getTask(dbTask.queuedTaskId)) {
        return { success: false, error: "Task is already queued" };
      }

      const args = resolveConfigArgs({
        preset,
        defaultCwd: this.defaultCwd,
        defaultConfigPath: this.defaultConfigPath,
        configMerger: this.configMerger,
        collectionService: this.collectionService,
      });

      // Enqueue the task with the title as the prompt
      const queuedTask = this.taskQueue.enqueue({
        taskType: this.taskType,
        payload: {
          prompt: dbTask.title,
          cwd: this.defaultCwd,
          dbTaskId: dbTask.id, // Include for reference in handlers
          args: args.length > 0 ? args : undefined,
        },
        priority: dbTask.priority,
      });

      // Store the mapping for event correlation
      this.taskIdMap.set(queuedTask.id, dbTask.id);

      // Update the database task with queue info
      this.taskRepository.update(dbTask.id, {
        status: "pending",
        queuedTaskId: queuedTask.id,
        // Clear any previous error
        errorMessage: null,
        startedAt: null,
        completedAt: null,
      });

      return { success: true, queuedTaskId: queuedTask.id };
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      return { success: false, error: errorMessage };
    }
  }

  /**
   * Enqueue all pending database tasks for execution.
   *
   * @returns Result with count of enqueued tasks and any errors
   */
  enqueueAllPending(): EnqueueAllResult {
    const pendingTasks = this.taskRepository.findAll("open");
    const errors: Array<{ taskId: string; error: string }> = [];
    let enqueued = 0;

    for (const task of pendingTasks) {
      // Skip blocked tasks
      if (task.blockedBy) {
        const blocker = this.taskRepository.findById(task.blockedBy);
        if (blocker && blocker.status !== "closed") {
          continue; // Still blocked
        }
      }

      const result = this.enqueueTask(task);
      if (result.success) {
        enqueued++;
      } else {
        errors.push({ taskId: task.id, error: result.error || "Unknown error" });
      }
    }

    return { enqueued, errors };
  }

  /**
   * Get execution status for a database task.
   */
  getExecutionStatus(dbTaskId: string): ExecutionStatus {
    const dbTask = this.taskRepository.findById(dbTaskId);

    if (!dbTask || !dbTask.queuedTaskId) {
      return { isQueued: false };
    }

    const queuedTask = this.taskQueue.getTask(dbTask.queuedTaskId);

    return {
      isQueued: !!queuedTask,
      queuedTask,
    };
  }

  /**
   * Reset a failed task and re-enqueue it for execution.
   */
  retryTask(dbTaskId: string): EnqueueResult {
    const dbTask = this.taskRepository.findById(dbTaskId);

    if (!dbTask) {
      return { success: false, error: "Task not found" };
    }

    if (dbTask.status !== "failed") {
      return { success: false, error: "Only failed tasks can be retried" };
    }

    // Reset the task state
    this.taskRepository.update(dbTaskId, {
      status: "open",
      queuedTaskId: null,
      errorMessage: null,
      startedAt: null,
      completedAt: null,
    });

    // Fetch the updated task and enqueue it
    const updatedTask = this.taskRepository.findById(dbTaskId);
    if (!updatedTask) {
      return { success: false, error: "Task not found after reset" };
    }

    return this.enqueueTask(updatedTask);
  }

  /**
   * Recover tasks that are stuck in 'running' state.
   * This handles cases where the server restarted while a task was executing.
   * Stuck tasks are marked as failed.
   */
  recoverStuckTasks(): number {
    const runningTasks = this.taskRepository.findAll("running");
    let recoveredCount = 0;

    for (const task of runningTasks) {
      this.taskRepository.update(task.id, {
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
   * Reconnect to running ralph processes after server restart.
   * Attempts to reconnect to each running task's process.
   * If alive, resumes output streaming. If dead, marks as failed.
   */
  reconnectRunningTasks(): { reconnected: number; failed: number } {
    if (!this.processSupervisor || !this.outputStreamer) {
      console.warn("ProcessSupervisor or FileOutputStreamer not available, skipping reconnection");
      return { reconnected: 0, failed: 0 };
    }

    const runningTasks = this.taskRepository.findAll("running");
    let reconnectedCount = 0;
    let failedCount = 0;

    for (const task of runningTasks) {
      try {
        const handle = this.processSupervisor.reconnect(task.id);

        if (handle && handle.isAlive) {
          console.log(`Reconnected to task ${task.id} (PID ${handle.pid})`);

          // Resume output streaming
          this.outputStreamer.stream(task.id, handle.taskDir, (line, source) => {
            this.eventBus.publish("task.output", {
              taskId: task.id,
              line,
              source,
            });
          });

          reconnectedCount++;
        } else {
          // Process is dead, mark task as failed
          const status = this.processSupervisor.getStatus(task.id);
          const error = status?.error || "Process died during server restart";

          this.taskRepository.update(task.id, {
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
        this.taskRepository.update(task.id, {
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
   * Cancel a running task by stopping the underlying process.
   */
  cancelTask(dbTaskId: string): EnqueueResult {
    const dbTask = this.taskRepository.findById(dbTaskId);

    if (!dbTask) {
      return { success: false, error: "Task not found" };
    }

    if (dbTask.status !== "running") {
      return { success: false, error: "Only running tasks can be cancelled" };
    }

    if (!this.processSupervisor) {
      return { success: false, error: "Process supervisor not available" };
    }

    // Stop the process
    const stopResult = this.processSupervisor.stop(dbTaskId);

    if (!stopResult.success) {
      // Special case: process already terminated means the task ended unexpectedly
      // We should update the status to reflect reality and return success
      if (stopResult.error === "Process already terminated") {
        console.warn(`[TaskBridge] Task ${dbTaskId}: Process already terminated, marking as failed`);
        this.taskRepository.update(dbTaskId, {
          status: "failed",
          completedAt: new Date(),
          errorMessage: "Process terminated unexpectedly",
          exitCode: -1,
        });

        if (dbTask.queuedTaskId) {
          this.taskIdMap.delete(dbTask.queuedTaskId);
        }

        return { success: true };
      }

      return { success: false, error: stopResult.error || "Failed to stop process" };
    }

    // Update task status to failed with cancellation message
    this.taskRepository.update(dbTaskId, {
      status: "failed",
      completedAt: new Date(),
      errorMessage: `Task cancelled by user (signal: ${stopResult.signal})`,
      exitCode: 143, // Standard exit code for SIGTERM (128 + 15)
    });

    if (dbTask.queuedTaskId) {
      this.taskIdMap.delete(dbTask.queuedTaskId);
    }

    return { success: true };
  }

  /**
   * Poll for loop ID resolution after a task starts.
   *
   * Thin delegation to `scheduleLoopIdResolution` in `TaskBridge.loopResolver`.
   * Kept as a private method so the event-handler deps can bind `this` once
   * in the constructor.
   *
   * @param dbTaskId - The database task ID to update once the loop ID is found
   */
  private scheduleLoopIdResolution(dbTaskId: string): void {
    scheduleLoopIdResolution(
      {
        taskRepository: this.taskRepository,
        defaultCwd: this.defaultCwd,
      },
      dbTaskId
    );
  }

  /**
   * Clean up event subscriptions.
   * Call this when shutting down the service.
   */
  destroy(): void {
    for (const subscription of this.subscriptions) {
      subscription.unsubscribe();
    }
    this.subscriptions.length = 0;
    this.taskIdMap.clear();
  }
}
