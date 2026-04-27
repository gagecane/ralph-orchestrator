/**
 * Shared types for the planning service modules.
 */

/**
 * Status of a planning session.
 */
export enum SessionStatus {
  Active = "active",
  WaitingForInput = "waiting_for_input",
  Completed = "completed",
  TimedOut = "timed_out",
  Failed = "failed",
  Paused = "paused",
}

/**
 * Session metadata from the session.json file.
 */
export interface SessionMetadata {
  id: string;
  prompt: string;
  status: SessionStatus;
  created_at: string;
  updated_at: string;
  iterations: number;
  config?: string;
}

/**
 * A single entry in the planning conversation (backend format).
 */
export interface ConversationEntry {
  type: "user_prompt" | "user_response";
  id: string;
  text: string;
  ts: string;
}

/**
 * Frontend-compatible conversation entry format.
 */
export interface FrontendConversationEntry {
  type: "prompt" | "response";
  id: string;
  content: string;
  timestamp: string;
}

/**
 * Full session details with conversation history (frontend format).
 */
export interface PlanningSessionDetail {
  id: string;
  prompt: string;
  status: string;
  title?: string;
  createdAt: string;
  updatedAt: string;
  completedAt?: string;
  conversation: FrontendConversationEntry[];
  artifacts?: string[];
  messageCount?: number;
}

/**
 * Summary info for session lists (frontend format).
 */
export interface PlanningSessionSummary {
  id: string;
  title?: string;
  prompt: string;
  status: string;
  createdAt: string;
  updatedAt: string;
  messageCount?: number;
  iterations?: number;
}

/**
 * Configuration options for the PlanningService.
 */
export interface PlanningServiceOptions {
  /** Root directory of the Ralph project */
  workspaceRoot: string;
  /** Path to ralph binary (default: "ralph") */
  ralphPath?: string;
  /** Default timeout for user responses (seconds, default: 300) */
  defaultTimeoutSeconds?: number;
}

/**
 * A Ralph event from the events JSONL file.
 */
export interface RalphEvent {
  topic: string;
  payload: unknown;
  ts: string;
}

/**
 * Payload for the user.prompt event.
 */
export interface UserPromptPayload {
  id: string;
  question: string;
}
