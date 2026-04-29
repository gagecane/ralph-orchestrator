/**
 * Dispatcher
 *
 * The core execution engine of the "Employee" model. The Dispatcher:
 * 1. Polls TaskQueueService for pending tasks
 * 2. Executes tasks by invoking registered handlers
 * 3. Manages state transitions (PENDING → RUNNING → COMPLETED/FAILED)
 * 4. Publishes events via EventBus for coordination with other components
 *
 * Integration:
 * - TaskQueueService: Task storage and state management
 * - EventBus: Pub/sub for workflow coordination (e.g., 'task.started', 'task.completed')
 * - TaskHandlers: Registered functions that execute specific task types
 *
 * Lifecycle:
 *   dispatcher.start() → polling loop begins
 *   dispatcher.stop()  → graceful shutdown, waits for running tasks
 *
 * Per-task execution (handler invocation, timeout, cancellation, state
 * transitions, event publication) is delegated to TaskExecutor so this
 * class can focus on orchestration.
 */

import { TaskQueueService, QueuedTask } from "./TaskQueueService";
import { EventBus } from "./EventBus";
import { TaskState } from "./TaskState";
import { TaskExecutor } from "./TaskExecutor";
import {
  TaskHandler,
  TaskExecutionResult,
  DispatcherOptions,
  DispatcherStats,
  ExecutorCounters,
} from "./DispatcherTypes";

// Re-export types from the types module so existing imports keep working.
export type {
  TaskHandler,
  TaskExecutionContext,
  TaskExecutionResult,
  DispatcherOptions,
  DispatcherEventType,
  DispatcherStats,
} from "./DispatcherTypes";

/**
 * Dispatcher
 *
 * Manages the task execution lifecycle. Uses a polling loop to dequeue
 * pending tasks and execute them via registered handlers.
 */
export class Dispatcher {
  /** Task queue service for enqueueing/dequeueing */
  private readonly queue: TaskQueueService;
  /** Event bus for publishing lifecycle events */
  private readonly eventBus: EventBus;
  /** Registered task handlers by type */
  private readonly handlers: Map<string, TaskHandler> = new Map();
  /** Default handler for unregistered task types */
  private defaultHandler?: TaskHandler;

  /** Configuration */
  private readonly pollIntervalMs: number;
  private readonly maxConcurrent: number;
  private readonly taskTimeoutMs: number;

  /** Runtime state */
  private running: boolean = false;
  private pollTimeoutId?: ReturnType<typeof setTimeout>;
  private readonly runningTasks: Map<string, AbortController> = new Map();
  private startedAt?: Date;

  /** Statistics (mutable counters shared with TaskExecutor). */
  private readonly counters: ExecutorCounters = {
    totalProcessed: 0,
    successCount: 0,
    failureCount: 0,
    cancelledCount: 0,
    timeoutCount: 0,
    totalDurationMs: 0,
  };

  /** Per-task executor (handler invocation, timeout, cancellation, events). */
  private readonly executor: TaskExecutor;

  /**
   * Create a new Dispatcher
   *
   * @param queue - TaskQueueService instance
   * @param eventBus - EventBus instance for pub/sub
   * @param options - Configuration options
   */
  constructor(queue: TaskQueueService, eventBus: EventBus, options: DispatcherOptions = {}) {
    this.queue = queue;
    this.eventBus = eventBus;
    this.pollIntervalMs = options.pollIntervalMs ?? 100;
    this.maxConcurrent = options.maxConcurrent ?? 1;
    this.taskTimeoutMs = options.taskTimeoutMs ?? 7200000;

    this.executor = new TaskExecutor({
      queue: this.queue,
      eventBus: this.eventBus,
      taskTimeoutMs: this.taskTimeoutMs,
      getHandler: (taskType) => this.getHandler(taskType),
      counters: this.counters,
      runningTasks: this.runningTasks,
    });

    if (options.autoStart) {
      this.start();
    }
  }

  /**
   * Register a handler for a specific task type.
   *
   * @param taskType - The task type string (e.g., 'build.compile')
   * @param handler - Function to execute for this task type
   * @returns this for chaining
   */
  registerHandler<TPayload = Record<string, unknown>, TResult = unknown>(
    taskType: string,
    handler: TaskHandler<TPayload, TResult>
  ): this {
    this.handlers.set(taskType, handler as TaskHandler);
    return this;
  }

  /**
   * Register a default handler for unregistered task types.
   *
   * @param handler - Function to execute for unknown task types
   * @returns this for chaining
   */
  registerDefaultHandler<TResult = unknown>(
    handler: TaskHandler<Record<string, unknown>, TResult>
  ): this {
    this.defaultHandler = handler as TaskHandler;
    return this;
  }

  /**
   * Unregister a handler for a task type.
   *
   * @param taskType - The task type to unregister
   * @returns true if handler was found and removed
   */
  unregisterHandler(taskType: string): boolean {
    return this.handlers.delete(taskType);
  }

  /**
   * Get the handler for a task type.
   * Returns the registered handler or the default handler if set.
   */
  private getHandler(taskType: string): TaskHandler | undefined {
    return this.handlers.get(taskType) ?? this.defaultHandler;
  }

