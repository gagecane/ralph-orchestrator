/**
 * tRPC Planning Router Tests
 *
 * Tests for the planning.* endpoints that wrap PlanningService. Every
 * endpoint returns INTERNAL_SERVER_ERROR when planningService is not
 * configured on the context; with a stub service injected, inputs are
 * forwarded and outputs / errors propagate correctly.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";
import { appRouter, createContext, type Context } from "./trpc";
import {
  initializeDatabase,
  getDatabase,
  closeDatabase,
} from "../db/connection";
import type { PlanningService } from "../services/PlanningService";

function freshBaseCtx() {
  closeDatabase();
  initializeDatabase(getDatabase(":memory:"));
  return createContext(getDatabase());
}

/**
 * Build a minimal stub PlanningService. Only the methods used by the router
 * are implemented; unused surface is left off (the router never touches them).
 * Each method defaults to a resolved value; per-test overrides replace them.
 */
function stubPlanningService(overrides: Partial<PlanningService> = {}): PlanningService {
  const calls: Record<string, unknown[]> = {};
  const track = <T>(name: string, impl: (...args: any[]) => T) =>
    async (...args: any[]) => {
      calls[name] = args;
      return impl(...args);
    };

  const stub = {
    // Defaults: return empty-ish responses.
    listSessions: track("listSessions", () => []),
    getSession: track("getSession", (id: string) => ({
      id,
      prompt: "test prompt",
      status: "active",
      createdAt: "2026-01-01T00:00:00.000Z",
      updatedAt: "2026-01-01T00:00:00.000Z",
      conversation: [],
    })),
    startSession: track("startSession", (_prompt: string) => ({
      sessionId: "session-123",
    })),
    submitResponse: track("submitResponse", () => undefined),
    resumeSession: track("resumeSession", () => undefined),
    deleteSession: track("deleteSession", () => undefined),
    getArtifact: track("getArtifact", (_sessionId: string, filename: string) => ({
      content: "# artifact content",
      filename,
    })),
    ...overrides,
    /** Call log visible to tests — exposed as an extra property */
    _calls: calls as Record<string, unknown[]>,
  } as unknown as PlanningService & { _calls: Record<string, unknown[]> };

  return stub;
}

function ctxWith(planningService: PlanningService): Context {
  const base = freshBaseCtx();
  return { ...base, planningService };
}

// ---------------------------------------------------------------------------
// "Not configured" error paths
// ---------------------------------------------------------------------------

describe("planning endpoints: planningService not configured", () => {
  // All endpoints must reject with INTERNAL_SERVER_ERROR when the service is
  // missing. The createContext default is planningService: undefined.

  const cases = [
    { name: "list", invoke: (c: any) => c.planning.list() },
    {
      name: "get",
      invoke: (c: any) => c.planning.get({ id: "s1" }),
    },
    {
      name: "start",
      invoke: (c: any) => c.planning.start({ prompt: "hi" }),
    },
    {
      name: "respond",
      invoke: (c: any) =>
        c.planning.respond({ sessionId: "s1", promptId: "p1", response: "ok" }),
    },
    {
      name: "resume",
      invoke: (c: any) => c.planning.resume({ id: "s1" }),
    },
    {
      name: "delete",
      invoke: (c: any) => c.planning.delete({ id: "s1" }),
    },
    {
      name: "getArtifact",
      invoke: (c: any) =>
        c.planning.getArtifact({ sessionId: "s1", filename: "plan.md" }),
    },
  ];

  for (const { name, invoke } of cases) {
    test(`planning.${name} throws INTERNAL_SERVER_ERROR when service missing`, async () => {
      // Given: a context with no planningService
      const ctx = freshBaseCtx();
      assert.strictEqual(ctx.planningService, undefined);
      const caller = appRouter.createCaller(ctx);

      // When/Then: the endpoint rejects with INTERNAL_SERVER_ERROR
      await assert.rejects(
        () => invoke(caller),
        (err: any) => {
          assert.strictEqual(err.code, "INTERNAL_SERVER_ERROR");
          assert.ok(/PlanningService is not configured/.test(err.message));
          return true;
        }
      );
    });
  }
});

// ---------------------------------------------------------------------------
// Happy-path + input forwarding with a stub service
// ---------------------------------------------------------------------------

