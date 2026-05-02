/**
 * PlanningService tests
 *
 * Covers session lifecycle (create, list, get, delete, resume, stop), response
 * submission, artifact access with path-traversal protection, status
 * mapping, and request timeout behavior. Tests avoid spawning a real `ralph`
 * binary by pointing `ralphPath` at `/bin/true` (immediate success) or
 * `/bin/sleep` (forces the timeout watchdog to fire).
 */

import { describe, it, before, after, beforeEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as fsSync from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import {
  PlanningService,
  SessionStatus,
  type SessionMetadata,
  type ConversationEntry,
} from "./PlanningService";

/**
 * Create an isolated temp workspace root with the .ralph layout.
 * Returns the workspaceRoot and a cleanup function.
 */
async function makeWorkspace(): Promise<{ workspaceRoot: string; cleanup: () => Promise<void> }> {
  const workspaceRoot = await fs.mkdtemp(path.join(os.tmpdir(), "planning-service-test-"));
  await fs.mkdir(path.join(workspaceRoot, ".ralph"), { recursive: true });
  return {
    workspaceRoot,
    cleanup: async () => {
      await fs.rm(workspaceRoot, { recursive: true, force: true });
    },
  };
}

/**
 * Build a PlanningService wired against the given workspace with a no-op
 * ralph binary. `/bin/true` is available on all POSIX systems and simply
 * exits with code 0 — perfect for avoiding real subprocess behavior in tests.
 */
function makeService(workspaceRoot: string): PlanningService {
  return new PlanningService({
    workspaceRoot,
    ralphPath: "/bin/true",
    defaultTimeoutSeconds: 1,
  });
}

/**
 * Seed a planning session on disk without going through startSession().
 */
async function seedSession(
  workspaceRoot: string,
  sessionId: string,
  overrides: Partial<SessionMetadata> = {},
  conversation: ConversationEntry[] = []
): Promise<void> {
  const sessionDir = path.join(workspaceRoot, ".ralph", "planning-sessions", sessionId);
  await fs.mkdir(path.join(sessionDir, "artifacts"), { recursive: true });

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
  await fs.writeFile(
    path.join(sessionDir, "session.json"),
    JSON.stringify(metadata, null, 2),
  );

  const conversationPath = path.join(sessionDir, "conversation.jsonl");
  const body = conversation.map((entry) => JSON.stringify(entry)).join("\n");
  await fs.writeFile(conversationPath, body + (body.length > 0 ? "\n" : ""));
}

describe("PlanningService", () => {
  let workspaceRoot: string;
  let cleanup: () => Promise<void>;
  let service: PlanningService;

  beforeEach(async () => {
    if (cleanup) {
      await cleanup();
    }
    const ws = await makeWorkspace();
    workspaceRoot = ws.workspaceRoot;
    cleanup = ws.cleanup;
    service = makeService(workspaceRoot);
  });

  after(async () => {
    if (cleanup) {
      await cleanup();
    }
  });

  describe("listSessions", () => {
    it("returns an empty array when no sessions exist", async () => {
      const sessions = await service.listSessions();
      assert.deepEqual(sessions, []);
    });

    it("returns session summaries sorted by updated_at descending", async () => {
      await seedSession(workspaceRoot, "older", {
        prompt: "old prompt",
        updated_at: "2024-01-01T00:00:00Z",
        created_at: "2024-01-01T00:00:00Z",
      });
      await seedSession(workspaceRoot, "newer", {
        prompt: "new prompt",
        updated_at: "2024-06-01T00:00:00Z",
        created_at: "2024-06-01T00:00:00Z",
      });

      const sessions = await service.listSessions();
      assert.equal(sessions.length, 2);
      assert.equal(sessions[0].id, "newer");
      assert.equal(sessions[1].id, "older");
      assert.equal(sessions[0].prompt, "new prompt");
    });

    it("derives title from prompt — short prompt is returned verbatim", async () => {
      await seedSession(workspaceRoot, "s", { prompt: "Short prompt" });
      const [summary] = await service.listSessions();
      assert.equal(summary.title, "Short prompt");
    });

    it("derives title from prompt — long prompt is truncated with ellipsis at 60 chars", async () => {
      const longPrompt = "a".repeat(100);
      await seedSession(workspaceRoot, "s", { prompt: longPrompt });
      const [summary] = await service.listSessions();
      assert.equal(summary.title?.length, 60);
      assert.ok(summary.title?.endsWith("..."));
    });

    it("counts messages from conversation.jsonl", async () => {
      const entries: ConversationEntry[] = [
        { type: "user_prompt", id: "q1", text: "Q1", ts: "2024-01-01T00:00:00Z" },
        { type: "user_response", id: "q1", text: "A1", ts: "2024-01-01T00:00:01Z" },
        { type: "user_prompt", id: "q2", text: "Q2", ts: "2024-01-01T00:00:02Z" },
      ];
      await seedSession(workspaceRoot, "with-msgs", {}, entries);
      const [summary] = await service.listSessions();
      assert.equal(summary.messageCount, 3);
    });

    it("skips directories with invalid session.json", async () => {
      await seedSession(workspaceRoot, "valid");
      const brokenDir = path.join(workspaceRoot, ".ralph", "planning-sessions", "broken");
      await fs.mkdir(brokenDir, { recursive: true });
      await fs.writeFile(path.join(brokenDir, "session.json"), "not json");

      const sessions = await service.listSessions();
      assert.equal(sessions.length, 1);
      assert.equal(sessions[0].id, "valid");
    });

    it("maps waiting_for_input status to 'paused' for the frontend", async () => {
      await seedSession(workspaceRoot, "paused", {
        status: SessionStatus.WaitingForInput,
      });
      const [summary] = await service.listSessions();
      assert.equal(summary.status, "paused");
    });

    it("returns non-waiting statuses unchanged", async () => {
      await seedSession(workspaceRoot, "completed", {
        status: SessionStatus.Completed,
      });
      const [summary] = await service.listSessions();
      assert.equal(summary.status, "completed");
    });
  });

  describe("getSession", () => {
    it("returns full session detail with conversation converted to frontend format", async () => {
      const entries: ConversationEntry[] = [
        { type: "user_prompt", id: "q1", text: "Question", ts: "2024-01-01T00:00:00Z" },
        { type: "user_response", id: "q1", text: "Answer", ts: "2024-01-01T00:00:01Z" },
      ];
      await seedSession(
        workspaceRoot,
        "s1",
        { prompt: "hello", status: SessionStatus.Active },
        entries,
      );

      const detail = await service.getSession("s1");
      assert.equal(detail.id, "s1");
      assert.equal(detail.prompt, "hello");
      assert.equal(detail.title, "hello");
      assert.equal(detail.status, "active");
      assert.equal(detail.messageCount, 2);
      assert.deepEqual(detail.conversation, [
        { type: "prompt", id: "q1", content: "Question", timestamp: "2024-01-01T00:00:00Z" },
        { type: "response", id: "q1", content: "Answer", timestamp: "2024-01-01T00:00:01Z" },
      ]);
      assert.deepEqual(detail.artifacts, []);
      assert.equal(detail.completedAt, undefined);
    });

    it("sets completedAt when status is completed", async () => {
      await seedSession(workspaceRoot, "done", {
        status: SessionStatus.Completed,
        updated_at: "2024-01-01T12:00:00Z",
      });
      const detail = await service.getSession("done");
      assert.equal(detail.completedAt, "2024-01-01T12:00:00Z");
    });

    it("leaves completedAt undefined when status is not completed", async () => {
      await seedSession(workspaceRoot, "active", {
        status: SessionStatus.Active,
      });
      const detail = await service.getSession("active");
      assert.equal(detail.completedAt, undefined);
    });

    it("includes artifacts (excluding dotfiles)", async () => {
      await seedSession(workspaceRoot, "with-artifacts");
      const artifactsDir = path.join(
        workspaceRoot,
        ".ralph",
        "planning-sessions",
        "with-artifacts",
        "artifacts",
      );
      await fs.writeFile(path.join(artifactsDir, "plan.md"), "# Plan");
      await fs.writeFile(path.join(artifactsDir, "notes.txt"), "notes");
      await fs.writeFile(path.join(artifactsDir, ".hidden"), "hidden");

      const detail = await service.getSession("with-artifacts");
      assert.ok(detail.artifacts);
      assert.deepEqual(detail.artifacts!.sort(), ["notes.txt", "plan.md"]);
    });

    it("throws when session does not exist", async () => {
      await assert.rejects(() => service.getSession("nope"));
    });

    it("handles empty conversation file gracefully", async () => {
      await seedSession(workspaceRoot, "empty-conv", {});
      const detail = await service.getSession("empty-conv");
      assert.deepEqual(detail.conversation, []);
      assert.equal(detail.messageCount, 0);
    });
  });

  describe("startSession", () => {
    it("creates a session directory, metadata, and conversation file", async () => {
      const { sessionId } = await service.startSession("my first prompt");
      assert.ok(sessionId, "session id should be generated");
      assert.match(sessionId, /^\d{8}T\d{6}-[0-9a-f]+$/);

      const sessionDir = path.join(workspaceRoot, ".ralph", "planning-sessions", sessionId);
      assert.ok(fsSync.existsSync(sessionDir));
      assert.ok(fsSync.existsSync(path.join(sessionDir, "session.json")));
      assert.ok(fsSync.existsSync(path.join(sessionDir, "conversation.jsonl")));
      assert.ok(fsSync.existsSync(path.join(sessionDir, "artifacts")));

      const metadataRaw = await fs.readFile(path.join(sessionDir, "session.json"), "utf-8");
      const metadata: SessionMetadata = JSON.parse(metadataRaw);
      assert.equal(metadata.prompt, "my first prompt");
      assert.equal(metadata.status, SessionStatus.Active);
      assert.equal(metadata.iterations, 0);
    });

    it("generates unique session IDs for concurrent starts", async () => {
      const [a, b, c] = await Promise.all([
        service.startSession("a"),
        service.startSession("b"),
        service.startSession("c"),
      ]);
      assert.notEqual(a.sessionId, b.sessionId);
      assert.notEqual(b.sessionId, c.sessionId);
      assert.notEqual(a.sessionId, c.sessionId);
    });

    it("creates conversation.jsonl as an empty file", async () => {
      const { sessionId } = await service.startSession("empty");
      const conversationPath = service.getConversationPath(sessionId);
      const body = await fs.readFile(conversationPath, "utf-8");
      assert.equal(body, "");
    });
  });

  describe("submitResponse", () => {
    it("appends a user_response entry and resets status to active", async () => {
      await seedSession(workspaceRoot, "s1", {
        status: SessionStatus.WaitingForInput,
      }, [
        { type: "user_prompt", id: "q1", text: "What?", ts: "2024-01-01T00:00:00Z" },
      ]);

      await service.submitResponse("s1", "q1", "Because.");

      const conversationPath = service.getConversationPath("s1");
      const body = await fs.readFile(conversationPath, "utf-8");
      const lines = body.trim().split("\n");
      assert.equal(lines.length, 2);

      const responseEntry: ConversationEntry = JSON.parse(lines[1]);
      assert.equal(responseEntry.type, "user_response");
      assert.equal(responseEntry.id, "q1");
      assert.equal(responseEntry.text, "Because.");
      assert.ok(responseEntry.ts);

      const metadataRaw = await fs.readFile(
        path.join(service.getSessionDir("s1"), "session.json"),
        "utf-8",
      );
      const metadata: SessionMetadata = JSON.parse(metadataRaw);
      assert.equal(metadata.status, SessionStatus.Active);
    });

    it("rejects when session does not exist", async () => {
      await assert.rejects(() => service.submitResponse("nope", "q1", "hi"));
    });
  });

  describe("deleteSession", () => {
    it("removes the session directory", async () => {
      await seedSession(workspaceRoot, "gone");
      const sessionDir = service.getSessionDir("gone");
      assert.ok(fsSync.existsSync(sessionDir));

      await service.deleteSession("gone");
      assert.equal(fsSync.existsSync(sessionDir), false);
    });

    it("is a no-op when session does not exist (fs.rm with force)", async () => {
      await service.deleteSession("never-existed");
      // no throw = pass
    });
  });

  describe("stopSession", () => {
    it("is a no-op when no process is running for the session", async () => {
      await seedSession(workspaceRoot, "idle", { status: SessionStatus.Active });
      await service.stopSession("idle");
      // no throw = pass; status unchanged because there's nothing to stop
      const detail = await service.getSession("idle");
      assert.equal(detail.status, "active");
    });
  });

  describe("getArtifact", () => {
    it("returns artifact content for an existing file", async () => {
      await seedSession(workspaceRoot, "s");
      const artifactsDir = path.join(service.getSessionDir("s"), "artifacts");
      await fs.writeFile(path.join(artifactsDir, "plan.md"), "# My Plan\n");

      const { content, filename } = await service.getArtifact("s", "plan.md");
      assert.equal(filename, "plan.md");
      assert.equal(content, "# My Plan\n");
    });

    it("rejects filenames that try to escape the artifacts directory", async () => {
      await seedSession(workspaceRoot, "s");
      await assert.rejects(
        () => service.getArtifact("s", "../../etc/passwd"),
        /Invalid artifact path/,
      );
    });

    it("rejects when artifact does not exist", async () => {
      await seedSession(workspaceRoot, "s");
      await assert.rejects(
        () => service.getArtifact("s", "missing.txt"),
        /Artifact not found: missing\.txt/,
      );
    });
  });

  describe("path helpers", () => {
    it("getConversationPath returns path under the session directory", () => {
      const p = service.getConversationPath("sX");
      assert.ok(p.endsWith("planning-sessions/sX/conversation.jsonl"));
    });

    it("getSessionDir returns path under the planning-sessions directory", () => {
      const p = service.getSessionDir("sX");
      assert.ok(p.endsWith("planning-sessions/sX"));
    });
  });

  describe("resumeSession", () => {
    it("updates status to active when session exists", async () => {
      await seedSession(workspaceRoot, "paused", {
        status: SessionStatus.Paused,
      });

      await service.resumeSession("paused");
      const detail = await service.getSession("paused");
      assert.equal(detail.status, "active");
    });

    it("throws when session does not exist", async () => {
      await assert.rejects(
        () => service.resumeSession("never"),
        /Session never not found/,
      );
    });
  });

  describe("defaultTimeoutSeconds (request timeout)", () => {
    /**
     * Wait until a metadata file reports the expected status, or fail the
     * test if the deadline passes. This avoids coupling tests to specific
     * sleep durations — we just poll until the watchdog + exit handler
     * have observably flipped the file on disk.
     */
    async function waitForStatus(
      sessionDir: string,
      want: SessionStatus,
      timeoutMs: number,
    ): Promise<SessionMetadata> {
      const metadataPath = path.join(sessionDir, "session.json");
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        try {
          const raw = await fs.readFile(metadataPath, "utf-8");
          const meta: SessionMetadata = JSON.parse(raw);
          if (meta.status === want) {
            return meta;
          }
        } catch {
          // metadata might be mid-write; retry
        }
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      const raw = await fs.readFile(metadataPath, "utf-8");
      const meta: SessionMetadata = JSON.parse(raw);
      throw new Error(
        `Timed out waiting for status=${want}; last seen status=${meta.status}`,
      );
    }

    it("kills a ralph process that exceeds defaultTimeoutSeconds and records timed_out", async () => {
      // The spawn args are baked into spawnRalphForSession (`run -c ...`), so
      // /bin/sleep would reject them and exit with code 1 before the
      // watchdog ever fires. Drop a tiny shell script that ignores its
      // arguments and sleeps long enough for the 100ms watchdog to trip.
      const sleeperPath = path.join(workspaceRoot, "slow-ralph.sh");
      await fs.writeFile(
        sleeperPath,
        "#!/bin/sh\nexec sleep 10\n",
        { mode: 0o755 },
      );

      const slowService = new PlanningService({
        workspaceRoot,
        ralphPath: sleeperPath,
        defaultTimeoutSeconds: 0.1,
      });

      const { sessionId } = await slowService.startSession("hang please");
      const sessionDir = slowService.getSessionDir(sessionId);

      // Wait for the watchdog → SIGTERM → exit → status write chain.
      // 2s is plenty on any non-pathologically-slow machine.
      const meta = await waitForStatus(sessionDir, SessionStatus.TimedOut, 2000);
      assert.equal(meta.status, SessionStatus.TimedOut);
    });

    it("exposes timed_out backend status as 'failed' to the frontend", async () => {
      await seedSession(workspaceRoot, "timed", {
        status: SessionStatus.TimedOut,
      });
      const [summary] = await service.listSessions();
      assert.equal(summary.status, "failed");

      const detail = await service.getSession("timed");
      assert.equal(detail.status, "failed");
      // `completedAt` is only populated for Completed sessions, not timeouts.
      assert.equal(detail.completedAt, undefined);
    });

    it("does not record timed_out when ralph exits on its own before the timeout", async () => {
      // /bin/true exits ~immediately; the 5s watchdog will never fire. We
      // want the session to end up as `completed`, not `timed_out`.
      const fastService = new PlanningService({
        workspaceRoot,
        ralphPath: "/bin/true",
        defaultTimeoutSeconds: 5,
      });

      const { sessionId } = await fastService.startSession("fast");
      const sessionDir = fastService.getSessionDir(sessionId);

      const meta = await waitForStatus(sessionDir, SessionStatus.Completed, 2000);
      assert.equal(meta.status, SessionStatus.Completed);
    });

    it("disables the watchdog when defaultTimeoutSeconds <= 0", async () => {
      // A non-positive timeout means "no watchdog". /bin/true still
      // completes immediately, so we should see `completed` with no
      // spurious timeout interference.
      const noTimeoutService = new PlanningService({
        workspaceRoot,
        ralphPath: "/bin/true",
        defaultTimeoutSeconds: 0,
      });

      const { sessionId } = await noTimeoutService.startSession("no watchdog");
      const sessionDir = noTimeoutService.getSessionDir(sessionId);

      const meta = await waitForStatus(sessionDir, SessionStatus.Completed, 2000);
      assert.equal(meta.status, SessionStatus.Completed);
    });
  });
});
