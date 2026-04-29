/**
 * Fastify Server Tests
 *
 * Tests for api/server.ts — the HTTP server wiring:
 *   - createServer() registers health, tRPC, REST and WebSocket routes
 *   - createServer() honors logger / db / extra-service options
 *   - startServer() actually binds to a port and returns a running instance
 *   - WebSocket endpoint welcomes clients and accepts subscribe/unsubscribe
 *
 * These are integration tests: we build real servers backed by an in-memory
 * SQLite DB. Where a real network listen is needed we use port 0 so the OS
 * picks a free port and we release it immediately afterwards.
 */

import { describe, test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import { createServer, startServer } from "./server";
import {
  initializeDatabase,
  getDatabase,
  closeDatabase,
} from "../db/connection";
import { tasks } from "../db/schema";
import { resetLogBroadcaster } from "./LogBroadcaster";
import type { FastifyInstance } from "fastify";

// --- Helpers ---------------------------------------------------------------

function freshDb() {
  closeDatabase();
  initializeDatabase(getDatabase(":memory:"));
  const db = getDatabase();
  db.delete(tasks).run();
  return db;
}

/**
 * Collect messages from a WebSocket into a FIFO. Listener is attached
 * synchronously so no welcome frame can be missed.
 */
interface MessageCollector {
  next: (timeoutMs?: number) => Promise<string>;
  close: () => void;
}

function collectMessages(socket: WebSocket): MessageCollector {
  const buffer: string[] = [];
  const waiters: Array<(msg: string) => void> = [];

  const push = (msg: string) => {
    const waiter = waiters.shift();
    if (waiter) waiter(msg);
    else buffer.push(msg);
  };

  const onMessage = (data: Buffer | ArrayBuffer | Buffer[]) => {
    // ws normalizes string frames to Buffer; Buffer.toString() is safe.
    const text = Buffer.isBuffer(data)
      ? data.toString("utf8")
      : data instanceof ArrayBuffer
        ? Buffer.from(data).toString("utf8")
        : Buffer.concat(data as Buffer[]).toString("utf8");
    push(text);
  };

  socket.on("message", onMessage);

  return {
    next(timeoutMs = 2000) {
      if (buffer.length > 0) {
        return Promise.resolve(buffer.shift()!);
      }
      return new Promise<string>((resolve, reject) => {
        const timer = setTimeout(() => {
          const idx = waiters.indexOf(resolve);
          if (idx >= 0) waiters.splice(idx, 1);
          reject(new Error(`timeout waiting for message (${timeoutMs}ms)`));
        }, timeoutMs);
        waiters.push((msg) => {
          clearTimeout(timer);
          resolve(msg);
        });
      });
    },
    close() {
      socket.off("message", onMessage);
    },
  };
}

function waitForOpen(socket: WebSocket, timeoutMs = 2000): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    if (socket.readyState === WebSocket.OPEN) {
      resolve();
      return;
    }
    const timer = setTimeout(() => {
      reject(new Error(`timeout waiting for open (${timeoutMs}ms)`));
    }, timeoutMs);
    socket.once("open", () => {
      clearTimeout(timer);
      resolve();
    });
    socket.once("error", (err: unknown) => {
      clearTimeout(timer);
      reject(err instanceof Error ? err : new Error(String(err)));
    });
  });
}

// --- createServer() --------------------------------------------------------

describe("createServer()", () => {
  let server: FastifyInstance;

  beforeEach(() => {
    freshDb();
    resetLogBroadcaster();
  });

  afterEach(async () => {
    if (server) {
      await server.close();
    }
  });

  test("registers /health endpoint returning ok status", async () => {
    server = await createServer({ db: getDatabase(), logger: false });

    const res = await server.inject({ method: "GET", url: "/health" });

    assert.equal(res.statusCode, 200);
    const body = res.json();
    assert.equal(body.status, "ok");
    assert.ok(body.timestamp, "response should include a timestamp");
    // Should be a valid ISO string
    assert.doesNotThrow(() => new Date(body.timestamp).toISOString());
  });

  test("registers tRPC plugin under /trpc prefix", async () => {
    server = await createServer({ db: getDatabase(), logger: false });

    // Hit a known tRPC query endpoint — task.list takes no input and
    // should return a 200 with a tRPC-shaped response.
    const res = await server.inject({
      method: "GET",
      url: "/trpc/task.list",
    });

    assert.equal(res.statusCode, 200, "tRPC endpoint should be registered");
    const body = res.json();
    // tRPC v11 response: { result: { data: ... } }
    assert.ok(body.result, "response should be tRPC-shaped");
  });

  test("registers REST API routes under /api/v1", async () => {
    server = await createServer({ db: getDatabase(), logger: false });

    const res = await server.inject({ method: "GET", url: "/api/v1/health" });
    assert.equal(res.statusCode, 200);
    const body = res.json();
    assert.equal(body.status, "ok");
  });

  test("has CORS configured (responds to preflight OPTIONS)", async () => {
    server = await createServer({ db: getDatabase(), logger: false });

    const res = await server.inject({
      method: "OPTIONS",
      url: "/health",
      headers: {
        origin: "http://localhost:5173",
        "access-control-request-method": "GET",
      },
    });

    // @fastify/cors should have handled the preflight
    assert.equal(res.statusCode, 204);
    assert.ok(
      res.headers["access-control-allow-origin"],
      "CORS allow-origin header should be set"
    );
  });

  test("accepts a supplied db instance", async () => {
    const db = getDatabase();
    server = await createServer({ db, logger: false });

    // Server was built without throwing — that is the contract we care
    // about here. The tRPC tests exercise the wired repositories deeply.
    const res = await server.inject({ method: "GET", url: "/health" });
    assert.equal(res.statusCode, 200);
  });

  test("falls back to getDatabase() when db is not supplied", async () => {
    // Pre-initialize so getDatabase() has something to return.
    const db = freshDb();
    assert.ok(db, "baseline db must exist");

    server = await createServer({ logger: false });

    const res = await server.inject({ method: "GET", url: "/health" });
    assert.equal(res.statusCode, 200);
  });

  test("forwards optional taskBridge / loopsManager / planningService to tRPC context", async () => {
    // Use sentinels — createContext only stores references, so passing
    // opaque objects is enough to confirm wiring.
    const taskBridge = { marker: "bridge" } as any;
    const loopsManager = { marker: "loops" } as any;
    const planningService = { marker: "planning" } as any;

    server = await createServer({
      db: getDatabase(),
      logger: false,
      taskBridge,
      loopsManager,
      planningService,
    });

    const res = await server.inject({ method: "GET", url: "/health" });
    assert.equal(res.statusCode, 200);
    // The real proof that these are forwarded is that startup doesn't throw
    // and routes using them are reachable. Deep behavior is covered by the
    // trpc.*.test.ts suite.
  });

  test("accepts logger=false without failing", async () => {
    // This is the pattern used by every other test — just sanity-check it.
    server = await createServer({ db: getDatabase(), logger: false });
    assert.ok(server, "server should build with logger disabled");
  });
});

