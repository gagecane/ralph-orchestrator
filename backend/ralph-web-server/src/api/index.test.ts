/**
 * API Barrel Export Tests
 *
 * Tests for api/index.ts — verifies that every public symbol advertised by
 * the API barrel is actually exported and is the same identity as the
 * module it re-exports from. This catches accidental drift where a submodule
 * adds/removes an export but the barrel is not updated.
 */

import { describe, test } from "node:test";
import assert from "node:assert/strict";
import * as apiBarrel from "./index";

// Source modules we re-export from
import * as serverMod from "./server";
import * as trpcMod from "./trpc";
import * as restMod from "./rest";
import * as logBroadcasterMod from "./LogBroadcaster";

describe("api/index.ts barrel exports", () => {
  test("re-exports server module values", () => {
    assert.strictEqual(apiBarrel.createServer, serverMod.createServer);
    assert.strictEqual(apiBarrel.startServer, serverMod.startServer);
  });

  test("re-exports tRPC primitives and routers", () => {
    assert.strictEqual(apiBarrel.appRouter, trpcMod.appRouter);
    assert.strictEqual(apiBarrel.taskRouter, trpcMod.taskRouter);
    assert.strictEqual(apiBarrel.router, trpcMod.router);
    assert.strictEqual(apiBarrel.publicProcedure, trpcMod.publicProcedure);
    assert.strictEqual(apiBarrel.createContext, trpcMod.createContext);
  });

  test("re-exports REST route registration", () => {
    assert.strictEqual(
      apiBarrel.registerRestRoutes,
      restMod.registerRestRoutes
    );
  });

  test("re-exports LogBroadcaster surface", () => {
    assert.strictEqual(
      apiBarrel.LogBroadcaster,
      logBroadcasterMod.LogBroadcaster
    );
    assert.strictEqual(
      apiBarrel.getLogBroadcaster,
      logBroadcasterMod.getLogBroadcaster
    );
    assert.strictEqual(
      apiBarrel.configureLogBroadcaster,
      logBroadcasterMod.configureLogBroadcaster
    );
    assert.strictEqual(
      apiBarrel.resetLogBroadcaster,
      logBroadcasterMod.resetLogBroadcaster
    );
  });

  test("exported functions are callable", () => {
    // These are value-only checks — we're not invoking the server or tRPC
    // here, just confirming the barrel hands out the right kind of value.
    assert.equal(typeof apiBarrel.createServer, "function");
    assert.equal(typeof apiBarrel.startServer, "function");
    assert.equal(typeof apiBarrel.createContext, "function");
    assert.equal(typeof apiBarrel.router, "function");
    assert.equal(typeof apiBarrel.registerRestRoutes, "function");
    assert.equal(typeof apiBarrel.getLogBroadcaster, "function");
    assert.equal(typeof apiBarrel.configureLogBroadcaster, "function");
    assert.equal(typeof apiBarrel.resetLogBroadcaster, "function");
  });

  test("appRouter is a usable tRPC router", () => {
    // tRPC v11 exposes _def on routers — minimal structural sanity check.
    const def = (apiBarrel.appRouter as any)._def;
    assert.ok(def, "appRouter should have a _def");
    assert.ok(def.record, "appRouter._def should have a record map");
  });
});
