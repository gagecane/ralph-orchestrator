/**
 * EventBus tests
 *
 * Covers subscribe/publish mechanics, once subscriptions, wildcard fan-out,
 * filters, publishSync, history, waitFor, and fault tolerance.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { EventBus, type Event } from "./EventBus";

describe("EventBus", () => {
  describe("subscribe / publish", () => {
    it("delivers an event to a subscriber", async () => {
      const bus = new EventBus();
      const received: Event[] = [];
      bus.subscribe("greeting", (event) => {
        received.push(event);
      });

      const result = await bus.publish("greeting", { text: "hi" });

      assert.equal(result.handlerCount, 1);
      assert.equal(result.successCount, 1);
      assert.equal(result.errors.length, 0);
      assert.equal(received.length, 1);
      assert.equal(received[0].type, "greeting");
      assert.deepEqual(received[0].payload, { text: "hi" });
      assert.ok(received[0].timestamp instanceof Date);
    });

    it("invokes multiple subscribers for the same event", async () => {
      const bus = new EventBus();
      let countA = 0;
      let countB = 0;
      bus.subscribe("ping", () => {
        countA++;
      });
      bus.subscribe("ping", () => {
        countB++;
      });

      const result = await bus.publish("ping", {});

      assert.equal(countA, 1);
      assert.equal(countB, 1);
      assert.equal(result.handlerCount, 2);
      assert.equal(result.successCount, 2);
    });

    it("returns zero handlers when nothing is subscribed", async () => {
      const bus = new EventBus();
      const result = await bus.publish("noone", {});
      assert.equal(result.handlerCount, 0);
      assert.equal(result.successCount, 0);
      assert.equal(result.errors.length, 0);
    });

    it("propagates correlationId on the emitted event", async () => {
      const bus = new EventBus();
      let captured: Event | undefined;
      bus.subscribe("t", (event) => {
        captured = event;
      });

      await bus.publish("t", {}, { correlationId: "corr-1" });

      assert.equal(captured?.correlationId, "corr-1");
    });

    it("awaits async handlers to completion", async () => {
      const bus = new EventBus();
      let finished = false;
      bus.subscribe("slow", async () => {
        await new Promise((r) => setTimeout(r, 10));
        finished = true;
      });

      const result = await bus.publish("slow", {});
      assert.equal(finished, true);
      assert.equal(result.successCount, 1);
    });
  });

  describe("unsubscribe", () => {
    it("removes a single subscription", async () => {
      const bus = new EventBus();
      let calls = 0;
      const sub = bus.subscribe("x", () => {
        calls++;
      });

      assert.equal(bus.unsubscribe("x", sub.id), true);
      await bus.publish("x", {});
      assert.equal(calls, 0);
    });

    it("Subscription.unsubscribe removes the handler", async () => {
      const bus = new EventBus();
      let calls = 0;
      const sub = bus.subscribe("x", () => {
        calls++;
      });

      sub.unsubscribe();
      await bus.publish("x", {});
      assert.equal(calls, 0);
    });

    it("returns false when the event type or id is unknown", () => {
      const bus = new EventBus();
      assert.equal(bus.unsubscribe("missing", "sub-x"), false);

      const sub = bus.subscribe("x", () => {});
      assert.equal(bus.unsubscribe("x", "not-the-real-id"), false);
      assert.equal(bus.unsubscribe("x", sub.id), true);
    });

    it("cleans up the internal list when the last subscriber leaves", () => {
      const bus = new EventBus();
      const sub = bus.subscribe("x", () => {});
      assert.deepEqual(bus.getEventTypes(), ["x"]);
      sub.unsubscribe();
      assert.deepEqual(bus.getEventTypes(), []);
    });
  });

  describe("once", () => {
    it("fires exactly once and then auto-removes", async () => {
      const bus = new EventBus();
      let calls = 0;
      bus.once("boom", () => {
        calls++;
      });

      await bus.publish("boom", {});
      await bus.publish("boom", {});

      assert.equal(calls, 1);
      assert.equal(bus.getSubscriberCount("boom"), 0);
    });
  });

  describe("wildcard subscriptions", () => {
    it("receives every event type", async () => {
      const bus = new EventBus();
      const seen: string[] = [];
      bus.subscribe("*", (event) => {
        seen.push(event.type);
      });

      await bus.publish("a", {});
      await bus.publish("b", {});
      await bus.publish("c", {});

      assert.deepEqual(seen, ["a", "b", "c"]);
    });

    it("fires alongside specific subscribers (handlerCount counts both)", async () => {
      const bus = new EventBus();
      let specific = 0;
      let wildcard = 0;
      bus.subscribe("t", () => {
        specific++;
      });
      bus.subscribe("*", () => {
        wildcard++;
      });

      const result = await bus.publish("t", {});
      assert.equal(specific, 1);
      assert.equal(wildcard, 1);
      assert.equal(result.handlerCount, 2);
    });
  });

  describe("filters", () => {
    it("skips handlers whose filter rejects the event", async () => {
      const bus = new EventBus();
      const seen: Event[] = [];
      bus.subscribe<{ n: number }>(
        "num",
        (event) => {
          seen.push(event);
        },
        { filter: (event) => (event.payload as { n: number }).n > 0 }
      );

      const dropped = await bus.publish("num", { n: -1 });
      const kept = await bus.publish("num", { n: 2 });

      assert.equal(seen.length, 1);
      assert.equal(dropped.handlerCount, 0);
      assert.equal(kept.handlerCount, 1);
    });

    it("does not consume a 'once' subscription if the filter rejects", async () => {
      const bus = new EventBus();
      let calls = 0;
      bus.subscribe(
        "t",
        () => {
          calls++;
        },
        { once: true, filter: (event) => (event.payload as { ok: boolean }).ok === true }
      );

      await bus.publish("t", { ok: false });
      assert.equal(bus.getSubscriberCount("t"), 1);

      await bus.publish("t", { ok: true });
      assert.equal(calls, 1);
      assert.equal(bus.getSubscriberCount("t"), 0);
    });
  });

  describe("fault tolerance", () => {
    it("collects errors from sync-throwing handlers and continues", async () => {
      const bus = new EventBus();
      let goodCalls = 0;
      bus.subscribe("e", () => {
        throw new Error("sync boom");
      });
      bus.subscribe("e", () => {
        goodCalls++;
      });

      const result = await bus.publish("e", {});
      assert.equal(goodCalls, 1);
      assert.equal(result.handlerCount, 2);
      assert.equal(result.successCount, 1);
      assert.equal(result.errors.length, 1);
      assert.match(result.errors[0].message, /sync boom/);
    });

    it("collects rejections from async handlers", async () => {
      const bus = new EventBus();
      bus.subscribe("e", async () => {
        throw new Error("async boom");
      });

      const result = await bus.publish("e", {});
      assert.equal(result.successCount, 0);
      assert.equal(result.errors.length, 1);
      assert.match(result.errors[0].message, /async boom/);
    });

    it("wraps non-Error rejections into Error objects", async () => {
      const bus = new EventBus();
      bus.subscribe("e", () => {
        throw "bare string";
      });

      const result = await bus.publish("e", {});
      assert.equal(result.errors.length, 1);
      assert.ok(result.errors[0] instanceof Error);
      assert.match(result.errors[0].message, /bare string/);
    });

    it("waitForHandlers=false does not block on async work", async () => {
      const bus = new EventBus();
      let finished = false;
      bus.subscribe("slow", async () => {
        await new Promise((r) => setTimeout(r, 30));
        finished = true;
      });

      const result = await bus.publish("slow", {}, { waitForHandlers: false });
      // successCount is incremented optimistically before the promise settles.
      assert.equal(result.handlerCount, 1);
      assert.equal(finished, false);

      // But the handler still runs in the background.
      await new Promise((r) => setTimeout(r, 60));
      assert.equal(finished, true);
    });
  });

  describe("publishSync", () => {
    it("returns immediately with the subscriber count", () => {
      const bus = new EventBus();
      bus.subscribe("t", () => {});
      bus.subscribe("t", () => {});

      const result = bus.publishSync("t", { value: 1 });
      assert.equal(result.handlerCount, 2);
      assert.equal(result.successCount, 0);
      assert.equal(result.event.type, "t");
      assert.deepEqual(result.event.payload, { value: 1 });
    });

    it("propagates correlationId through the synthetic event", () => {
      const bus = new EventBus();
      const result = bus.publishSync("t", {}, { correlationId: "abc" });
      assert.equal(result.event.correlationId, "abc");
    });
  });

  describe("history", () => {
    it("is disabled by default", async () => {
      const bus = new EventBus();
      await bus.publish("t", {});
      assert.deepEqual(bus.getHistory(), []);
    });

    it("retains events up to maxHistorySize", async () => {
      const bus = new EventBus({ maxHistorySize: 3 });
      for (let i = 0; i < 5; i++) {
        await bus.publish("t", { i });
      }
      const history = bus.getHistory();
      assert.equal(history.length, 3);
      assert.deepEqual(
        history.map((e) => (e.payload as { i: number }).i),
        [2, 3, 4]
      );
    });

    it("getHistory(limit) returns the last N events", async () => {
      const bus = new EventBus({ maxHistorySize: 10 });
      for (let i = 0; i < 5; i++) {
        await bus.publish("t", { i });
      }
      const last2 = bus.getHistory(2);
      assert.equal(last2.length, 2);
      assert.deepEqual(
        last2.map((e) => (e.payload as { i: number }).i),
        [3, 4]
      );
    });

    it("getHistoryByType filters by event type", async () => {
      const bus = new EventBus({ maxHistorySize: 10 });
      await bus.publish("a", {});
      await bus.publish("b", {});
      await bus.publish("a", {});

      assert.equal(bus.getHistoryByType("a").length, 2);
      assert.equal(bus.getHistoryByType("b").length, 1);
      assert.equal(bus.getHistoryByType("a", 1).length, 1);
    });

    it("clearHistory empties the log", async () => {
      const bus = new EventBus({ maxHistorySize: 5 });
      await bus.publish("t", {});
      assert.equal(bus.getHistory().length, 1);
      bus.clearHistory();
      assert.equal(bus.getHistory().length, 0);
    });
  });

  describe("introspection", () => {
    it("getSubscriberCount sums specific + wildcard subscribers", () => {
      const bus = new EventBus();
      bus.subscribe("t", () => {});
      bus.subscribe("t", () => {});
      bus.subscribe("*", () => {});

      assert.equal(bus.getSubscriberCount("t"), 3);
      assert.equal(bus.getSubscriberCount("other"), 1);
      assert.equal(bus.getSubscriberCount("*"), 1);
    });

    it("getEventTypes lists every bucket with subscribers", () => {
      const bus = new EventBus();
      bus.subscribe("a", () => {});
      bus.subscribe("b", () => {});
      bus.subscribe("*", () => {});

      const types = bus.getEventTypes().sort();
      assert.deepEqual(types, ["*", "a", "b"]);
    });

    it("hasSubscribers reflects current state", () => {
      const bus = new EventBus();
      assert.equal(bus.hasSubscribers("t"), false);
      const sub = bus.subscribe("t", () => {});
      assert.equal(bus.hasSubscribers("t"), true);
      sub.unsubscribe();
      assert.equal(bus.hasSubscribers("t"), false);
    });
  });

  describe("clear", () => {
    it("drops every subscription", () => {
      const bus = new EventBus();
      bus.subscribe("a", () => {});
      bus.subscribe("*", () => {});

      bus.clear();

      assert.deepEqual(bus.getEventTypes(), []);
      assert.equal(bus.hasSubscribers("a"), false);
    });
  });

  describe("waitFor", () => {
    it("resolves with the next matching event", async () => {
      const bus = new EventBus();

      const waiter = bus.waitFor<{ v: number }>("ready");
      queueMicrotask(() => {
        void bus.publish("ready", { v: 42 });
      });

      const event = await waiter;
      assert.equal(event.type, "ready");
      assert.deepEqual(event.payload, { v: 42 });
    });

    it("rejects after the timeout elapses", async () => {
      const bus = new EventBus();
      await assert.rejects(() => bus.waitFor("never", 5), /Timeout waiting for event: never/);
    });

    it("cleans up its subscription when the timeout fires", async () => {
      const bus = new EventBus();
      try {
        await bus.waitFor("never", 5);
      } catch {
        // expected
      }
      assert.equal(bus.getSubscriberCount("never"), 0);
    });
  });
});
