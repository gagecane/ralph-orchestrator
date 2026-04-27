/**
 * Manages Ralph child-process lifecycle for planning sessions:
 * spawning, events-file polling, response-timeout watchdog, and
 * status transitions on exit.
 */

import * as fs from "node:fs/promises";
import * as path from "node:path";
import { spawn, ChildProcess } from "child_process";
import {
  SessionStatus,
  type ConversationEntry,
  type RalphEvent,
  type UserPromptPayload,
} from "./types";

const POLL_INTERVAL_MS = 500;

export interface ProcessManagerOptions {
  workspaceRoot: string;
  ralphPath: string;
  sessionsDir: string;
  /**
   * Response timeout in seconds. A session left in WaitingForInput for
   * longer than this is transitioned to TimedOut.
   */
  responseTimeoutSeconds: number;
  updateStatus: (sessionId: string, status: SessionStatus) => Promise<void>;
}

/**
 * Owns the background machinery for a running planning session.
 */
export class RalphProcessManager {
  private readonly runningProcesses = new Map<string, ChildProcess>();
  private readonly eventPollers = new Map<string, NodeJS.Timeout>();
  private readonly processedEventTimestamps = new Map<string, Set<string>>();
  private readonly waitingSince = new Map<string, number>();

  constructor(private readonly opts: ProcessManagerOptions) {}

  /**
   * Spawn Ralph for a session using the planning preset.
   */
  spawn(sessionId: string, prompt: string): void {
    const presetPath = path.join(
      this.opts.workspaceRoot,
      "crates",
      "ralph-cli",
      "presets",
      "planning.yml",
    );

    const args = [
      "run",
      "-c", presetPath,
      "-p", prompt,
      "--no-tui",
    ];

    console.log(
      `[PlanningService] Spawning ralph for session ${sessionId}:`,
      this.opts.ralphPath,
      args.join(" "),
    );

    const child = spawn(this.opts.ralphPath, args, {
      cwd: this.opts.workspaceRoot,
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...process.env,
        RALPH_PLANNING_SESSION_ID: sessionId,
      },
    });

    this.runningProcesses.set(sessionId, child);
    this.processedEventTimestamps.set(sessionId, new Set());
    this.startEventPolling(sessionId);

    child.stdout?.on("data", (data: Buffer) => {
      console.log(`[PlanningService:${sessionId}] stdout:`, data.toString().trim());
    });

    child.stderr?.on("data", (data: Buffer) => {
      console.error(`[PlanningService:${sessionId}] ralph stderr:`, data.toString());
    });

    child.on("exit", (code, signal) => {
      console.log(
        `[PlanningService:${sessionId}] ralph exited: code=${code}, signal=${signal}`,
      );
      this.cleanupSession(sessionId);
      const newStatus = code === 0 ? SessionStatus.Completed : SessionStatus.Failed;
      void this.opts.updateStatus(sessionId, newStatus);
    });

    child.on("error", (err) => {
      console.error(`[PlanningService:${sessionId}] ralph error:`, err);
      this.cleanupSession(sessionId);
    });
  }

  /**
   * Kill a running session. Returns true if a process was actually killed.
   */
  kill(sessionId: string): boolean {
    const child = this.runningProcesses.get(sessionId);
    if (!child) return false;
    child.kill("SIGTERM");
    this.runningProcesses.delete(sessionId);
    return true;
  }

  /**
   * Whether the session currently has a running process.
   */
  isRunning(sessionId: string): boolean {
    return this.runningProcesses.has(sessionId);
  }

  private cleanupSession(sessionId: string): void {
    this.runningProcesses.delete(sessionId);
    this.stopEventPolling(sessionId);
    this.waitingSince.delete(sessionId);
  }

  private startEventPolling(sessionId: string): void {
    const poller = setInterval(() => {
      void this.pollEventsFile(sessionId);
      void this.checkResponseTimeout(sessionId);
    }, POLL_INTERVAL_MS);

    this.eventPollers.set(sessionId, poller);
    console.log(`[PlanningService:${sessionId}] Started event polling`);
  }

  private stopEventPolling(sessionId: string): void {
    const poller = this.eventPollers.get(sessionId);
    if (poller) {
      clearInterval(poller);
      this.eventPollers.delete(sessionId);
      this.processedEventTimestamps.delete(sessionId);
      console.log(`[PlanningService:${sessionId}] Stopped event polling`);
    }
  }

  private async getCurrentEventsPath(): Promise<string | null> {
    const currentEventsPath = path.join(this.opts.workspaceRoot, ".ralph", "current-events");
    try {
      const relativePath = await fs.readFile(currentEventsPath, "utf-8");
      return path.join(this.opts.workspaceRoot, relativePath.trim());
    } catch (err) {
      console.warn("[PlanningService] Could not read current-events file:", err);
      return null;
    }
  }

  private async pollEventsFile(sessionId: string): Promise<void> {
    const eventsPath = await this.getCurrentEventsPath();
    if (!eventsPath) return;

    try {
      const content = await fs.readFile(eventsPath, "utf-8");
      const lines = content.trim().split("\n").filter((l) => l.trim());
      const processedTimestamps =
        this.processedEventTimestamps.get(sessionId) ?? new Set<string>();

      for (const line of lines) {
        try {
          const event = JSON.parse(line) as RalphEvent;
          if (processedTimestamps.has(event.ts)) continue;

          if (event.topic === "user.prompt") {
            const payload = event.payload as UserPromptPayload;
            const promptId = payload.id ?? `q${processedTimestamps.size + 1}`;
            const questionText =
              payload.question ??
              (typeof payload === "string" ? payload : JSON.stringify(payload));

            console.log(
              `[PlanningService:${sessionId}] Detected user.prompt from events file: id=${promptId}`,
            );

            const conversationPath = path.join(
              this.opts.sessionsDir,
              sessionId,
              "conversation.jsonl",
            );
            const entry: ConversationEntry = {
              type: "user_prompt",
              id: promptId,
              text: questionText,
              ts: event.ts,
            };
            await fs.appendFile(conversationPath, JSON.stringify(entry) + "\n");

            await this.opts.updateStatus(sessionId, SessionStatus.WaitingForInput);
            this.waitingSince.set(sessionId, Date.now());
          }

          processedTimestamps.add(event.ts);
        } catch (parseErr) {
          console.warn(
            `[PlanningService:${sessionId}] Skipping malformed event line:`,
            parseErr,
          );
        }
      }

      this.processedEventTimestamps.set(sessionId, processedTimestamps);
    } catch (err) {
      console.warn(`[PlanningService:${sessionId}] Could not read events file:`, err);
    }
  }

  /**
   * If a session has been WaitingForInput longer than the configured timeout,
   * transition it to TimedOut and tear down.
   */
  private async checkResponseTimeout(sessionId: string): Promise<void> {
    const waitStart = this.waitingSince.get(sessionId);
    if (waitStart === undefined) return;

    const elapsedSec = (Date.now() - waitStart) / 1000;
    if (elapsedSec < this.opts.responseTimeoutSeconds) return;

    console.log(
      `[PlanningService:${sessionId}] Response timeout after ${elapsedSec.toFixed(0)}s; marking TimedOut`,
    );
    this.waitingSince.delete(sessionId);
    this.kill(sessionId);
    await this.opts.updateStatus(sessionId, SessionStatus.TimedOut);
  }

  /**
   * Clear the waiting-for-response watchdog (called when a response arrives).
   */
  clearWaiting(sessionId: string): void {
    this.waitingSince.delete(sessionId);
  }
}
