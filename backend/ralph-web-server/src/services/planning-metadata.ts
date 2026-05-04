/**
 * Low-level read/write helpers for planning-session metadata files
 * (session.json and conversation.jsonl). Extracted from PlanningService
 * so the service class can focus on orchestration instead of file layout.
 *
 * These helpers are stateless. Callers pass the absolute sessions directory
 * and a session id; each helper derives its own paths from those.
 */

import * as fs from "node:fs/promises";
import * as path from "node:path";

import {
  type ConversationEntry,
  type SessionMetadata,
  SessionStatus,
} from "./planning-types";

/**
 * Return the directory that holds a single session's files.
 */
export function sessionDirFor(sessionsDir: string, sessionId: string): string {
  return path.join(sessionsDir, sessionId);
}

/**
 * Return the path to a session's session.json metadata file.
 */
export function metadataPathFor(sessionsDir: string, sessionId: string): string {
  return path.join(sessionsDir, sessionId, "session.json");
}

/**
 * Return the path to a session's conversation.jsonl file.
 */
export function conversationPathFor(sessionsDir: string, sessionId: string): string {
  return path.join(sessionsDir, sessionId, "conversation.jsonl");
}

/**
 * Read and parse a session's metadata file.
 */
export async function readSessionMetadata(
  sessionsDir: string,
  sessionId: string,
): Promise<SessionMetadata> {
  const content = await fs.readFile(metadataPathFor(sessionsDir, sessionId), "utf-8");
  return JSON.parse(content) as SessionMetadata;
}

/**
 * Serialize and write a session's metadata file.
 */
export async function writeSessionMetadata(
  sessionsDir: string,
  sessionId: string,
  metadata: SessionMetadata,
): Promise<void> {
  await fs.writeFile(
    metadataPathFor(sessionsDir, sessionId),
    JSON.stringify(metadata, null, 2),
  );
}

/**
 * Update a session's status and bump updated_at. Reads the current metadata,
 * applies the change, and writes it back. Logs (rather than throws) on I/O
 * failure to match the original PlanningService semantics, where status
 * transitions are best-effort.
 */
export async function updateSessionStatus(
  sessionsDir: string,
  sessionId: string,
  status: SessionStatus,
): Promise<void> {
  try {
    const metadata = await readSessionMetadata(sessionsDir, sessionId);
    metadata.status = status;
    metadata.updated_at = new Date().toISOString();
    await writeSessionMetadata(sessionsDir, sessionId, metadata);
  } catch (err) {
    console.error(`[PlanningService:${sessionId}] Failed to update status:`, err);
  }
}

/**
 * Append a conversation entry to the session's conversation.jsonl file.
 */
export async function appendConversationEntry(
  sessionsDir: string,
  sessionId: string,
  entry: ConversationEntry,
): Promise<void> {
  await fs.appendFile(
    conversationPathFor(sessionsDir, sessionId),
    JSON.stringify(entry) + "\n",
  );
}

/**
 * Count non-empty lines in a session's conversation.jsonl file.
 * Returns 0 and logs a warning if the file can't be read.
 */
export async function countConversationMessages(
  sessionsDir: string,
  sessionId: string,
): Promise<number> {
  const conversationPath = conversationPathFor(sessionsDir, sessionId);
  try {
    const convContent = await fs.readFile(conversationPath, "utf-8");
    return convContent.trim().split("\n").filter((l: string) => l.trim()).length;
  } catch (err) {
    // Expected if conversation file doesn't exist yet
    console.warn(
      `[PlanningService] Could not read conversation file for session ${sessionId}:`,
      err,
    );
    return 0;
  }
}

/**
 * Read and parse a session's conversation.jsonl file as backend entries.
 * Returns [] (and logs) if the file can't be read.
 */
export async function readConversationEntries(
  sessionsDir: string,
  sessionId: string,
): Promise<ConversationEntry[]> {
  const conversationPath = conversationPathFor(sessionsDir, sessionId);
  try {
    const conversationContent = await fs.readFile(conversationPath, "utf-8");
    const lines = conversationContent.trim().split("\n");
    return lines
      .filter((line: string) => line.trim().length > 0)
      .map((line: string) => JSON.parse(line) as ConversationEntry);
  } catch (err) {
    console.warn(`[PlanningService:${sessionId}] Could not load conversation file:`, err);
    return [];
  }
}
