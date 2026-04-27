/**
 * PlanningService
 *
 * Manages planning session lifecycle for the web-based planning page:
 * session directory layout, metadata + conversation files, and
 * coordination with the Ralph child process (see RalphProcessManager).
 */

import * as fs from "node:fs/promises";
import * as path from "node:path";
import { v4 as uuidv4 } from "uuid";

import {
  SessionStatus,
  type ConversationEntry,
  type FrontendConversationEntry,
  type PlanningServiceOptions,
  type PlanningSessionDetail,
  type PlanningSessionSummary,
  type SessionMetadata,
} from "./planning/types";
import { generateTitle, toFrontendEntry, toFrontendStatus } from "./planning/shaping";
import { RalphProcessManager } from "./planning/RalphProcessManager";

// Re-exports for backwards-compatible public API.
export {
  SessionStatus,
  type ConversationEntry,
  type FrontendConversationEntry,
  type PlanningServiceOptions,
  type PlanningSessionDetail,
  type PlanningSessionSummary,
  type SessionMetadata,
} from "./planning/types";

const DEFAULT_TIMEOUT_SECONDS = 300;

/**
 * Service for managing planning sessions.
 */
export class PlanningService {
  private readonly workspaceRoot: string;
  private readonly sessionsDir: string;
  private readonly processManager: RalphProcessManager;

  constructor(options: PlanningServiceOptions) {
    this.workspaceRoot = options.workspaceRoot;
    this.sessionsDir = path.join(this.workspaceRoot, ".ralph", "planning-sessions");
    this.processManager = new RalphProcessManager({
      workspaceRoot: this.workspaceRoot,
      ralphPath: options.ralphPath ?? "ralph",
      sessionsDir: this.sessionsDir,
      responseTimeoutSeconds: options.defaultTimeoutSeconds ?? DEFAULT_TIMEOUT_SECONDS,
      updateStatus: (id, status) => this.updateSessionStatus(id, status),
    });
  }

  /**
   * Get all planning sessions as summaries.
   */
  async listSessions(): Promise<PlanningSessionSummary[]> {
    await this.ensureSessionsDir();

    const entries = await fs.readdir(this.sessionsDir, { withFileTypes: true });
    const sessions: PlanningSessionSummary[] = [];

    for (const entry of entries) {
      if (!entry.isDirectory()) continue;

      const metadataPath = path.join(this.sessionsDir, entry.name, "session.json");
      try {
        const content = await fs.readFile(metadataPath, "utf-8");
        const metadata: SessionMetadata = JSON.parse(content);

        const conversationPath = path.join(
          this.sessionsDir,
          entry.name,
          "conversation.jsonl",
        );
        let messageCount = 0;
        try {
          const convContent = await fs.readFile(conversationPath, "utf-8");
          messageCount = convContent.trim().split("\n").filter((l: string) => l.trim()).length;
        } catch (err) {
          console.warn(
            `[PlanningService] Could not read conversation file for session ${entry.name}:`,
            err,
          );
        }

        sessions.push({
          id: metadata.id,
          title: generateTitle(metadata.prompt),
          prompt: metadata.prompt,
          status: toFrontendStatus(metadata.status),
          createdAt: metadata.created_at,
          updatedAt: metadata.updated_at,
          messageCount,
          iterations: metadata.iterations,
        });
      } catch (err) {
        console.warn(
          `[PlanningService] Skipping session ${entry.name} due to invalid metadata:`,
          err,
        );
      }
    }

    sessions.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    return sessions;
  }

  /**
   * Get a specific session with full details.
   */
  async getSession(sessionId: string): Promise<PlanningSessionDetail> {
    const sessionDir = path.join(this.sessionsDir, sessionId);

    const metadataPath = path.join(sessionDir, "session.json");
    const metadataContent = await fs.readFile(metadataPath, "utf-8");
    const metadata: SessionMetadata = JSON.parse(metadataContent);

    const conversationPath = path.join(sessionDir, "conversation.jsonl");
    const conversation: FrontendConversationEntry[] = [];

    try {
      const conversationContent = await fs.readFile(conversationPath, "utf-8");
      const entries = conversationContent
        .trim()
        .split("\n")
        .filter((line: string) => line.trim().length > 0)
        .map((line: string) => JSON.parse(line) as ConversationEntry);

      for (const entry of entries) {
        conversation.push(toFrontendEntry(entry));
      }
    } catch (err) {
      console.warn(
        `[PlanningService:${sessionId}] Could not load conversation file:`,
        err,
      );
    }

    const artifactsDir = path.join(sessionDir, "artifacts");
    let artifacts: string[] = [];
    try {
      const artifactEntries = await fs.readdir(artifactsDir);
      artifacts = artifactEntries.filter((e: string) => !e.startsWith("."));
    } catch (err) {
      console.warn(
        `[PlanningService:${sessionId}] Could not read artifacts directory:`,
        err,
      );
    }

    const isCompleted = metadata.status === SessionStatus.Completed;

    return {
      id: metadata.id,
      prompt: metadata.prompt,
      title: generateTitle(metadata.prompt),
      status: toFrontendStatus(metadata.status),
      createdAt: metadata.created_at,
      updatedAt: metadata.updated_at,
      completedAt: isCompleted ? metadata.updated_at : undefined,
      conversation,
      artifacts,
      messageCount: conversation.length,
    };
  }