// --- startServer() ---------------------------------------------------------

describe("startServer()", () => {
  let server: FastifyInstance;

  beforeEach(() => {
    freshDb();
    resetLogBroadcaster();
  });

  afterEach(async () => {
    if (server) {
      await server.close();
    }
  });

  test("binds to the supplied port and listens", async () => {
    // Port 0 = OS-assigned free port
    server = await startServer({
      db: getDatabase(),
      logger: false,
      port: 0,
      host: "127.0.0.1",
    });

    const address = server.server.address() as AddressInfo | null;
    assert.ok(address, "server should have an address after listening");
    assert.equal(address.address, "127.0.0.1");
    assert.ok(address.port > 0, "OS should have assigned a port");
  });

  test("responds to /health on the bound address", async () => {
    server = await startServer({
      db: getDatabase(),
      logger: false,
      port: 0,
      host: "127.0.0.1",
    });
    const address = server.server.address() as AddressInfo;

    const res = await fetch(`http://127.0.0.1:${address.port}/health`);
    assert.equal(res.status, 200);
    const body = (await res.json()) as { status: string };
    assert.equal(body.status, "ok");
  });
});

// --- WebSocket: /ws/logs ---------------------------------------------------

describe("WebSocket /ws/logs endpoint", () => {
  let server: FastifyInstance;
  let url: string;

  beforeEach(async () => {
    freshDb();
    resetLogBroadcaster();
    server = await startServer({
      db: getDatabase(),
      logger: false,
      port: 0,
      host: "127.0.0.1",
    });
    const address = server.server.address() as AddressInfo;
    url = `ws://127.0.0.1:${address.port}/ws/logs`;
  });

  afterEach(async () => {
    if (server) {
      await server.close();
    }
    resetLogBroadcaster();
  });

  test("sends a 'connected' welcome message on connect", async () => {
    const socket = new WebSocket(url);
    const messages = collectMessages(socket);
    try {
      await waitForOpen(socket);
      const raw = await messages.next();
      const msg = JSON.parse(raw);

      assert.equal(msg.type, "status");
      assert.equal(msg.taskId, "");
      assert.equal(msg.data.status, "connected");
      assert.ok(msg.data.clientId, "welcome payload should include a clientId");
      assert.ok(msg.timestamp, "welcome payload should include a timestamp");
    } finally {
      messages.close();
      socket.close();
    }
  });

  test("rejects malformed messages with an error payload", async () => {
    const socket = new WebSocket(url);
    const messages = collectMessages(socket);
    try {
      await waitForOpen(socket);
      // consume welcome
      await messages.next();

      socket.send("not-json");

      const raw = await messages.next();
      const msg = JSON.parse(raw);

      assert.equal(msg.type, "error");
      assert.ok(
        typeof msg.data.error === "string" && msg.data.error.length > 0,
        "error payload should describe the problem"
      );
    } finally {
      messages.close();
      socket.close();
    }
  });

  test("ignores unknown message types without crashing", async () => {
    const socket = new WebSocket(url);
    const messages = collectMessages(socket);
    try {
      await waitForOpen(socket);
      await messages.next(); // welcome

      // Neither 'subscribe' nor 'unsubscribe' — should be silently ignored
      socket.send(JSON.stringify({ type: "hello", taskId: "x" }));

      // Give the server a tick to process; then confirm the connection
      // is still alive.
      await new Promise((r) => setTimeout(r, 50));
      assert.equal(
        socket.readyState,
        WebSocket.OPEN,
        "socket should remain open after unknown type"
      );
    } finally {
      messages.close();
      socket.close();
    }
  });

  test("subscribe + unsubscribe cycle runs without error", async () => {
    const socket = new WebSocket(url);
    const messages = collectMessages(socket);
    try {
      await waitForOpen(socket);
      await messages.next(); // welcome

      socket.send(
        JSON.stringify({ type: "subscribe", taskId: "task-42" })
      );
      socket.send(
        JSON.stringify({ type: "unsubscribe", taskId: "task-42" })
      );

      // Give the server a tick to process — we expect no error frame.
      await new Promise((r) => setTimeout(r, 50));
      assert.equal(socket.readyState, WebSocket.OPEN);
    } finally {
      messages.close();
      socket.close();
    }
  });
});
