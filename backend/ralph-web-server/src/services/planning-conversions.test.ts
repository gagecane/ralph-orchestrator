/**
 * Unit tests for the pure conversion helpers. These assert the
 * backend/frontend data contracts that PlanningService relies on, without
 * spawning subprocesses or touching the filesystem.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  toFrontendEntry,
  toFrontendStatus,
  generateTitle,
} from "./planning-conversions";
import {
  type ConversationEntry,
  SessionStatus,
} from "./planning-types";

describe("planning-conversions", () => {
  describe("toFrontendEntry", () => {
    it("maps user_prompt to prompt and renames text/ts fields", () => {
      const entry: ConversationEntry = {
        type: "user_prompt",
        id: "q1",
        text: "What should we do?",
        ts: "2026-05-04T07:00:00.000Z",
      };

      assert.deepEqual(toFrontendEntry(entry), {
        type: "prompt",
        id: "q1",
        content: "What should we do?",
        timestamp: "2026-05-04T07:00:00.000Z",
      });
    });

    it("maps user_response to response and renames text/ts fields", () => {
      const entry: ConversationEntry = {
        type: "user_response",
        id: "q1",
        text: "Ship it.",
        ts: "2026-05-04T07:01:00.000Z",
      };

      assert.deepEqual(toFrontendEntry(entry), {
        type: "response",
        id: "q1",
        content: "Ship it.",
        timestamp: "2026-05-04T07:01:00.000Z",
      });
    });
  });

  describe("toFrontendStatus", () => {
    it("maps waiting_for_input to paused for the frontend", () => {
      assert.equal(toFrontendStatus(SessionStatus.WaitingForInput), "paused");
    });

    it("maps timed_out to failed because the frontend has no timeout state", () => {
      assert.equal(toFrontendStatus(SessionStatus.TimedOut), "failed");
    });

    it("passes through other statuses unchanged", () => {
      assert.equal(toFrontendStatus(SessionStatus.Active), "active");
      assert.equal(toFrontendStatus(SessionStatus.Completed), "completed");
      assert.equal(toFrontendStatus(SessionStatus.Failed), "failed");
      assert.equal(toFrontendStatus(SessionStatus.Paused), "paused");
    });
  });

  describe("generateTitle", () => {
    it("returns the trimmed prompt when it fits within 60 chars", () => {
      assert.equal(generateTitle("  hello world  "), "hello world");
    });

    it("returns at most 60 chars (57 + ellipsis) for long prompts", () => {
      const prompt = "a".repeat(100);
      const title = generateTitle(prompt);
      assert.equal(title.length, 60);
      assert.ok(title.endsWith("..."), `expected ellipsis, got: ${title}`);
      assert.equal(title.slice(0, 57), "a".repeat(57));
    });

    it("handles the 60-char boundary without truncation", () => {
      const prompt = "a".repeat(60);
      assert.equal(generateTitle(prompt), prompt);
    });

    it("truncates at 61 chars", () => {
      const prompt = "a".repeat(61);
      const title = generateTitle(prompt);
      assert.equal(title.length, 60);
      assert.ok(title.endsWith("..."));
    });
  });
});
