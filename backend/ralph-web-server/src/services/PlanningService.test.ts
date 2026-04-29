/**
 * PlanningService Tests
 *
 * Tests for the planning session lifecycle management layer. The service owns
 * the on-disk layout for sessions (session.json, conversation.jsonl, artifacts/)
 * and delegates child-process lifecycle to a RalphProcessManager. These tests
 * exercise the file-system behaviors directly and stub the process manager so
 * we do not spawn real ralph child processes.
 */

import { test, describe, beforeEach, afterEach } from "node:test";
import assert from "node:assert";
import * as path from "node:path";
import * as fs from "node:fs";
import * as fsp from "node:fs/promises";
import * as os from "node:os";

import { PlanningService } from "./PlanningService";
import { SessionStatus, type SessionMetadata } from "./planning/types";

/**
 * Replace the private processManager on a PlanningService with a no-op stub
 * so tests never spawn a real ralph child process. Returns the stub so tests
 * can assert on its interactions.
 */
interface ProcessManagerStub {
  spawn: (id: string, prompt: string) => void;
  kill: (id: string) => boolean;
  isRunning: (id: string) => boolean;
  clearWaiting: (id: string) => void;
  spawnCalls: Array<{ id: string; prompt: string }>;
  killCalls: string[];
  clearWaitingCalls: string[];
  running: Set<string>;
}

function stubProcessManager(service: PlanningService): ProcessManagerStub {
  const stub: ProcessManagerStub = {
    spawnCalls: [],
    killCalls: [],
    clearWaitingCalls: [],
    running: new Set<string>(),
    spawn(id, prompt) {
      this.spawnCalls.push({ id, prompt });
      this.running.add(id);
    },
    kill(id) {
      this.killCalls.push(id);
      const wasRunning = this.running.has(id);
      this.running.delete(id);
      return wasRunning;
    },
    isRunning(id) {
      return this.running.has(id);
    },
    clearWaiting(id) {
      this.clearWaitingCalls.push(id);
    },
  };
  (service as unknown as { processManager: ProcessManagerStub }).processManager = stub;
  return stub;
}

/**
 * Read a session's on-disk metadata.
 */
async function readSessionMetadata(
  workspaceRoot: string,
  sessionId: string,
): Promise<SessionMetadata> {
  const metadataPath = path.join(
    workspaceRoot,
    ".ralph",
    "planning-sessions",
    sessionId,
    "session.json",
  );
  const content = await fsp.readFile(metadataPath, "utf-8");
  return JSON.parse(content) as SessionMetadata;
}

