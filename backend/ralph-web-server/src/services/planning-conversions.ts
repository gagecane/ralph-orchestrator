/**
 * Conversions between backend and frontend representations of planning
 * sessions. These helpers are pure and have no dependency on the rest of
 * the service — extracted to keep PlanningService.ts focused on behavior.
 */

import {
  ConversationEntry,
  FrontendConversationEntry,
  SessionStatus,
} from "./planning-types";

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
 */
export function toFrontendStatus(status: SessionStatus): string {
  // Map waiting_for_input to paused for the frontend
  if (status === SessionStatus.WaitingForInput) {
    return "paused";
  }
  // The frontend has no dedicated "timed_out" state — treat it as a failure
  // so the existing failure UI handles it without a new code path.
  if (status === SessionStatus.TimedOut) {
    return "failed";
  }
  return status;
}

/**
 * Generate a title from the prompt.
 */
export function generateTitle(prompt: string): string {
  const trimmed = prompt.trim();
  if (trimmed.length <= 60) {
    return trimmed;
  }
  return trimmed.substring(0, 57) + "...";
}
