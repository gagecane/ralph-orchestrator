/**
 * tRPC Context + App Router Assembly Tests
 *
 * Tests for the shared plumbing in api/trpc/context.ts and api/trpc/index.ts:
 *   - createContext() wires up repositories + services
 *   - createContext() forwards optional taskBridge/loopsManager/planningService
 *   - appRouter exposes every expected sub-router namespace
 *
 * These are small but the glue had no tests at all.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";
import {
  appRouter,
  createContext,
  router,
  publicProcedure,
} from "./trpc";
import {
  initializeDatabase,
  getDatabase,
  closeDatabase,
} from "../db/connection";

function fresh() {
  closeDatabase();
  initializeDatabase(getDatabase(":memory:"));
  return getDatabase();
}

describe("createContext()", () => {
  test("wires up all required repositories and services with defaults", () => {
    // Given: an in-memory database
    const db = fresh();

    // When: building a context with only the db
    const ctx = createContext(db);

    // Then: required fields are present
    assert.ok(ctx.taskRepository, "taskRepository should be set");
    assert.ok(ctx.taskLogRepository, "taskLogRepository should be set");
    assert.ok(ctx.settingsService, "settingsService should be set");
    assert.ok(ctx.collectionService, "collectionService should be set");

    // And: optional fields default to undefined
    assert.strictEqual(ctx.taskBridge, undefined);
    assert.strictEqual(ctx.loopsManager, undefined);
    assert.strictEqual(ctx.planningService, undefined);
  });

  test("forwards the optional taskBridge / loopsManager / planningService", () => {
    const db = fresh();

    // Use sentinel objects — context only stores the references
    const taskBridge = { marker: "bridge" } as any;
    const loopsManager = { marker: "loops" } as any;
    const planningService = { marker: "planning" } as any;

    const ctx = createContext(db, taskBridge, loopsManager, planningService);

    assert.strictEqual(ctx.taskBridge, taskBridge);
    assert.strictEqual(ctx.loopsManager, loopsManager);
    assert.strictEqual(ctx.planningService, planningService);
  });

  test("re-exported router / publicProcedure come from the same tRPC builder", () => {
    // These are re-exported from context.ts via trpc/index.ts, and should
    // be callable as expected (router creates a router, publicProcedure has
    // a .query method). This is a minimal sanity check to ensure the
    // re-exports work for consumers.
    assert.strictEqual(typeof router, "function");
    assert.ok(publicProcedure, "publicProcedure should be defined");
    assert.strictEqual(typeof (publicProcedure as any).query, "function");
  });
});

describe("appRouter assembly", () => {
  test("exposes every expected top-level namespace", () => {
    // Given: the assembled appRouter
    // When: we createCaller with a context
    const db = fresh();
    const ctx = createContext(db);
    const caller = appRouter.createCaller(ctx);

    // Then: every namespace is accessible on the caller (tRPC v11 exposes
    //       sub-routers as proxies, so we just verify the key is truthy —
    //       invoking a procedure would be the deeper check, which the
    //       per-router test files already do).
    const expected = [
      "task",
      "hat",
      "loops",
      "collection",
      "presets",
      "config",
      "planning",
    ];

    for (const ns of expected) {
      assert.ok(
        (caller as any)[ns] !== undefined,
        `appRouter should expose namespace '${ns}'`
      );
    }
  });

  test("appRouter._def.record exposes the same namespaces on the router definition", () => {
    // tRPC v11 exposes the procedure/router map via _def.record. This is a
    // structural guarantee — if someone removes a router from index.ts the
    // test catches it even without invoking any endpoint.
    const record = (appRouter as any)._def?.record;
    assert.ok(record, "router should have _def.record");

    const names = Object.keys(record).sort();
    const expected = [
      "collection",
      "config",
      "hat",
      "loops",
      "planning",
      "presets",
      "task",
    ];
    assert.deepStrictEqual(names, expected);
  });
});
