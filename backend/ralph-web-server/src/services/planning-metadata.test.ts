/**
 * Unit tests for the session metadata I/O helpers. These cover the
 * round-trip of session.json and conversation.jsonl so regressions in
 * the extracted helpers are caught without spinning up the full service.
 */

import { describe, it, before, after, beforeEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import {
  appendConversationEntry,
  conversationPathFor,
  countConversationMessages,
  metadataPathFor,
  readConversationEntries,
  readSessionMetadata,
  sessionDirFor,
  updateSessionStatus,
  writeSessionMetadata,
} from "./planning-metadata";
import {
  type ConversationEntry,
  type SessionMetadata,
  SessionStatus,
} from "./planning-types";

async function makeSessionsDir(): Promise<{ sessionsDir: string; cleanup: () => Promise<void> }> {
  const sessionsDir = await fs.mkdtemp(path.join(os.tmpdir(), "planning-metadata-test-"));
  return {
    sessionsDir,
    cleanup: async () => {
      await fs.rm(sessionsDir, { recursive: true, force: true });
    },
  };
}

async function seedMetadata(
  sessionsDir: string,
  sessionId: string,
  overrides: Partial<SessionMetadata> = {},
): Promise<SessionMetadata> {
  await fs.mkdir(sessionDirFor(sessionsDir, sessionId), { recursive: true });
  const now = new Date().toISOString();
  const metadata: SessionMetadata = {
    id: sessionId,
    prompt: "seed prompt",
    status: SessionStatus.Active,
    created_at: now,
    updated_at: now,
    iterations: 0,
    ...overrides,
  };
  await writeSessionMetadata(sessionsDir, sessionId, metadata);
  return metadata;
}

describe("planning-metadata", () => {
  let sessionsDir: string;
  let cleanup: () => Promise<void>;

  beforeEach(async () => {
    if (cleanup) {
      await cleanup();
    }
    const ws = await makeSessionsDir();
    sessionsDir = ws.sessionsDir;
    cleanup = ws.cleanup;
  });

  after(async () => {
    if (cleanup) {
      await cleanup();
    }
  });

  describe("path helpers", () => {
    it("returns conventional paths for session files", () => {
      assert.equal(
        sessionDirFor("/tmp/root", "abc"),
        path.join("/tmp/root", "abc"),
      );
      assert.equal(
        metadataPathFor("/tmp/root", "abc"),
        path.join("/tmp/root", "abc", "session.json"),
      );
      assert.equal(
        conversationPathFor("/tmp/root", "abc"),
        path.join("/tmp/root", "abc", "conversation.jsonl"),
      );
    });
  });

  describe("session metadata round-trip", () => {
    it("writes and reads back the same metadata", async () => {
      const sessionId = "abc";
      const original = await seedMetadata(sessionsDir, sessionId, {
        prompt: "hello",
        iterations: 3,
      });

      const loaded = await readSessionMetadata(sessionsDir, sessionId);
      assert.deepEqual(loaded, original);
    });

    it("updateSessionStatus mutates status and refreshes updated_at", async () => {
      const sessionId = "abc";
      const seeded = await seedMetadata(sessionsDir, sessionId, {
        updated_at: "2020-01-01T00:00:00.000Z",
      });

      await updateSessionStatus(sessionsDir, sessionId, SessionStatus.Completed);

      const loaded = await readSessionMetadata(sessionsDir, sessionId);
      assert.equal(loaded.status, SessionStatus.Completed);
      assert.notEqual(loaded.updated_at, seeded.updated_at);
      // The rest of the metadata is preserved.
      assert.equal(loaded.id, seeded.id);
      assert.equal(loaded.prompt, seeded.prompt);
      assert.equal(loaded.created_at, seeded.created_at);
    });

    it("updateSessionStatus logs and swallows I/O errors", async () => {
      // Capture console.error so the test output isn't polluted and we can
      // assert the helper does not rethrow.
      const originalError = console.error;
      let captured: unknown = null;
      console.error = (..._args: unknown[]) => {
        captured = _args;
      };
      try {
        await assert.doesNotReject(
          updateSessionStatus(sessionsDir, "missing", SessionStatus.Completed),
        );
        assert.ok(captured !== null, "expected an error to be logged");
      } finally {
        console.error = originalError;
      }
    });
  });

  describe("conversation helpers", () => {
    it("appendConversationEntry + readConversationEntries round-trips", async () => {
      const sessionId = "abc";
      await seedMetadata(sessionsDir, sessionId);

      const entries: ConversationEntry[] = [
        { type: "user_prompt", id: "q1", text: "first?", ts: "2026-05-04T07:00:00Z" },
        { type: "user_response", id: "q1", text: "yes", ts: "2026-05-04T07:00:05Z" },
      ];

      for (const entry of entries) {
        await appendConversationEntry(sessionsDir, sessionId, entry);
      }

      const loaded = await readConversationEntries(sessionsDir, sessionId);
      assert.deepEqual(loaded, entries);
    });

    it("countConversationMessages counts non-empty lines and returns 0 when missing", async () => {
      const sessionId = "abc";
      await seedMetadata(sessionsDir, sessionId);

      // No conversation file yet — must return 0 without throwing.
      assert.equal(await countConversationMessages(sessionsDir, sessionId), 0);

      await appendConversationEntry(sessionsDir, sessionId, {
        type: "user_prompt",
        id: "q1",
        text: "hi",
        ts: "2026-05-04T07:00:00Z",
      });
      await appendConversationEntry(sessionsDir, sessionId, {
        type: "user_response",
        id: "q1",
        text: "hey",
        ts: "2026-05-04T07:00:05Z",
      });

      assert.equal(await countConversationMessages(sessionsDir, sessionId), 2);
    });

    it("readConversationEntries returns [] when file does not exist", async () => {
      const sessionId = "abc";
      await seedMetadata(sessionsDir, sessionId);

      assert.deepEqual(await readConversationEntries(sessionsDir, sessionId), []);
    });
  });
});
