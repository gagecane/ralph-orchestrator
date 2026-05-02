/**
 * useTaskWebSocket Hook Tests
 *
 * Covers the streaming websocket hook that:
 *   - Subscribes via rpcSubscribe then opens a WebSocket
 *   - Routes task.log.line events into the log store
 *   - Tracks status / ralph events, capped at MAX_EVENTS
 *   - Debounces stream.ack calls (ACK_DEBOUNCE_MS = 250)
 *   - Batches log appends (flush timer 50ms)
 *   - Reconnects with exponential backoff on close / subscribe failure
 *   - Cleans up on taskId change / unmount
 *
 * These tests mock the RPC client and substitute a scriptable WebSocket
 * so they can run fully offline under jsdom.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, renderHook } from "@testing-library/react";
import type { StreamEventEnvelope } from "@/rpc/client";
import { useLogStore } from "@/stores/logStore";
import { useTaskWebSocket } from "./useTaskWebSocket";

// ---------------------------------------------------------------------------
// RPC client mock (hoisted so vi.mock factory can access it)
// ---------------------------------------------------------------------------

const mocks = vi.hoisted(() => {
  class RpcClientError extends Error {
    code: string;
    retryable: boolean;
    constructor(message: string, opts: { code?: string; retryable?: boolean } = {}) {
      super(message);
      this.name = "RpcClientError";
      this.code = opts.code ?? "INTERNAL";
      this.retryable = opts.retryable ?? false;
    }
  }
  return {
    rpcSubscribe: vi.fn(),
    rpcUnsubscribe: vi.fn(),
    rpcAck: vi.fn(),
    buildStreamWebSocketUrl: vi.fn(
      (subscriptionId: string) => `ws://test/?sid=${subscriptionId}`
    ),
    RpcClientError,
  };
});

const { rpcSubscribe, rpcUnsubscribe, rpcAck, buildStreamWebSocketUrl, RpcClientError } = mocks;

vi.mock("@/rpc/client", () => ({
  RpcClientError: mocks.RpcClientError,
  rpcSubscribe: mocks.rpcSubscribe,
  rpcUnsubscribe: mocks.rpcUnsubscribe,
  rpcAck: mocks.rpcAck,
  buildStreamWebSocketUrl: mocks.buildStreamWebSocketUrl,
}));

// ---------------------------------------------------------------------------
// WebSocket stub
// ---------------------------------------------------------------------------

type WsListener = ((event: unknown) => void) | null;

class MockWebSocket {
  static instances: MockWebSocket[] = [];

  url: string;
  readyState: number = 0; // CONNECTING
  onopen: WsListener = null;
  onmessage: WsListener = null;
  onclose: WsListener = null;
  onerror: WsListener = null;
  closeCalled = false;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  // Simulate server driving the socket open.
  simulateOpen() {
    this.readyState = 1;
    this.onopen?.({} as unknown);
  }

  simulateMessage(envelope: Partial<StreamEventEnvelope>) {
    this.onmessage?.({ data: JSON.stringify(envelope) } as unknown);
  }

  simulateRawMessage(data: unknown) {
    this.onmessage?.({ data } as unknown);
  }

  simulateError() {
    this.onerror?.({} as unknown);
  }

  simulateClose() {
    this.readyState = 3;
    this.onclose?.({} as unknown);
  }

  close() {
    this.closeCalled = true;
    this.readyState = 3;
    // Don't auto-fire onclose — the hook may have cleared it already.
  }

  send = vi.fn();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeEnvelope(
  topic: string,
  overrides: Partial<StreamEventEnvelope> & { payload?: unknown } = {}
): StreamEventEnvelope {
  return {
    apiVersion: "v1",
    stream: "events",
    topic,
    cursor: overrides.cursor ?? "c-1",
    sequence: overrides.sequence ?? 1,
    ts: overrides.ts ?? "2026-01-01T00:00:00Z",
    resource: overrides.resource ?? { type: "task", id: "task-1" },
    replay: overrides.replay ?? { mode: "live" },
    payload: overrides.payload ?? null,
  };
}

function resetLogStore() {
  useLogStore.setState({ taskLogs: {}, taskLogMeta: {} });
}

function latestWs(): MockWebSocket {
  const ws = MockWebSocket.instances[MockWebSocket.instances.length - 1];
  if (!ws) throw new Error("No WebSocket was constructed");
  return ws;
}

// ---------------------------------------------------------------------------
// Test setup
// ---------------------------------------------------------------------------

let originalWebSocket: typeof WebSocket;

beforeEach(() => {
  vi.useFakeTimers();

  rpcSubscribe.mockReset();
  rpcUnsubscribe.mockReset();
  rpcUnsubscribe.mockResolvedValue(undefined);
  rpcAck.mockReset();
  rpcAck.mockResolvedValue(undefined);
  buildStreamWebSocketUrl.mockClear();

  MockWebSocket.instances = [];
  originalWebSocket = globalThis.WebSocket;
  (globalThis as unknown as { WebSocket: unknown }).WebSocket = MockWebSocket;

  resetLogStore();

  // Default successful subscribe.
  rpcSubscribe.mockResolvedValue({
    subscriptionId: "sub-1",
    acceptedTopics: ["task.log.line"],
    cursor: "c-0",
  });
});

afterEach(() => {
  (globalThis as unknown as { WebSocket: unknown }).WebSocket = originalWebSocket;
  vi.useRealTimers();
  // Note: don't call vi.restoreAllMocks() here — it wipes the hoisted module
  // mocks, which causes React 19's deferred unmount effects to crash when they
  // call rpcUnsubscribe() and get back `undefined` instead of a Promise.
});

/**
 * Let pending microtasks (rpcSubscribe resolves, .then/.catch chains) settle.
 * We combine this with fake-timer advancement because the hook's connect() path
 * uses an async IIFE kicked off from a useEffect.
 */
