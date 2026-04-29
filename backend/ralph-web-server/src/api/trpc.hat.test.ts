/**
 * tRPC Hat Router Tests
 *
 * Tests for the hat.* endpoints that manage operational hat definitions
 * stored via SettingsService/SettingsRepository:
 *   - hat.list, hat.get, hat.getActive
 *   - hat.setActive, hat.save, hat.delete
 *
 * Uses an in-memory SQLite database so the SettingsRepository backing store
 * is exercised for real; no service mocking required.
 */

import { test, describe, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { appRouter, createContext, hatRouter } from "./trpc";
import {
  initializeDatabase,
  getDatabase,
  closeDatabase,
} from "../db/connection";
import type { HatDefinition } from "../services/SettingsService";

/**
 * Fresh in-memory DB per test — the module-level getDatabase() caches the
 * first instance it was given, so we must closeDatabase() between tests to
 * prevent settings leaking across describe blocks.
 */
function freshDb() {
  closeDatabase();
  initializeDatabase(getDatabase(":memory:"));
  return getDatabase();
}

function makeCtx() {
  freshDb();
  return createContext(getDatabase());
}

const SAMPLE_HAT: HatDefinition = {
  name: "Reviewer",
  description: "Reviews code changes",
  triggersOn: ["code.change"],
  publishes: ["review.done"],
  instructions: "Be thorough.",
};

describe("hat.list tRPC endpoint", () => {
  beforeEach(() => {
    freshDb();
  });

  test("returns empty array when no hats are defined and active hat is default", async () => {
    // Given: a fresh database with no hat definitions
    const ctx = createContext(getDatabase());
    const caller = appRouter.createCaller(ctx);

    // When: listing hats
    const result = await caller.hat.list();

    // Then: returns empty array (definitions map is empty; default hat is
    //       only served as a fallback on getActive/getActiveHatDefinition)
    assert.ok(Array.isArray(result), "Result should be an array");
    assert.strictEqual(result.length, 0);
  });

  test("returns all defined hats with isActive flag", async () => {
    // Given: two hats defined, "reviewer" is active
    const ctx = createContext(getDatabase());
    ctx.settingsService.setHat("reviewer", SAMPLE_HAT);
    ctx.settingsService.setHat("builder", {
      ...SAMPLE_HAT,
      name: "Builder",
      description: "Writes code",
    });
    ctx.settingsService.setActiveHat("reviewer");

    // When: listing hats
    const caller = appRouter.createCaller(ctx);
    const result = await caller.hat.list();

    // Then: both hats returned, with exactly one marked active
    assert.strictEqual(result.length, 2);
    const keys = result.map((h: any) => h.key).sort();
    assert.deepStrictEqual(keys, ["builder", "reviewer"]);

    const reviewer = result.find((h: any) => h.key === "reviewer");
    assert.ok(reviewer);
    assert.strictEqual(reviewer.isActive, true);
    assert.strictEqual(reviewer.name, "Reviewer");

    const builder = result.find((h: any) => h.key === "builder");
    assert.ok(builder);
    assert.strictEqual(builder.isActive, false);
  });
});

describe("hat.getActive tRPC endpoint", () => {
  beforeEach(() => {
    freshDb();
  });

  test("returns the default hat with fallback definition when nothing is configured", async () => {
    // Given: no hats defined, no active hat set
    const ctx = createContext(getDatabase());
    const caller = appRouter.createCaller(ctx);

    // When: fetching the active hat
    const result = await caller.hat.getActive();

    // Then: returns the default hat key + fallback definition
    assert.strictEqual(result.key, "ralph");
    assert.ok(result.definition, "definition should not be null");
    assert.strictEqual(result.definition?.name, "Ralph");
  });

  test("returns the configured active hat and its definition", async () => {
    // Given: a hat is defined and marked active
    const ctx = createContext(getDatabase());
    ctx.settingsService.setHat("reviewer", SAMPLE_HAT);
    ctx.settingsService.setActiveHat("reviewer");

    // When: fetching the active hat
    const caller = appRouter.createCaller(ctx);
    const result = await caller.hat.getActive();

    // Then: key and definition match the stored hat
    assert.strictEqual(result.key, "reviewer");
    assert.strictEqual(result.definition?.name, "Reviewer");
    assert.deepStrictEqual(result.definition?.triggersOn, ["code.change"]);
  });
});

describe("hat.get tRPC endpoint", () => {
  beforeEach(() => {
    freshDb();
  });

  test("returns a defined hat with isActive flag", async () => {
    // Given: a hat is defined and marked active
    const ctx = createContext(getDatabase());
    ctx.settingsService.setHat("reviewer", SAMPLE_HAT);
    ctx.settingsService.setActiveHat("reviewer");

    // When: fetching the hat by key
    const caller = appRouter.createCaller(ctx);
    const result = await caller.hat.get({ key: "reviewer" });

    // Then: returns the hat with active flag set
    assert.strictEqual(result.key, "reviewer");
    assert.strictEqual(result.isActive, true);
    assert.strictEqual(result.name, "Reviewer");
  });

  test("throws NOT_FOUND for an unknown hat", async () => {
    // Given: no hats defined
    const ctx = createContext(getDatabase());
    const caller = appRouter.createCaller(ctx);

    // When/Then: looking up a missing hat raises NOT_FOUND
    await assert.rejects(
      () => caller.hat.get({ key: "missing" }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        return true;
      }
    );
  });

  test("isActive is false when the requested hat is not the active one", async () => {
    // Given: two hats defined, only one is active
    const ctx = createContext(getDatabase());
    ctx.settingsService.setHat("reviewer", SAMPLE_HAT);
    ctx.settingsService.setHat("builder", { ...SAMPLE_HAT, name: "Builder" });
    ctx.settingsService.setActiveHat("builder");

    // When: fetching the non-active hat
    const caller = appRouter.createCaller(ctx);
    const result = await caller.hat.get({ key: "reviewer" });

    // Then: isActive is false
    assert.strictEqual(result.isActive, false);
  });
});

describe("hat.setActive tRPC endpoint", () => {
  beforeEach(() => {
    freshDb();
  });

  test("activates an existing hat", async () => {
    // Given: a defined hat
    const ctx = createContext(getDatabase());
    ctx.settingsService.setHat("reviewer", SAMPLE_HAT);

    // When: setting it active
    const caller = appRouter.createCaller(ctx);
    const result = await caller.hat.setActive({ key: "reviewer" });

    // Then: success and the active hat in the service matches
    assert.deepStrictEqual(result, { success: true, activeHat: "reviewer" });
    assert.strictEqual(ctx.settingsService.getActiveHat(), "reviewer");
  });

  test("throws NOT_FOUND when activating a hat that does not exist", async () => {
    // Given: no hats defined
    const ctx = createContext(getDatabase());
    const caller = appRouter.createCaller(ctx);

    // When/Then: activating a missing hat raises NOT_FOUND and does not change state
    await assert.rejects(
      () => caller.hat.setActive({ key: "ghost" }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        return true;
      }
    );
    // Active hat remains the default
    assert.strictEqual(ctx.settingsService.getActiveHat(), "ralph");
  });
});

describe("hat.save tRPC endpoint", () => {
  beforeEach(() => {
    freshDb();
  });

  test("creates a new hat", async () => {
    // Given: no hats defined
    const ctx = createContext(getDatabase());
    const caller = appRouter.createCaller(ctx);

    // When: saving a new hat
    const result = await caller.hat.save({
      key: "reviewer",
      name: "Reviewer",
      description: "Reviews code",
      triggersOn: ["code.change"],
      publishes: ["review.done"],
      instructions: "Be thorough.",
    });

    // Then: success and the hat is persisted
    assert.deepStrictEqual(result, { success: true, key: "reviewer" });
    const stored = ctx.settingsService.getHat("reviewer");
    assert.ok(stored);
    assert.strictEqual(stored?.name, "Reviewer");
    assert.strictEqual(stored?.instructions, "Be thorough.");
  });

  test("updates an existing hat definition", async () => {
    // Given: a hat already exists
    const ctx = createContext(getDatabase());
    ctx.settingsService.setHat("reviewer", SAMPLE_HAT);

    // When: saving with a new description and different triggers
    const caller = appRouter.createCaller(ctx);
    await caller.hat.save({
      key: "reviewer",
      name: "Reviewer v2",
      description: "Updated description",
      triggersOn: ["pr.opened"],
      publishes: ["review.done", "review.blocked"],
    });

    // Then: the stored hat reflects the update
    const stored = ctx.settingsService.getHat("reviewer");
    assert.strictEqual(stored?.name, "Reviewer v2");
    assert.strictEqual(stored?.description, "Updated description");
    assert.deepStrictEqual(stored?.triggersOn, ["pr.opened"]);
    assert.deepStrictEqual(stored?.publishes, ["review.done", "review.blocked"]);
  });

  test("rejects invalid input via zod (empty key)", async () => {
    const ctx = createContext(getDatabase());
    const caller = appRouter.createCaller(ctx);

    await assert.rejects(
      () =>
        caller.hat.save({
          key: "",
          name: "Reviewer",
          description: "x",
          triggersOn: [],
          publishes: [],
        }),
      (err: any) => {
        // tRPC wraps zod errors as BAD_REQUEST
        assert.strictEqual(err.code, "BAD_REQUEST");
        return true;
      }
    );
  });
});

describe("hat.delete tRPC endpoint", () => {
  beforeEach(() => {
    freshDb();
  });

  test("deletes an existing hat", async () => {
    // Given: a hat exists
    const ctx = createContext(getDatabase());
    ctx.settingsService.setHat("reviewer", SAMPLE_HAT);

    // When: deleting it via tRPC
    const caller = appRouter.createCaller(ctx);
    const result = await caller.hat.delete({ key: "reviewer" });

    // Then: success, and the hat is gone from the service
    assert.deepStrictEqual(result, { success: true });
    assert.strictEqual(ctx.settingsService.getHat("reviewer"), undefined);
  });

  test("throws NOT_FOUND when deleting a missing hat", async () => {
    // Given: no hats defined
    const ctx = createContext(getDatabase());
    const caller = appRouter.createCaller(ctx);

    // When/Then: deleting a missing hat raises NOT_FOUND
    await assert.rejects(
      () => caller.hat.delete({ key: "ghost" }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        return true;
      }
    );
  });

  test("deleting the active hat resets it to the default", async () => {
    // Given: a hat is defined and active
    const ctx = createContext(getDatabase());
    ctx.settingsService.setHat("reviewer", SAMPLE_HAT);
    ctx.settingsService.setActiveHat("reviewer");

    // When: deleting the active hat
    const caller = appRouter.createCaller(ctx);
    await caller.hat.delete({ key: "reviewer" });

    // Then: active hat falls back to the default
    assert.strictEqual(ctx.settingsService.getActiveHat(), "ralph");
  });
});

describe("hatRouter (standalone) createCaller", () => {
  // Confirms the exported hatRouter works on its own (used by the index.ts
  // re-export path and by other test suites), not just via appRouter.

  test("list + save round-trip via hatRouter directly", async () => {
    const ctx = makeCtx();
    const caller = hatRouter.createCaller(ctx);

    const listBefore = await caller.list();
    assert.strictEqual(listBefore.length, 0);

    await caller.save({
      key: "solo",
      name: "Solo",
      description: "Standalone router test",
      triggersOn: [],
      publishes: [],
    });

    const listAfter = await caller.list();
    assert.strictEqual(listAfter.length, 1);
    assert.strictEqual(listAfter[0]!.key, "solo");
  });
});
