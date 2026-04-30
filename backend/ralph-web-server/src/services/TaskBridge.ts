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
 * - `TaskBridge.eventHandlers.ts` — EventBus → DB lifecycle handlers
 * - `TaskBridge.lifecycle.ts`     — recover / reconnect / cancel helpers
 */

import { TaskRepository } from "../repositories";
import { ProcessSupervisor } from "../runner/ProcessSupervisor";
import { FileOutputStreamer } from "../runner/FileOutputStreamer";
import { CollectionService } from "./CollectionService";
import { ConfigMerger } from "./ConfigMerger";
import { TaskQueueService } from "../queue/TaskQueueService";
import { EventBus, Subscription } from "../queue/EventBus";
import { Task } from "../db/schema";

import { resolveConfigArgs } from "./TaskBridge.configResolver";
import { scheduleLoopIdResolution } from "./TaskBridge.loopResolver";
import { subscribeLifecycleEvents } from "./TaskBridge.eventHandlers";
import {
  cancelTask as cancelTaskImpl,
  recoverStuckTasks as recoverStuckTasksImpl,
  reconnectRunningTasks as reconnectRunningTasksImpl,
} from "./TaskBridge.lifecycle";
import type {
  EnqueueAllResult,
  EnqueueResult,
  ExecutionStatus,
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
    this.subscriptions.push(
      ...subscribeLifecycleEvents(this.eventBus, {
        taskRepository: this.taskRepository,
        taskIdMap: this.taskIdMap,
        defaultCwd: this.defaultCwd,
        scheduleLoopIdResolution: (dbTaskId) =>
          this.scheduleLoopIdResolution(dbTaskId),
      })
    );
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
    return recoverStuckTasksImpl(this.taskRepository);
  }

  /**
   * Reconnect to running ralph processes after server restart.
   * Attempts to reconnect to each running task's process.
   * If alive, resumes output streaming. If dead, marks as failed.
   */
  reconnectRunningTasks(): { reconnected: number; failed: number } {
    if (!this.processSupervisor || !this.outputStreamer) {
      console.warn(
        "ProcessSupervisor or FileOutputStreamer not available, skipping reconnection"
      );
      return { reconnected: 0, failed: 0 };
    }

    return reconnectRunningTasksImpl({
      taskRepository: this.taskRepository,
      processSupervisor: this.processSupervisor,
      outputStreamer: this.outputStreamer,
      eventBus: this.eventBus,
    });
  }

  /**
   * Cancel a running task by stopping the underlying process.
   */
  cancelTask(dbTaskId: string): EnqueueResult {
    return cancelTaskImpl(
      {
        taskRepository: this.taskRepository,
        processSupervisor: this.processSupervisor,
        taskIdMap: this.taskIdMap,
      },
      dbTaskId
    );
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
