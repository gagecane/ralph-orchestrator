/**
 * TaskExecutor
 *
 * Executes a single task on behalf of the Dispatcher. Handles:
 * - Handler invocation with an execution context
 * - Timeout enforcement
 * - Cancellation via AbortController
 * - State transitions in TaskQueueService
 * - Event publication on EventBus
 * - Stats tracking (success, failure, timeout, cancelled counts)
 *
 * Kept separate from Dispatcher.ts so the orchestration logic (polling,
 * lifecycle, handler registry) remains readable while the per-task
 * execution pipeline lives in one focused place.
 */

import { TaskQueueService, QueuedTask } from "./TaskQueueService";
import { EventBus } from "./EventBus";
import {
  TaskHandler,
  TaskExecutionContext,
  TaskExecutionResult,
  ExecutorCounters,
} from "./DispatcherTypes";

/**
 * Collaborators and configuration for TaskExecutor.
 */
export interface TaskExecutorDeps {
  queue: TaskQueueService;
  eventBus: EventBus;
  taskTimeoutMs: number;
  /** Lookup the handler to use for a given task type. */
  getHandler: (taskType: string) => TaskHandler | undefined;
  /** Shared counters mutated by the executor after each task finishes. */
  counters: ExecutorCounters;
  /** Abort controllers for in-flight tasks, keyed by task id. */
  runningTasks: Map<string, AbortController>;
}

export class TaskExecutor {
  private readonly queue: TaskQueueService;
  private readonly eventBus: EventBus;
  private readonly taskTimeoutMs: number;
  private readonly getHandler: (taskType: string) => TaskHandler | undefined;
  private readonly counters: ExecutorCounters;
  private readonly runningTasks: Map<string, AbortController>;

  constructor(deps: TaskExecutorDeps) {
    this.queue = deps.queue;
    this.eventBus = deps.eventBus;
    this.taskTimeoutMs = deps.taskTimeoutMs;
    this.getHandler = deps.getHandler;
    this.counters = deps.counters;
    this.runningTasks = deps.runningTasks;
  }

  /**
   * Execute a single task.
   * Handles timeouts, errors, and state transitions.
   */
  async execute(task: QueuedTask): Promise<TaskExecutionResult> {
    const startTime = Date.now();
    const correlationId = `exec-${task.id}-${startTime}`;

    // Create abort controller for this task
    const abortController = new AbortController();
    this.runningTasks.set(task.id, abortController);

    // Create execution context
    const context: TaskExecutionContext = {
      eventBus: this.eventBus,
      correlationId,
      signal: abortController.signal,
    };

    // Publish task started event
    await this.eventBus.publish(
      "task.started",
      {
        taskId: task.id,
        taskType: task.taskType,
        payload: task.payload,
        priority: task.priority,
      },
      { correlationId }
    );

    // Get the handler
    const handler = this.getHandler(task.taskType);

    let result: TaskExecutionResult;

    if (!handler) {
      result = await this.handleMissingHandler(task, correlationId, startTime);
    } else {
      result = await this.runWithHandler(task, handler, context, abortController, correlationId, startTime);
    }

    // Update shared stats
    this.counters.totalProcessed++;
    this.counters.totalDurationMs += result.durationMs;

    // Remove from running tasks
    this.runningTasks.delete(task.id);

    return result;
  }

  /**
   * Handle the case where no handler is registered for a task type.
   * Marks the task as failed and publishes a task.failed event.
   */
  private async handleMissingHandler(
    task: QueuedTask,
    correlationId: string,
    startTime: number
  ): Promise<TaskExecutionResult> {
    const errorMsg = `No handler registered for task type: ${task.taskType}`;
    this.queue.fail(task.id, errorMsg);

    const result: TaskExecutionResult = {
      task: this.queue.getTask(task.id) ?? task,
      success: false,
      error: errorMsg,
      durationMs: Date.now() - startTime,
    };

    await this.eventBus.publish(
      "task.failed",
      {
        taskId: task.id,
        taskType: task.taskType,
        error: errorMsg,
        durationMs: result.durationMs,
      },
      { correlationId }
    );

    // Note: We do NOT increment failureCount here — this matches the legacy
    // behaviour of the original Dispatcher.executeTask (which also omitted it
    // for the missing-handler branch). Preserving that to avoid changing the
    // public DispatcherStats contract.

    return result;
  }