async function flushMicrotasks() {
  // Using the real queueMicrotask; vi.useFakeTimers does not stub microtasks.
  for (let i = 0; i < 10; i++) {
    await Promise.resolve();
  }
}

/**
 * Render the hook, wait for rpcSubscribe to resolve (promise microtasks),
 * then open the mock websocket so the hook hits "connected".
 */
async function renderAndConnect(
  taskId: string | null,
  options: Parameters<typeof useTaskWebSocket>[1] = {}
) {
  let view!: ReturnType<typeof renderHook<ReturnType<typeof useTaskWebSocket>, { id: string | null; opts: typeof options }>>;

  await act(async () => {
    view = renderHook(({ id, opts }) => useTaskWebSocket(id, opts), {
      initialProps: { id: taskId, opts: options },
    });
    // Let effect + async IIFE settle.
    await flushMicrotasks();
  });

  if (taskId && (options.autoConnect ?? true)) {
    await act(async () => {
      latestWs().simulateOpen();
    });
  }

  return view;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("useTaskWebSocket", () => {
  describe("initial state", () => {
    it("returns disconnected defaults when taskId is null", () => {
      const { result } = renderHook(() => useTaskWebSocket(null));

      expect(result.current.entries).toEqual([]);
      expect(result.current.latestEntry).toBeNull();
      expect(result.current.events).toEqual([]);
      expect(result.current.latestEvent).toBeNull();
      expect(result.current.connectionState).toBe("disconnected");
      expect(result.current.taskStatus).toBe("unknown");
      expect(result.current.error).toBeNull();
    });

    it("does not attempt to subscribe with no taskId", async () => {
      renderHook(() => useTaskWebSocket(null));

      await act(async () => {
        await vi.runAllTimersAsync();
      });

      expect(rpcSubscribe).not.toHaveBeenCalled();
      expect(MockWebSocket.instances).toHaveLength(0);
    });

    it("does not auto-connect when autoConnect is false", async () => {
      renderHook(() => useTaskWebSocket("task-1", { autoConnect: false }));

      await act(async () => {
        await vi.runAllTimersAsync();
      });

      expect(rpcSubscribe).not.toHaveBeenCalled();
    });
  });

  describe("connection lifecycle", () => {
    it("subscribes and transitions connecting -> connected", async () => {
      const onConnectionChange = vi.fn();
      const { result } = await renderAndConnect("task-1", { onConnectionChange });

      expect(rpcSubscribe).toHaveBeenCalledWith(
        expect.objectContaining({
          topics: expect.arrayContaining(["task.log.line", "task.status.changed"]),
          filters: { taskId: "task-1" },
        })
      );
      expect(buildStreamWebSocketUrl).toHaveBeenCalledWith("sub-1", undefined);

      // Connection change fired with both states (connecting then connected).
      expect(onConnectionChange).toHaveBeenCalledWith("connecting");
      expect(onConnectionChange).toHaveBeenCalledWith("connected");
      expect(result.current.connectionState).toBe("connected");
      expect(result.current.error).toBeNull();
    });

    it("uses custom wsUrl when provided", async () => {
      await renderAndConnect("task-1", { wsUrl: "ws://custom/path" });
      expect(buildStreamWebSocketUrl).toHaveBeenCalledWith("sub-1", "ws://custom/path");
    });

    it("sets error state and schedules reconnect when subscribe fails", async () => {
      rpcSubscribe.mockReset();
      rpcSubscribe.mockRejectedValueOnce(new RpcClientError("nope", { code: "INTERNAL" }));

      let result!: ReturnType<typeof renderHook<ReturnType<typeof useTaskWebSocket>, unknown>>["result"];
      await act(async () => {
        ({ result } = renderHook(() => useTaskWebSocket("task-1")));
        await flushMicrotasks();
      });

      expect(result.current.connectionState).toBe("error");
      expect(result.current.error).toBe("nope");

      // Reconnect should be scheduled; arm success for the retry.
      rpcSubscribe.mockResolvedValueOnce({
        subscriptionId: "sub-2",
        acceptedTopics: ["task.log.line"],
        cursor: "c-0",
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1500);
        await flushMicrotasks();
      });
      await act(async () => {
        latestWs().simulateOpen();
      });

      expect(result.current.connectionState).toBe("connected");
      expect(rpcSubscribe).toHaveBeenCalledTimes(2);
    });

    it("uses Error.message fallback for non-RpcClientError failures", async () => {
      rpcSubscribe.mockReset();
      rpcSubscribe.mockRejectedValueOnce(new Error("boom"));

      let result!: ReturnType<typeof renderHook<ReturnType<typeof useTaskWebSocket>, unknown>>["result"];
      await act(async () => {
        ({ result } = renderHook(() => useTaskWebSocket("task-1")));
        await flushMicrotasks();
      });

      expect(result.current.error).toBe("boom");
    });

    it("falls back to generic error for unknown thrown values", async () => {
      rpcSubscribe.mockReset();
      rpcSubscribe.mockRejectedValueOnce("not-an-error");

      let result!: ReturnType<typeof renderHook<ReturnType<typeof useTaskWebSocket>, unknown>>["result"];
      await act(async () => {
        ({ result } = renderHook(() => useTaskWebSocket("task-1")));
        await flushMicrotasks();
      });

      expect(result.current.error).toBe("Stream connection failed");
    });

    it("transitions to error on WebSocket error event", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateError();
      });

      expect(result.current.connectionState).toBe("error");
      expect(result.current.error).toBe("WebSocket stream connection failed");
    });
  });

  describe("log events", () => {
    it("appends a log line for the current task and flushes into the store", async () => {
      const onLogEntry = vi.fn();
      const { result } = await renderAndConnect("task-1", { onLogEntry });

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.log.line", {
            cursor: "c-log-1",
            sequence: 42,
            payload: { line: "hello world", source: "stdout", timestamp: "2026-01-01T00:00:00Z" },
          })
        );
      });

      // Flush timer (50ms) drains the buffer into the store.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(60);
      });

      expect(result.current.entries).toHaveLength(1);
      const entry = result.current.entries[0];
      expect(entry.line).toBe("hello world");
      expect(entry.source).toBe("stdout");
      expect(entry.id).toBe(42);
      expect(entry.cursor).toBe("c-log-1");
      expect(result.current.latestEntry).toEqual(entry);

      // Callback fired synchronously during message parse.
      expect(onLogEntry).toHaveBeenCalledTimes(1);
      expect(onLogEntry).toHaveBeenCalledWith(expect.objectContaining({ line: "hello world" }));
    });

    it("defaults to stdout source and stringifies unknown payloads", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.log.line", {
            cursor: "c-log-2",
            sequence: 1,
            payload: { foo: "bar" }, // no line/message/text
          })
        );
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(60);
      });

      expect(result.current.entries).toHaveLength(1);
      const entry = result.current.entries[0];
      expect(entry.source).toBe("stdout");
      expect(entry.line).toBe(JSON.stringify({ foo: "bar" }));
    });

    it("ignores log lines for other tasks", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.log.line", {
            cursor: "c-other",
            sequence: 5,
            resource: { type: "task", id: "task-2" },
            payload: { line: "not mine" },
          })
        );
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(60);
      });

      expect(result.current.entries).toHaveLength(0);
    });

    it("ignores malformed messages", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        // Not JSON.
        latestWs().simulateRawMessage("not-json");
        // JSON but missing required fields.
        latestWs().simulateRawMessage(JSON.stringify({ hello: "world" }));
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(60);
      });

      expect(result.current.entries).toHaveLength(0);
      expect(result.current.events).toHaveLength(0);
    });
  });

  describe("status and generic events", () => {
    it("updates taskStatus for task.status.changed using payload.to", async () => {
      const onStatusChange = vi.fn();
      const { result } = await renderAndConnect("task-1", { onStatusChange });

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.status.changed", {
            cursor: "c-s-1",
            sequence: 2,
            payload: { to: "running" },
          })
        );
      });

      expect(result.current.taskStatus).toBe("running");
      expect(onStatusChange).toHaveBeenCalledWith("running");
    });

    it("falls back to payload.status when 'to' is missing", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.status.changed", {
            cursor: "c-s-2",
            sequence: 3,
            payload: { status: "completed" },
          })
        );
      });

      expect(result.current.taskStatus).toBe("completed");
    });

    it("ignores status events for other tasks", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.status.changed", {
            cursor: "c-s-3",
            sequence: 4,
            resource: { type: "task", id: "task-99" },
            payload: { to: "failed" },
          })
        );
      });

      expect(result.current.taskStatus).toBe("unknown");
    });

    it("surfaces error.raised BACKPRESSURE_DROPPED messages", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("error.raised", {
            cursor: "c-e-1",
            sequence: 5,
            payload: { code: "BACKPRESSURE_DROPPED", message: "dropped" },
          })
        );
      });

      expect(result.current.error).toBe("dropped");
    });

    it("does not set error for non-backpressure error.raised codes", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("error.raised", {
            cursor: "c-e-2",
            sequence: 6,
            payload: { code: "INTERNAL", message: "boom" },
          })
        );
      });

      expect(result.current.error).toBeNull();
    });

    it("buffers ralph events but skips stream.keepalive", async () => {
      const onEvent = vi.fn();
      const { result } = await renderAndConnect("task-1", { onEvent });

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("ralph.hat", {
            cursor: "c-r-1",
            sequence: 7,
            payload: { iteration: 3, hat: "reviewer", triggered: "review.file" },
          })
        );
        latestWs().simulateMessage(
          makeEnvelope("stream.keepalive", { cursor: "c-k-1", sequence: 8, payload: null })
        );
      });

      expect(result.current.events).toHaveLength(1);
      expect(result.current.events[0]).toMatchObject({
        topic: "ralph.hat",
        iteration: 3,
        hat: "reviewer",
        triggered: "review.file",
      });
      expect(result.current.latestEvent?.topic).toBe("ralph.hat");
      expect(onEvent).toHaveBeenCalledTimes(1);
    });

    it("caps the events buffer at MAX_EVENTS (200)", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        for (let i = 0; i < 210; i++) {
          latestWs().simulateMessage(
            makeEnvelope("ralph.iteration", {
              cursor: `c-${i}`,
              sequence: i,
              payload: { iteration: i },
            })
          );
        }
      });

      expect(result.current.events).toHaveLength(200);
      // Oldest 10 should have been dropped.
      expect(result.current.events[0]).toMatchObject({ topic: "ralph.iteration", iteration: 10 });
      expect(result.current.events[199]).toMatchObject({ iteration: 209 });
    });

    it("coerces non-object, non-string payloads to strings in ralph events", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("ralph.misc", {
            cursor: "c-num",
            sequence: 1,
            payload: 42 as unknown as StreamEventEnvelope["payload"],
          })
        );
      });

      expect(result.current.events[0].payload).toBe("42");
    });
  });

  describe("ack debouncing", () => {
    it("debounces ack calls to the server", async () => {
      await renderAndConnect("task-1");
      rpcAck.mockClear();

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("ralph.a", { cursor: "cursor-A", sequence: 1, payload: {} })
        );
        latestWs().simulateMessage(
          makeEnvelope("ralph.b", { cursor: "cursor-B", sequence: 2, payload: {} })
        );
      });

      // Before the debounce window elapses, no ack yet.
      expect(rpcAck).not.toHaveBeenCalled();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(260);
      });

      // Single ack with the latest cursor.
      expect(rpcAck).toHaveBeenCalledTimes(1);
      expect(rpcAck).toHaveBeenCalledWith("sub-1", "cursor-B");
    });

    it("suppresses the pending ack if disconnected before it fires", async () => {
      const { result } = await renderAndConnect("task-1");
      rpcAck.mockClear();

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("ralph.a", { cursor: "cursor-A", sequence: 1, payload: {} })
        );
      });

      act(() => {
        result.current.disconnect();
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });

      expect(rpcAck).not.toHaveBeenCalled();
    });
  });

  describe("reconnect", () => {
    it("reconnects with backoff after close; clears on successful open", async () => {
      const onConnectionChange = vi.fn();
      await renderAndConnect("task-1", { onConnectionChange });

      const firstWs = latestWs();
      rpcSubscribe.mockResolvedValueOnce({
        subscriptionId: "sub-2",
        acceptedTopics: ["task.log.line"],
        cursor: "c-0",
      });

      act(() => {
        firstWs.simulateClose();
      });

      // First backoff delay is ~1000ms.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1200);
      });

      expect(MockWebSocket.instances.length).toBeGreaterThanOrEqual(2);
      const secondWs = latestWs();
      expect(secondWs).not.toBe(firstWs);

      act(() => {
        secondWs.simulateOpen();
      });

      expect(onConnectionChange).toHaveBeenCalledWith("connected");
    });

    it("does not reconnect when explicitly disconnected", async () => {
      const { result } = await renderAndConnect("task-1");

      const firstWs = latestWs();
      act(() => {
        result.current.disconnect();
      });

      // disconnect() cleared onclose so it won't fire — and even if a stray close
      // happened, isDisconnectingRef suppresses reconnect.
      act(() => {
        firstWs.simulateClose();
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });

      // No additional subscribe attempts triggered.
      expect(rpcSubscribe).toHaveBeenCalledTimes(1);
    });

    it("passes the last-seen cursor as resume cursor on reconnect", async () => {
      await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.log.line", {
            cursor: "cursor-live",
            sequence: 11,
            payload: { line: "keep me", source: "stdout" },
          })
        );
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(60);
      });

      rpcSubscribe.mockClear();
      rpcSubscribe.mockResolvedValueOnce({
        subscriptionId: "sub-2",
        acceptedTopics: ["task.log.line"],
        cursor: "cursor-live",
      });

      act(() => {
        latestWs().simulateClose();
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1500);
      });

      expect(rpcSubscribe).toHaveBeenCalledTimes(1);
      expect(rpcSubscribe).toHaveBeenCalledWith(
        expect.objectContaining({ cursor: "cursor-live" })
      );
    });
  });

  describe("taskId changes and cleanup", () => {
    it("reuses a persisted cursor from the log store on initial connect", async () => {
      useLogStore.setState({
        taskLogs: {},
        taskLogMeta: { "task-1": { lastCursor: "from-store" } },
      });

      await act(async () => {
        renderHook(() => useTaskWebSocket("task-1"));
        await flushMicrotasks();
      });

      expect(rpcSubscribe).toHaveBeenCalledWith(
        expect.objectContaining({ cursor: "from-store" })
      );
    });

    it("unsubscribes and tears down the socket when taskId becomes null", async () => {
      let hookResult!: { result: { current: ReturnType<typeof useTaskWebSocket> }; rerender: (props: { id: string | null }) => void };
      await act(async () => {
        const view = renderHook(
          ({ id }) => useTaskWebSocket(id),
          { initialProps: { id: "task-1" as string | null } }
        );
        hookResult = view as unknown as typeof hookResult;
        await flushMicrotasks();
      });
      await act(async () => {
        latestWs().simulateOpen();
      });

      const firstWs = latestWs();
      rpcUnsubscribe.mockClear();

      await act(async () => {
        hookResult.rerender({ id: null });
        await flushMicrotasks();
      });

      expect(firstWs.closeCalled).toBe(true);
      expect(rpcUnsubscribe).toHaveBeenCalledWith("sub-1");
      expect(hookResult.result.current.connectionState).toBe("disconnected");
    });

    it("unsubscribes on unmount", async () => {
      const { unmount } = await renderAndConnect("task-1");
      rpcUnsubscribe.mockClear();

      unmount();

      await act(async () => {
        await vi.runAllTimersAsync();
      });

      expect(rpcUnsubscribe).toHaveBeenCalledWith("sub-1");
    });

    it("clearEntries clears the log store for the task and resets error", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.log.line", {
            cursor: "c1",
            sequence: 1,
            payload: { line: "x" },
          })
        );
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(60);
      });
      expect(result.current.entries).toHaveLength(1);

      act(() => {
        // Force an error to confirm clearEntries clears it.
        latestWs().simulateError();
      });
      expect(result.current.error).not.toBeNull();

      act(() => {
        result.current.clearEntries();
      });

      expect(result.current.entries).toHaveLength(0);
      expect(result.current.error).toBeNull();
    });

    it("flushes buffered logs on socket close", async () => {
      const { result } = await renderAndConnect("task-1");

      act(() => {
        latestWs().simulateMessage(
          makeEnvelope("task.log.line", {
            cursor: "c-flush",
            sequence: 99,
            payload: { line: "buffered" },
          })
        );
        // Close immediately, before the 50ms flush timer fires.
        latestWs().simulateClose();
      });

      expect(result.current.entries).toHaveLength(1);
      expect(result.current.entries[0].line).toBe("buffered");
    });
  });
});