describe("PlanningService", () => {
  let workspaceRoot: string;
  let sessionsDir: string;
  let service: PlanningService;
  let processStub: ProcessManagerStub;

  beforeEach(() => {
    workspaceRoot = fs.mkdtempSync(path.join(os.tmpdir(), "planning-service-test-"));
    sessionsDir = path.join(workspaceRoot, ".ralph", "planning-sessions");
    service = new PlanningService({ workspaceRoot });
    processStub = stubProcessManager(service);
  });

  afterEach(() => {
    fs.rmSync(workspaceRoot, { recursive: true, force: true });
  });

  // ---------------------------------------------------------------------------
  // listSessions
  // ---------------------------------------------------------------------------

  describe("listSessions", () => {
    test("returns empty list when sessions directory is empty", async () => {
      const sessions = await service.listSessions();
      assert.deepStrictEqual(sessions, []);
    });

    test("creates sessions directory if it does not exist", async () => {
      assert.strictEqual(fs.existsSync(sessionsDir), false);
      await service.listSessions();
      assert.strictEqual(
        fs.existsSync(sessionsDir),
        true,
        "listSessions should create the sessions directory",
      );
    });

    test("skips non-directory entries", async () => {
      fs.mkdirSync(sessionsDir, { recursive: true });
      // Stray file in sessions dir should not be interpreted as a session.
      fs.writeFileSync(path.join(sessionsDir, "stray.txt"), "ignore me");
      const sessions = await service.listSessions();
      assert.deepStrictEqual(sessions, []);
    });

    test("returns summaries for sessions with valid metadata", async () => {
      const { sessionId } = await service.startSession("Plan a rocket launch");
      const sessions = await service.listSessions();
      assert.strictEqual(sessions.length, 1);
      assert.strictEqual(sessions[0].id, sessionId);
      assert.strictEqual(sessions[0].prompt, "Plan a rocket launch");
      assert.strictEqual(sessions[0].title, "Plan a rocket launch");
      assert.strictEqual(sessions[0].status, SessionStatus.Active);
      assert.strictEqual(sessions[0].messageCount, 0);
      assert.strictEqual(sessions[0].iterations, 0);
    });

    test("skips sessions with invalid metadata without throwing", async () => {
      const { sessionId: good } = await service.startSession("Good session");

      // Corrupt session directory: valid folder name but invalid JSON.
      const badDir = path.join(sessionsDir, "bad-session");
      fs.mkdirSync(badDir, { recursive: true });
      fs.writeFileSync(path.join(badDir, "session.json"), "{ not valid json ");

      const sessions = await service.listSessions();
      assert.strictEqual(sessions.length, 1, "Only valid session should be returned");
      assert.strictEqual(sessions[0].id, good);
    });

    test("sorts sessions by updatedAt descending", async () => {
      // Create three sessions with explicit, distinct updatedAt values.
      fs.mkdirSync(sessionsDir, { recursive: true });
      const ids = ["session-old", "session-mid", "session-new"];
      const times = [
        "2026-01-01T00:00:00.000Z",
        "2026-02-01T00:00:00.000Z",
        "2026-03-01T00:00:00.000Z",
      ];
      for (let i = 0; i < ids.length; i++) {
        const dir = path.join(sessionsDir, ids[i]);
        fs.mkdirSync(dir, { recursive: true });
        const meta: SessionMetadata = {
          id: ids[i],
          prompt: `prompt ${i}`,
          status: SessionStatus.Active,
          created_at: times[i],
          updated_at: times[i],
          iterations: i,
        };
        fs.writeFileSync(path.join(dir, "session.json"), JSON.stringify(meta));
        fs.writeFileSync(path.join(dir, "conversation.jsonl"), "");
      }

      const sessions = await service.listSessions();
      assert.deepStrictEqual(
        sessions.map((s) => s.id),
        ["session-new", "session-mid", "session-old"],
      );
    });

    test("counts messages from conversation.jsonl", async () => {
      const { sessionId } = await service.startSession("Count me");
      const convPath = path.join(sessionsDir, sessionId, "conversation.jsonl");
      fs.writeFileSync(
        convPath,
        [
          JSON.stringify({ type: "user_prompt", id: "q1", text: "A?", ts: "t1" }),
          JSON.stringify({ type: "user_response", id: "q1", text: "B", ts: "t2" }),
          "", // blank line should not count
          JSON.stringify({ type: "user_prompt", id: "q2", text: "C?", ts: "t3" }),
          "",
        ].join("\n"),
      );

      const sessions = await service.listSessions();
      assert.strictEqual(sessions.length, 1);
      assert.strictEqual(sessions[0].messageCount, 3);
    });

    test("treats missing conversation file as zero messages", async () => {
      const { sessionId } = await service.startSession("No conversation file");
      await fsp.unlink(path.join(sessionsDir, sessionId, "conversation.jsonl"));

      const sessions = await service.listSessions();
      assert.strictEqual(sessions.length, 1);
      assert.strictEqual(sessions[0].messageCount, 0);
    });

    test("maps waiting_for_input status to 'paused' for frontend", async () => {
      const { sessionId } = await service.startSession("Pauseable");
      // Directly update metadata to WaitingForInput.
      const metaPath = path.join(sessionsDir, sessionId, "session.json");
      const meta: SessionMetadata = JSON.parse(fs.readFileSync(metaPath, "utf-8"));
      meta.status = SessionStatus.WaitingForInput;
      fs.writeFileSync(metaPath, JSON.stringify(meta));

      const sessions = await service.listSessions();
      assert.strictEqual(sessions[0].status, "paused");
    });

    test("truncates long prompts to a 60-char title with ellipsis", async () => {
      const longPrompt = "a".repeat(100);
      await service.startSession(longPrompt);
      const sessions = await service.listSessions();
      assert.strictEqual(sessions.length, 1);
      assert.strictEqual((sessions[0].title ?? "").length, 60);
      assert.ok((sessions[0].title ?? "").endsWith("..."));
    });
  });

  // ---------------------------------------------------------------------------
  // startSession
  // ---------------------------------------------------------------------------

  describe("startSession", () => {
    test("creates session directory with expected layout", async () => {
      const { sessionId } = await service.startSession("Hello");
      const sessionDir = path.join(sessionsDir, sessionId);
      assert.ok(fs.existsSync(sessionDir), "session dir should exist");
      assert.ok(
        fs.existsSync(path.join(sessionDir, "session.json")),
        "session.json should exist",
      );
      assert.ok(
        fs.existsSync(path.join(sessionDir, "conversation.jsonl")),
        "conversation.jsonl should exist",
      );
      assert.ok(
        fs.existsSync(path.join(sessionDir, "artifacts")),
        "artifacts dir should exist",
      );
    });

    test("writes initial metadata with Active status and matching timestamps", async () => {
      const before = Date.now();
      const { sessionId } = await service.startSession("Start me up");
      const meta = await readSessionMetadata(workspaceRoot, sessionId);
      const after = Date.now();

      assert.strictEqual(meta.id, sessionId);
      assert.strictEqual(meta.prompt, "Start me up");
      assert.strictEqual(meta.status, SessionStatus.Active);
      assert.strictEqual(meta.iterations, 0);

      assert.strictEqual(meta.created_at, meta.updated_at);
      const created = Date.parse(meta.created_at);
      assert.ok(created >= before && created <= after, "created_at should be recent");
    });

    test("starts conversation.jsonl as empty", async () => {
      const { sessionId } = await service.startSession("Empty to start");
      const convPath = path.join(sessionsDir, sessionId, "conversation.jsonl");
      const content = await fsp.readFile(convPath, "utf-8");
      assert.strictEqual(content, "");
    });

    test("delegates to processManager.spawn exactly once", async () => {
      const { sessionId } = await service.startSession("Spawn me");
      assert.strictEqual(processStub.spawnCalls.length, 1);
      assert.deepStrictEqual(processStub.spawnCalls[0], {
        id: sessionId,
        prompt: "Spawn me",
      });
    });

    test("generates unique session IDs for concurrent starts", async () => {
      const results = await Promise.all([
        service.startSession("a"),
        service.startSession("b"),
        service.startSession("c"),
        service.startSession("d"),
      ]);
      const ids = results.map((r) => r.sessionId);
      const unique = new Set(ids);
      assert.strictEqual(unique.size, ids.length, "all session IDs must be unique");
    });

    test("creates sessions directory when it does not yet exist", async () => {
      assert.strictEqual(fs.existsSync(sessionsDir), false);
      await service.startSession("First ever");
      assert.strictEqual(fs.existsSync(sessionsDir), true);
    });
  });

  // ---------------------------------------------------------------------------
  // getSession
  // ---------------------------------------------------------------------------

  describe("getSession", () => {
    test("returns details for a started session", async () => {
      const { sessionId } = await service.startSession("Detail me");
      const detail = await service.getSession(sessionId);
      assert.strictEqual(detail.id, sessionId);
      assert.strictEqual(detail.prompt, "Detail me");
      assert.strictEqual(detail.status, SessionStatus.Active);
      assert.deepStrictEqual(detail.conversation, []);
      assert.deepStrictEqual(detail.artifacts, []);
      assert.strictEqual(detail.messageCount, 0);
      assert.strictEqual(
        detail.completedAt,
        undefined,
        "Non-completed session should have no completedAt",
      );
    });

    test("converts backend conversation entries to frontend format", async () => {
      const { sessionId } = await service.startSession("Convo test");
      const convPath = path.join(sessionsDir, sessionId, "conversation.jsonl");
      fs.writeFileSync(
        convPath,
        [
          JSON.stringify({ type: "user_prompt", id: "q1", text: "Why?", ts: "2026-01-01T00:00:00.000Z" }),
          JSON.stringify({ type: "user_response", id: "q1", text: "Because", ts: "2026-01-01T00:01:00.000Z" }),
          "",
        ].join("\n"),
      );

      const detail = await service.getSession(sessionId);
      assert.strictEqual(detail.conversation.length, 2);
      assert.deepStrictEqual(detail.conversation[0], {
        type: "prompt",
        id: "q1",
        content: "Why?",
        timestamp: "2026-01-01T00:00:00.000Z",
      });
      assert.deepStrictEqual(detail.conversation[1], {
        type: "response",
        id: "q1",
        content: "Because",
        timestamp: "2026-01-01T00:01:00.000Z",
      });
      assert.strictEqual(detail.messageCount, 2);
    });

    test("lists artifacts while filtering dotfiles", async () => {
      const { sessionId } = await service.startSession("Artifacts");
      const artifactsDir = path.join(sessionsDir, sessionId, "artifacts");
      fs.writeFileSync(path.join(artifactsDir, "plan.md"), "# plan");
      fs.writeFileSync(path.join(artifactsDir, "notes.txt"), "notes");
      fs.writeFileSync(path.join(artifactsDir, ".hidden"), "nope");

      const detail = await service.getSession(sessionId);
      const artifacts = (detail.artifacts ?? []).slice().sort();
      assert.deepStrictEqual(artifacts, ["notes.txt", "plan.md"]);
    });

    test("returns completedAt when status is Completed", async () => {
      const { sessionId } = await service.startSession("Will complete");
      const metaPath = path.join(sessionsDir, sessionId, "session.json");
      const meta: SessionMetadata = JSON.parse(fs.readFileSync(metaPath, "utf-8"));
      meta.status = SessionStatus.Completed;
      meta.updated_at = "2026-05-01T12:00:00.000Z";
      fs.writeFileSync(metaPath, JSON.stringify(meta));

      const detail = await service.getSession(sessionId);
      assert.strictEqual(detail.status, SessionStatus.Completed);
      assert.strictEqual(detail.completedAt, "2026-05-01T12:00:00.000Z");
    });

    test("throws when session metadata is missing", async () => {
      await assert.rejects(() => service.getSession("does-not-exist"));
    });
  });

  // ---------------------------------------------------------------------------
  // submitResponse
  // ---------------------------------------------------------------------------

  describe("submitResponse", () => {
    test("appends user_response entry and marks session Active", async () => {
      const { sessionId } = await service.startSession("Q");
      // Simulate being in WaitingForInput first.
      const metaPath = path.join(sessionsDir, sessionId, "session.json");
      {
        const meta: SessionMetadata = JSON.parse(fs.readFileSync(metaPath, "utf-8"));
        meta.status = SessionStatus.WaitingForInput;
        fs.writeFileSync(metaPath, JSON.stringify(meta));
      }

      await service.submitResponse(sessionId, "q1", "the answer");

      const convContent = await fsp.readFile(
        path.join(sessionsDir, sessionId, "conversation.jsonl"),
        "utf-8",
      );
      const lines = convContent.trim().split("\n").filter((l) => l.length > 0);
      assert.strictEqual(lines.length, 1);
      const entry = JSON.parse(lines[0]);
      assert.strictEqual(entry.type, "user_response");
      assert.strictEqual(entry.id, "q1");
      assert.strictEqual(entry.text, "the answer");
      assert.ok(typeof entry.ts === "string" && entry.ts.length > 0);

      const meta = await readSessionMetadata(workspaceRoot, sessionId);
      assert.strictEqual(meta.status, SessionStatus.Active);
    });

    test("calls processManager.clearWaiting for the session", async () => {
      const { sessionId } = await service.startSession("Q");
      await service.submitResponse(sessionId, "q1", "answer");
      assert.deepStrictEqual(processStub.clearWaitingCalls, [sessionId]);
    });

    test("updates updated_at timestamp", async () => {
      const { sessionId } = await service.startSession("Q");
      const metaPath = path.join(sessionsDir, sessionId, "session.json");
      const original: SessionMetadata = JSON.parse(
        fs.readFileSync(metaPath, "utf-8"),
      );

      // Backdate updated_at so we can detect the change.
      const backdated = "2020-01-01T00:00:00.000Z";
      const backMeta: SessionMetadata = { ...original, updated_at: backdated };
      fs.writeFileSync(metaPath, JSON.stringify(backMeta));

      await service.submitResponse(sessionId, "q1", "answer");

      const after = await readSessionMetadata(workspaceRoot, sessionId);
      assert.notStrictEqual(
        after.updated_at,
        backdated,
        "updated_at should be refreshed",
      );
      assert.ok(Date.parse(after.updated_at) > Date.parse(backdated));
    });
  });

  // ---------------------------------------------------------------------------
  // deleteSession
  // ---------------------------------------------------------------------------

  describe("deleteSession", () => {
    test("kills process and removes session directory", async () => {
      const { sessionId } = await service.startSession("Delete me");
      const sessionDir = path.join(sessionsDir, sessionId);
      assert.ok(fs.existsSync(sessionDir));

      await service.deleteSession(sessionId);

      assert.deepStrictEqual(processStub.killCalls, [sessionId]);
      assert.strictEqual(
        fs.existsSync(sessionDir),
        false,
        "Session directory should be removed",
      );
    });

    test("does not throw when session directory does not exist", async () => {
      await service.deleteSession("never-existed");
      assert.deepStrictEqual(processStub.killCalls, ["never-existed"]);
    });
  });

  // ---------------------------------------------------------------------------
  // resumeSession
  // ---------------------------------------------------------------------------

  describe("resumeSession", () => {
    test("throws when session does not exist", async () => {
      await assert.rejects(
        () => service.resumeSession("missing-session"),
        /not found/i,
      );
    });

    test("sets status to Active and refreshes updated_at", async () => {
      const { sessionId } = await service.startSession("Resume me");
      const metaPath = path.join(sessionsDir, sessionId, "session.json");

      // Pause the session first.
      {
        const meta: SessionMetadata = JSON.parse(fs.readFileSync(metaPath, "utf-8"));
        meta.status = SessionStatus.Paused;
        meta.updated_at = "2020-01-01T00:00:00.000Z";
        fs.writeFileSync(metaPath, JSON.stringify(meta));
      }

      await service.resumeSession(sessionId);

      const meta = await readSessionMetadata(workspaceRoot, sessionId);
      assert.strictEqual(meta.status, SessionStatus.Active);
      assert.notStrictEqual(meta.updated_at, "2020-01-01T00:00:00.000Z");
    });

    test("spawns process only when not already running", async () => {
      const { sessionId } = await service.startSession("First spawn");
      // Process stub treats session as running after spawn.
      processStub.spawnCalls.length = 0;
      assert.strictEqual(processStub.isRunning(sessionId), true);

      await service.resumeSession(sessionId);
      assert.strictEqual(
        processStub.spawnCalls.length,
        0,
        "Should not re-spawn when already running",
      );

      // Now simulate the process having exited.
      processStub.running.delete(sessionId);
      await service.resumeSession(sessionId);
      assert.strictEqual(
        processStub.spawnCalls.length,
        1,
        "Should spawn when not running",
      );
      assert.strictEqual(processStub.spawnCalls[0].id, sessionId);
      assert.strictEqual(processStub.spawnCalls[0].prompt, "First spawn");
    });
  });

  // ---------------------------------------------------------------------------
  // stopSession
  // ---------------------------------------------------------------------------

  describe("stopSession", () => {
    test("kills process and marks session Paused when running", async () => {
      const { sessionId } = await service.startSession("Stop me");
      await service.stopSession(sessionId);

      assert.deepStrictEqual(processStub.killCalls, [sessionId]);
      const meta = await readSessionMetadata(workspaceRoot, sessionId);
      assert.strictEqual(meta.status, SessionStatus.Paused);
    });

    test("does not update status when no process was running", async () => {
      const { sessionId } = await service.startSession("Already stopped");
      // Force stub to report no running process.
      processStub.running.delete(sessionId);

      await service.stopSession(sessionId);

      // kill was still called (returns false), but status should remain Active.
      const meta = await readSessionMetadata(workspaceRoot, sessionId);
      assert.strictEqual(meta.status, SessionStatus.Active);
    });
  });

  // ---------------------------------------------------------------------------
  // getArtifact
  // ---------------------------------------------------------------------------

  describe("getArtifact", () => {
    test("returns artifact content", async () => {
      const { sessionId } = await service.startSession("A");
      const artifactsDir = path.join(sessionsDir, sessionId, "artifacts");
      fs.writeFileSync(path.join(artifactsDir, "plan.md"), "# hello");

      const result = await service.getArtifact(sessionId, "plan.md");
      assert.strictEqual(result.filename, "plan.md");
      assert.strictEqual(result.content, "# hello");
    });

    test("rejects path traversal attempts", async () => {
      const { sessionId } = await service.startSession("A");
      // Put a file outside the artifacts dir.
      const sessionDir = path.join(sessionsDir, sessionId);
      fs.writeFileSync(path.join(sessionDir, "secret.txt"), "secret");

      await assert.rejects(
        () => service.getArtifact(sessionId, "../secret.txt"),
        /Invalid artifact path/,
      );
    });

    test("throws when artifact does not exist", async () => {
      const { sessionId } = await service.startSession("A");
      await assert.rejects(
        () => service.getArtifact(sessionId, "missing.md"),
        /Artifact not found/,
      );
    });
  });

  // ---------------------------------------------------------------------------
  // Path helpers
  // ---------------------------------------------------------------------------

  describe("path helpers", () => {
    test("getConversationPath returns path under session dir", () => {
      const p = service.getConversationPath("abc-123");
      assert.strictEqual(
        p,
        path.join(sessionsDir, "abc-123", "conversation.jsonl"),
      );
    });

    test("getSessionDir returns session directory under sessions root", () => {
      const p = service.getSessionDir("abc-123");
      assert.strictEqual(p, path.join(sessionsDir, "abc-123"));
    });
  });

  // ---------------------------------------------------------------------------
  // Configuration
  // ---------------------------------------------------------------------------

  describe("configuration", () => {
    test("honors custom defaultTimeoutSeconds without throwing", () => {
      // We do not assert on the internal RalphProcessManager's timeout (it is
      // private). This test simply ensures the option is accepted.
      const svc = new PlanningService({
        workspaceRoot,
        defaultTimeoutSeconds: 42,
      });
      stubProcessManager(svc);
      assert.ok(svc, "service should construct with custom timeout");
    });

    test("honors custom ralphPath without throwing", () => {
      const svc = new PlanningService({
        workspaceRoot,
        ralphPath: "/custom/ralph",
      });
      stubProcessManager(svc);
      assert.ok(svc, "service should construct with custom ralphPath");
    });
  });
});