  /**
   * Start a new planning session.
   */
  async startSession(prompt: string): Promise<{ sessionId: string }> {
    await this.ensureSessionsDir();

    const sessionId = this.generateSessionId();
    const sessionDir = path.join(this.sessionsDir, sessionId);

    await fs.mkdir(sessionDir, { recursive: true });
    await fs.mkdir(path.join(sessionDir, "artifacts"), { recursive: true });

    const now = new Date().toISOString();
    const metadata: SessionMetadata = {
      id: sessionId,
      prompt,
      status: SessionStatus.Active,
      created_at: now,
      updated_at: now,
      iterations: 0,
    };

    await fs.writeFile(
      path.join(sessionDir, "session.json"),
      JSON.stringify(metadata, null, 2),
    );
    await fs.writeFile(path.join(sessionDir, "conversation.jsonl"), "");

    this.processManager.spawn(sessionId, prompt);

    return { sessionId };
  }

  /**
   * Submit a user response to a planning session.
   */
  async submitResponse(
    sessionId: string,
    promptId: string,
    response: string,
  ): Promise<void> {
    const conversationPath = path.join(this.sessionsDir, sessionId, "conversation.jsonl");
    const entry: ConversationEntry = {
      type: "user_response",
      id: promptId,
      text: response,
      ts: new Date().toISOString(),
    };
    await fs.appendFile(conversationPath, JSON.stringify(entry) + "\n");

    const metadataPath = path.join(this.sessionsDir, sessionId, "session.json");
    const metadataContent = await fs.readFile(metadataPath, "utf-8");
    const metadata: SessionMetadata = JSON.parse(metadataContent);
    metadata.updated_at = new Date().toISOString();
    metadata.status = SessionStatus.Active;
    await fs.writeFile(metadataPath, JSON.stringify(metadata, null, 2));

    this.processManager.clearWaiting(sessionId);
  }

  /**
   * Delete a planning session.
   */
  async deleteSession(sessionId: string): Promise<void> {
    this.processManager.kill(sessionId);
    await fs.rm(path.join(this.sessionsDir, sessionId), { recursive: true, force: true });
  }

  /**
   * Resume a paused planning session.
   */
  async resumeSession(sessionId: string): Promise<void> {
    const sessionDir = path.join(this.sessionsDir, sessionId);

    try {
      await fs.access(sessionDir);
    } catch {
      throw new Error(`Session ${sessionId} not found`);
    }

    const metadataPath = path.join(sessionDir, "session.json");
    const metadataContent = await fs.readFile(metadataPath, "utf-8");
    const metadata: SessionMetadata = JSON.parse(metadataContent);

    metadata.status = SessionStatus.Active;
    metadata.updated_at = new Date().toISOString();
    await fs.writeFile(metadataPath, JSON.stringify(metadata, null, 2));

    if (!this.processManager.isRunning(sessionId)) {
      this.processManager.spawn(sessionId, metadata.prompt);
    }
  }

  /**
   * Stop a running planning session.
   */
  async stopSession(sessionId: string): Promise<void> {
    if (this.processManager.kill(sessionId)) {
      await this.updateSessionStatus(sessionId, SessionStatus.Paused);
    }
  }

  /**
   * Get artifact content for a specific session.
   */
  async getArtifact(
    sessionId: string,
    filename: string,
  ): Promise<{ content: string; filename: string }> {
    const artifactsDir = path.join(this.sessionsDir, sessionId, "artifacts");
    const artifactPath = path.join(artifactsDir, filename);

    // Security: ensure the artifact path is within the session's artifacts directory
    const normalizedPath = path.normalize(artifactPath);
    if (!normalizedPath.startsWith(artifactsDir)) {
      throw new Error("Invalid artifact path");
    }

    try {
      const content = await fs.readFile(artifactPath, "utf-8");
      return { content, filename };
    } catch (err) {
      console.warn(`[PlanningService] Could not read artifact ${filename}:`, err);
      throw new Error(`Artifact not found: ${filename}`);
    }
  }

  /**
   * Get the conversation file path for a session.
   */
  getConversationPath(sessionId: string): string {
    return path.join(this.sessionsDir, sessionId, "conversation.jsonl");
  }

  /**
   * Get the session directory path.
   */
  getSessionDir(sessionId: string): string {
    return path.join(this.sessionsDir, sessionId);
  }

  // ---------- internals ----------

  private async updateSessionStatus(
    sessionId: string,
    status: SessionStatus,
  ): Promise<void> {
    const metadataPath = path.join(this.sessionsDir, sessionId, "session.json");
    try {
      const content = await fs.readFile(metadataPath, "utf-8");
      const metadata: SessionMetadata = JSON.parse(content);
      metadata.status = status;
      metadata.updated_at = new Date().toISOString();
      await fs.writeFile(metadataPath, JSON.stringify(metadata, null, 2));
    } catch (err) {
      console.error(`[PlanningService:${sessionId}] Failed to update status:`, err);
    }
  }

  private async ensureSessionsDir(): Promise<void> {
    try {
      await fs.mkdir(this.sessionsDir, { recursive: true });
    } catch (err) {
      console.warn("[PlanningService] Failed to create sessions directory:", err);
    }
  }

  private generateSessionId(): string {
    const timestamp = new Date().toISOString().replace(/[-:.]/g, "").slice(0, 15);
    const random = uuidv4().slice(0, 8);
    return `${timestamp}-${random}`;
  }
}
