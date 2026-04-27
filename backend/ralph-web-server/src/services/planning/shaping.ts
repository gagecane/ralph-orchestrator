/**
 * Pure helpers for shaping planning data between backend and frontend formats.
 */

import type { ConversationEntry, FrontendConversationEntry } from "./types";
import { SessionStatus } from "./types";

/**
 * Convert backend conversation entry to frontend format.
 */
export function toFrontendEntry(entry: ConversationEntry): FrontendConversationEntry {
  return {
    type: entry.type === "user_prompt" ? "prompt" : "response",
    id: entry.id,
    content: entry.text,
    timestamp: entry.ts,
  };
}

/**
 * Convert backend status to frontend status string.
 * Maps waiting_for_input to paused for the frontend.
 */
export function toFrontendStatus(status: SessionStatus): string {
  if (status === SessionStatus.WaitingForInput) {
    return "paused";
  }
  return status;
}

/**
 * Generate a title from a prompt.
 */
export function generateTitle(prompt: string): string {
  const trimmed = prompt.trim();
  if (trimmed.length <= 60) {
    return trimmed;
  }
  return trimmed.substring(0, 57) + "...";
}