  /**
   * Run a task through its handler with timeout + cancellation racing.
   * Dispatches success, failure, timeout, or cancelled events based on outcome.
   */
  private async runWithHandler(
    task: QueuedTask,
    handler: TaskHandler,
    context: TaskExecutionContext,
    abortController: AbortController,
    correlationId: string,
    startTime: number
  ): Promise<TaskExecutionResult> {
    // Set up timeout
    let timeoutId: ReturnType<typeof setTimeout> | undefined;
    const timeoutError = new Error(`Task timeout after ${this.taskTimeoutMs}ms`);
    const timeoutPromise = new Promise<never>((_, reject) => {
      timeoutId = setTimeout(() => {
        // Pass the timeout error as the abort reason so cancellation promise
        // won't race ahead with a generic "aborted" message
        abortController.abort(timeoutError);
        reject(timeoutError);
      }, this.taskTimeoutMs);
    });

    // Set up cancellation monitoring
    const cancellationPromise = new Promise<never>((_, reject) => {
      if (abortController.signal.aborted) {
        reject(abortController.signal.reason || new Error("Task cancelled"));
      } else {
        abortController.signal.addEventListener("abort", () => {
          reject(abortController.signal.reason || new Error("Task cancelled"));
        });
      }
    });

    try {
      // Execute handler with timeout and cancellation
      const handlerResult = await Promise.race([
        Promise.resolve(handler(task, context)),
        timeoutPromise,
        cancellationPromise,
      ]);

      if (timeoutId) {
        clearTimeout(timeoutId);
      }

      this.queue.complete(task.id);

      const result: TaskExecutionResult = {
        task: this.queue.getTask(task.id) ?? task,
        success: true,
        result: handlerResult,
        durationMs: Date.now() - startTime,
      };

      await this.eventBus.publish(
        "task.completed",
        {
          taskId: task.id,
          taskType: task.taskType,
          result: handlerResult,
          durationMs: result.durationMs,
        },
        { correlationId }
      );

      this.counters.successCount++;
      return result;
    } catch (error) {
      if (timeoutId) {
        clearTimeout(timeoutId);
      }

      const errorMsg = error instanceof Error ? error.message : String(error);
      const isTimeout = errorMsg.includes("Task timeout");
      // Check for cancellation (passed as string "cancelled" or AbortError)
      const isCancelled =
        error === "cancelled" ||
        (error instanceof Error && error.name === "AbortError") ||
        abortController.signal.aborted;

      const result: TaskExecutionResult = {
        task: this.queue.getTask(task.id) ?? task,
        success: false,
        error: errorMsg,
        durationMs: Date.now() - startTime,
      };

      if (isTimeout) {
        // Check timeout FIRST - the timeout handler aborts the controller,
        // so we'd otherwise incorrectly detect this as a cancellation.
        this.queue.fail(task.id, errorMsg);

        await this.eventBus.publish(
          "task.timeout",
          {
            taskId: task.id,
            taskType: task.taskType,
            timeoutMs: this.taskTimeoutMs,
            durationMs: result.durationMs,
          },
          { correlationId }
        );
        this.counters.timeoutCount++;
        this.counters.failureCount++;
      } else if (isCancelled) {
        // User-initiated cancellation (not timeout).
        this.queue.cancel(task.id);

        await this.eventBus.publish(
          "task.cancelled",
          {
            taskId: task.id,
            taskType: task.taskType,
            reason: error === "cancelled" ? "cancelled by user" : errorMsg,
            durationMs: result.durationMs,
          },
          { correlationId }
        );
        this.counters.cancelledCount++;
      } else {
        // Generic failure.
        this.queue.fail(task.id, errorMsg);

        await this.eventBus.publish(
          "task.failed",
          {
            taskId: task.id,
            taskType: task.taskType,
            error: errorMsg,
            durationMs: result.durationMs,
          },
          { correlationId }
        );
        this.counters.failureCount++;
      }

      return result;
    }
  }
}