  /**
   * Cancel a task by its ID.
   *
   * - If task is PENDING, transitions to CANCELLED.
   * - If task is RUNNING, aborts execution and transitions to CANCELLED.
   *
   * @param taskId - ID of the task to cancel
   * @returns true if task was cancelled, false if not found or already terminal
   */
  async cancelTask(taskId: string): Promise<boolean> {
    // Check if running
    const controller = this.runningTasks.get(taskId);
    if (controller) {
      controller.abort("cancelled");
      return true;
    }

    // Check if pending in queue
    const task = this.queue.getTask(taskId);
    if (task && task.state === TaskState.PENDING) {
      this.queue.cancel(taskId);
      this.counters.cancelledCount++;

      await this.eventBus.publish("task.cancelled", {
        taskId,
        taskType: task.taskType,
        reason: "cancelled by user",
        durationMs: 0,
      });

      return true;
    }

    return false;
  }

  /**
   * Start the dispatcher polling loop.
   * Does nothing if already running.
   */
  start(): void {
    if (this.running) {
      return;
    }

    this.running = true;
    this.startedAt = new Date();

    // Publish start event
    this.eventBus.publishSync("dispatcher.started", {
      timestamp: this.startedAt,
      config: {
        pollIntervalMs: this.pollIntervalMs,
        maxConcurrent: this.maxConcurrent,
        taskTimeoutMs: this.taskTimeoutMs,
      },
    });

    // Start the polling loop
    this.poll();
  }

  /**
   * Stop the dispatcher gracefully.
   * Waits for currently running tasks to complete.
   *
   * @param forceTimeoutMs - Force stop after this many ms (default: wait indefinitely)
   * @returns Promise that resolves when stopped
   */
  async stop(forceTimeoutMs?: number): Promise<void> {
    if (!this.running) {
      return;
    }

    this.running = false;

    // Clear the poll timeout
    if (this.pollTimeoutId) {
      clearTimeout(this.pollTimeoutId);
      this.pollTimeoutId = undefined;
    }

    // Wait for running tasks to complete
    const waitForRunning = async () => {
      while (this.runningTasks.size > 0) {
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
    };

    if (forceTimeoutMs !== undefined) {
      // Race between waiting and timeout
      await Promise.race([
        waitForRunning(),
        new Promise<void>((resolve) => {
          setTimeout(() => {
            // Force cancel all running tasks
            for (const [, controller] of this.runningTasks) {
              controller.abort();
            }
            resolve();
          }, forceTimeoutMs);
        }),
      ]);
    } else {
      await waitForRunning();
    }

    // Publish stop event
    this.eventBus.publishSync("dispatcher.stopped", {
      timestamp: new Date(),
      stats: this.getStats(),
    });
  }

  /**
   * Main polling loop.
   * Dequeues pending tasks and executes them.
   * Fills all available slots in a single poll cycle for parallel execution.
   */
  private poll(): void {
    if (!this.running) {
      return;
    }

    const availableSlots = this.maxConcurrent - this.runningTasks.size;

    if (availableSlots > 0) {
      let tasksStarted = 0;

      // Dequeue up to availableSlots tasks
      for (let i = 0; i < availableSlots; i++) {
        const { task } = this.queue.dequeue();

        if (task) {
          // Execute the task asynchronously
          this.executor.execute(task).catch((error) => {
            // This shouldn't happen as executor.execute handles its own errors
            console.error("Unexpected error in executor.execute:", error);
          });
          tasksStarted++;
        } else {
          break; // No more tasks in queue
        }
      }

      // Emit idle only if queue empty AND nothing running
      if (tasksStarted === 0 && this.runningTasks.size === 0) {
        this.eventBus.publishSync("dispatcher.idle", {
          timestamp: new Date(),
          stats: this.getStats(),
        });
      }
    }

    // Schedule next poll
    this.pollTimeoutId = setTimeout(() => this.poll(), this.pollIntervalMs);
  }

  /**
   * Check if the dispatcher is currently running.
   */
  isRunning(): boolean {
    return this.running;
  }

  /**
   * Get dispatcher statistics.
   */
  getStats(): DispatcherStats {
    return {
      isRunning: this.running,
      totalProcessed: this.counters.totalProcessed,
      successCount: this.counters.successCount,
      failureCount: this.counters.failureCount,
      cancelledCount: this.counters.cancelledCount,
      runningCount: this.runningTasks.size,
      timeoutCount: this.counters.timeoutCount,
      avgDurationMs:
        this.counters.totalProcessed > 0
          ? this.counters.totalDurationMs / this.counters.totalProcessed
          : 0,
      uptimeMs: this.startedAt ? Date.now() - this.startedAt.getTime() : 0,
    };
  }

  /**
   * Get list of registered task types.
   */
  getRegisteredTaskTypes(): string[] {
    return Array.from(this.handlers.keys());
  }

  /**
   * Check if a handler is registered for a task type.
   */
  hasHandler(taskType: string): boolean {
    return this.handlers.has(taskType) || this.defaultHandler !== undefined;
  }

  /**
   * Execute a single task immediately without polling.
   * Useful for testing or one-off executions.
   *
   * @param task - Task to execute
   * @returns Execution result
   */
  async executeOnce(task: QueuedTask): Promise<TaskExecutionResult> {
    return this.executor.execute(task);
  }
}