describe("planning.list tRPC endpoint", () => {
  test("returns sessions returned by PlanningService.listSessions", async () => {
    // Given: a stub service that returns two summaries
    const summaries = [
      {
        id: "s1",
        prompt: "p1",
        status: "active",
        createdAt: "2026-01-01T00:00:00.000Z",
        updatedAt: "2026-01-01T00:00:00.000Z",
      },
      {
        id: "s2",
        prompt: "p2",
        status: "completed",
        createdAt: "2026-01-02T00:00:00.000Z",
        updatedAt: "2026-01-02T00:00:00.000Z",
      },
    ];
    const service = stubPlanningService({
      listSessions: async () => summaries,
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    // When: calling list
    const result = await caller.planning.list();

    // Then: the service output passes through unchanged
    assert.deepStrictEqual(result, summaries);
  });
});

describe("planning.get tRPC endpoint", () => {
  test("forwards the id to PlanningService.getSession", async () => {
    // Given: a stub that echoes the id it receives
    const service = stubPlanningService({
      getSession: async (id: string) => ({
        id,
        prompt: "echo",
        status: "active",
        createdAt: "t",
        updatedAt: "t",
        conversation: [],
      }),
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    // When: fetching session "my-id"
    const result = await caller.planning.get({ id: "my-id" });

    // Then: the echoed id is returned
    assert.strictEqual(result.id, "my-id");
    assert.strictEqual(result.prompt, "echo");
  });
});

describe("planning.start tRPC endpoint", () => {
  test("forwards the prompt and returns the service result", async () => {
    // Given: a stub that returns a new session summary
    const received: string[] = [];
    const service = stubPlanningService({
      startSession: async (prompt: string) => {
        received.push(prompt);
        return { sessionId: "new-session" };
      },
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    // When: starting a session
    const result = await caller.planning.start({ prompt: "Build a feature" });

    // Then: prompt was forwarded, and the returned shape is the service output
    assert.deepStrictEqual(received, ["Build a feature"]);
    assert.strictEqual(result.sessionId, "new-session");
  });

  test("rejects empty prompt via zod", async () => {
    const service = stubPlanningService();
    const caller = appRouter.createCaller(ctxWith(service));

    await assert.rejects(
      () => caller.planning.start({ prompt: "" }),
      (err: any) => {
        assert.strictEqual(err.code, "BAD_REQUEST");
        return true;
      }
    );
  });
});

describe("planning.respond tRPC endpoint", () => {
  test("forwards sessionId, promptId, response to submitResponse", async () => {
    // Given: a stub that records arguments
    let captured: unknown[] = [];
    const service = stubPlanningService({
      submitResponse: async (
        sessionId: string,
        promptId: string,
        response: string
      ) => {
        captured = [sessionId, promptId, response];
      },
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    // When: submitting a response
    const result = await caller.planning.respond({
      sessionId: "s1",
      promptId: "p1",
      response: "my answer",
    });

    // Then: args were forwarded and router returns {success: true}
    assert.deepStrictEqual(captured, ["s1", "p1", "my answer"]);
    assert.deepStrictEqual(result, { success: true });
  });
});

describe("planning.resume tRPC endpoint", () => {
  test("forwards the id to resumeSession and returns {success: true}", async () => {
    let captured: string | undefined;
    const service = stubPlanningService({
      resumeSession: async (id: string) => {
        captured = id;
      },
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    const result = await caller.planning.resume({ id: "s-42" });

    assert.strictEqual(captured, "s-42");
    assert.deepStrictEqual(result, { success: true });
  });
});

describe("planning.delete tRPC endpoint", () => {
  test("forwards the id to deleteSession and returns {success: true}", async () => {
    let captured: string | undefined;
    const service = stubPlanningService({
      deleteSession: async (id: string) => {
        captured = id;
      },
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    const result = await caller.planning.delete({ id: "s-delete" });

    assert.strictEqual(captured, "s-delete");
    assert.deepStrictEqual(result, { success: true });
  });
});

describe("planning.getArtifact tRPC endpoint", () => {
  test("forwards sessionId and filename and returns the service content", async () => {
    // Given: a stub returning fake artifact content
    let capturedArgs: unknown[] = [];
    const service = stubPlanningService({
      getArtifact: async (sessionId: string, filename: string) => {
        capturedArgs = [sessionId, filename];
        return { content: "# spec\n- step 1", filename };
      },
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    // When: fetching an artifact
    const result = await caller.planning.getArtifact({
      sessionId: "s1",
      filename: "plan.md",
    });

    // Then: both args forwarded and content returned
    assert.deepStrictEqual(capturedArgs, ["s1", "plan.md"]);
    assert.strictEqual(result.content, "# spec\n- step 1");
    assert.strictEqual(result.filename, "plan.md");
  });

  test("wraps PlanningService errors as NOT_FOUND", async () => {
    // Given: a service that throws when the artifact is missing
    const service = stubPlanningService({
      getArtifact: async () => {
        throw new Error("artifact missing.md not found");
      },
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    // When/Then: router turns it into NOT_FOUND with the inner message
    await assert.rejects(
      () =>
        caller.planning.getArtifact({
          sessionId: "s1",
          filename: "missing.md",
        }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        assert.ok(/artifact missing\.md not found/.test(err.message));
        return true;
      }
    );
  });

  test("wraps non-Error throws with a generic NOT_FOUND message", async () => {
    // Given: a service that throws a non-Error value
    const service = stubPlanningService({
      getArtifact: async () => {
        // eslint-disable-next-line @typescript-eslint/no-throw-literal
        throw "string-failure";
      },
    } as Partial<PlanningService>);
    const caller = appRouter.createCaller(ctxWith(service));

    // When/Then: router still returns NOT_FOUND but with the fallback message
    await assert.rejects(
      () =>
        caller.planning.getArtifact({
          sessionId: "s1",
          filename: "x.md",
        }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        assert.strictEqual(err.message, "Artifact not found");
        return true;
      }
    );
  });
});
