/**
 * CollectionBuilder Component Tests
 *
 * Tests CollectionBuilder's orchestration logic independently of
 * @xyflow/react's canvas rendering. ReactFlow is mocked so we can:
 *
 *   1. Render reliably in jsdom (ReactFlow depends on ResizeObserver,
 *      DOMMatrix, and layout APIs that jsdom does not ship).
 *   2. Capture the callbacks CollectionBuilder wires in (onNodesChange,
 *      onEdgesChange, onConnect) and invoke them directly to exercise
 *      the real state-management code paths.
 *
 * Covered behavior (scope from bead ro-evi):
 *   1. Render with empty and populated collections
 *   2. Add/remove/reorder collection items (via drop + state callbacks)
 *   3. Validation of collection item input (save disabled without name)
 *   4. Save / submit behavior
 *   5. Export YAML button presence / callback wiring
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import type { Node, Edge } from "@xyflow/react";
import type { HatNodeData } from "./HatNode";

// -------------------------------------------------------------------
// Mock @xyflow/react
//
// CollectionBuilder uses ReactFlow as a controlled component, passing
// nodes / edges / callbacks down. We replace the whole module with a
// lightweight fake that:
//   - Captures the latest props on `latestReactFlowProps` so tests can
//     invoke onNodesChange / onEdgesChange / onConnect directly.
//   - Reuses the real `addEdge`, `applyNodeChanges`, `applyEdgeChanges`
//     semantics (minimal re-implementation; the real ones assume
//     ReactFlow internals we do not want to pull in).
//   - Renders nodes as `data-testid="rf-node-<id>"` so assertions can
//     observe state changes through the DOM.
// -------------------------------------------------------------------

interface CapturedReactFlowProps {
  nodes: Node[];
  edges: Edge[];
  onNodesChange?: (changes: unknown[]) => void;
  onEdgesChange?: (changes: unknown[]) => void;
  onConnect?: (conn: {
    source: string;
    target: string;
    sourceHandle?: string | null;
    targetHandle?: string | null;
  }) => void;
}

const rfState: { latest: CapturedReactFlowProps | null } = { latest: null };

vi.mock("@xyflow/react", async () => {
  const React = await import("react");

  function FakeReactFlow(props: CapturedReactFlowProps & { children?: React.ReactNode }) {
    // Capture props so tests can drive callbacks.
    rfState.latest = {
      nodes: props.nodes,
      edges: props.edges,
      onNodesChange: props.onNodesChange,
      onEdgesChange: props.onEdgesChange,
      onConnect: props.onConnect,
    };

    return React.createElement(
      "div",
      { "data-testid": "react-flow-canvas" },
      props.nodes.map((n) =>
        React.createElement(
          "div",
          {
            key: n.id,
            "data-testid": `rf-node-${n.id}`,
            "data-node-type": n.type,
          },
          // Expose hat name in node text for query-by-text assertions
          (n.data as { name?: string } | undefined)?.name ?? ""
        )
      ),
      props.edges.map((e) =>
        React.createElement("div", {
          key: e.id,
          "data-testid": `rf-edge-${e.id}`,
          "data-source": e.source,
          "data-target": e.target,
          "data-label": String(e.label ?? ""),
        })
      ),
      props.children
    );
  }

  function FakeProvider({ children }: { children: React.ReactNode }) {
    return React.createElement(React.Fragment, null, children);
  }

  // Minimal but faithful state helpers
  function applyNodeChanges(changes: Array<Record<string, unknown>>, nodes: Node[]): Node[] {
    let out = nodes.slice();
    for (const c of changes) {
      const type = c.type as string;
      if (type === "add") {
        out.push(c.item as Node);
      } else if (type === "remove") {
        out = out.filter((n) => n.id !== (c.id as string));
      } else if (type === "position") {
        out = out.map((n) =>
          n.id === (c.id as string) && c.position
            ? { ...n, position: c.position as { x: number; y: number } }
            : n
        );
      } else if (type === "select") {
        out = out.map((n) =>
          n.id === (c.id as string) ? { ...n, selected: Boolean(c.selected) } : n
        );
      }
    }
    return out;
  }

  function applyEdgeChanges(changes: Array<Record<string, unknown>>, edges: Edge[]): Edge[] {
    let out = edges.slice();
    for (const c of changes) {
      const type = c.type as string;
      if (type === "add") {
        out.push(c.item as Edge);
      } else if (type === "remove") {
        out = out.filter((e) => e.id !== (c.id as string));
      }
    }
    return out;
  }

  function addEdge(edge: Edge, edges: Edge[]): Edge[] {
    return [...edges, edge];
  }

  // Minimal placeholders for the other imports CollectionBuilder pulls
  const noop = () => null;
  return {
    ReactFlow: FakeReactFlow,
    ReactFlowProvider: FakeProvider,
    Controls: noop,
    Background: noop,
    MiniMap: noop,
    Handle: noop,
    Position: { Top: "top", Bottom: "bottom", Left: "left", Right: "right" },
    BackgroundVariant: { Dots: "dots", Lines: "lines", Cross: "cross" },
    addEdge,
    applyNodeChanges,
    applyEdgeChanges,
    getBezierPath: () => ["M 0 0", 0, 0],
  };
});

// ReactFlow CSS import side-effect — stub it.
vi.mock("@xyflow/react/dist/style.css", () => ({}));

// Mock uuid so generated ids are predictable.
let uuidCounter = 0;
vi.mock("uuid", () => ({
  v4: () => {
    uuidCounter += 1;
    return `uuid-${uuidCounter.toString().padStart(8, "0")}`;
  },
}));

// Mock HatPalette + PropertiesPanel. They render their own widgets that
// are not under test here; stubbing isolates the Builder.
vi.mock("./HatPalette", () => ({
  HatPalette: () => <div data-testid="hat-palette" />,
}));

vi.mock("./PropertiesPanel", () => ({
  PropertiesPanel: ({
    selectedNode,
    onUpdateNode,
    onDeleteNode,
  }: {
    selectedNode: { id: string; data: HatNodeData } | null;
    onUpdateNode: (id: string, data: Partial<HatNodeData>) => void;
    onDeleteNode: (id: string) => void;
  }) => (
    <div data-testid="properties-panel">
      <div data-testid="selected-node-id">{selectedNode?.id ?? ""}</div>
      <div data-testid="selected-node-name">{selectedNode?.data.name ?? ""}</div>
      <button
        type="button"
        data-testid="rename-selected"
        onClick={() =>
          selectedNode && onUpdateNode(selectedNode.id, { name: "Renamed Hat" })
        }
      >
        rename
      </button>
      <button
        type="button"
        data-testid="delete-selected"
        onClick={() => selectedNode && onDeleteNode(selectedNode.id)}
      >
        delete
      </button>
    </div>
  ),
}));

// HatNode and friends are not reached because we mock @xyflow/react's
// renderer, but their import must resolve.
vi.mock("./HatNode", async () => {
  const actual = await vi.importActual<typeof import("./HatNode")>("./HatNode");
  return actual;
});

// -------------------------------------------------------------------
// Import under test (AFTER mocks are declared)
// -------------------------------------------------------------------

import { CollectionBuilder } from "./CollectionBuilder";

// -------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------

function makeHatNode(id: string, overrides: Partial<HatNodeData> = {}): Node {
  return {
    id,
    type: "hatNode",
    position: { x: 0, y: 0 },
    data: {
      key: id,
      name: overrides.name ?? `Hat ${id}`,
      description: overrides.description ?? "",
      triggersOn: overrides.triggersOn ?? [],
      publishes: overrides.publishes ?? [],
      instructions: overrides.instructions,
    } as unknown as Record<string, unknown>,
  };
}

function renderBuilder(
  overrides: Partial<React.ComponentProps<typeof CollectionBuilder>> = {}
) {
  const onSave = vi.fn();
  const onExportYaml = vi.fn();
  const onNameChange = vi.fn();
  const onDescriptionChange = vi.fn();

  const utils = render(
    <CollectionBuilder
      collectionId={null}
      name="Test Collection"
      description="Test description"
      onSave={onSave}
      onExportYaml={onExportYaml}
      onNameChange={onNameChange}
      onDescriptionChange={onDescriptionChange}
      {...overrides}
    />
  );

  return { ...utils, onSave, onExportYaml, onNameChange, onDescriptionChange };
}

function buildDropEvent(data: Record<string, string>): Parameters<
  typeof fireEvent.drop
>[1] {
  return {
    dataTransfer: {
      getData: (key: string) => data[key] ?? "",
      dropEffect: "move",
      effectAllowed: "move",
    },
    clientX: 200,
    clientY: 100,
  } as unknown as Parameters<typeof fireEvent.drop>[1];
}

/** Invoke one of the captured ReactFlow callbacks inside act() so React flushes updates. */
function runInAct(fn: () => void) {
  act(() => {
    fn();
  });
}

// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

describe("CollectionBuilder", () => {
  beforeEach(() => {
    uuidCounter = 0;
    rfState.latest = null;
  });

  describe("rendering", () => {
    it("renders toolbar inputs with the supplied name and description", () => {
      renderBuilder({ name: "My Coll", description: "Hello" });

      expect(screen.getByPlaceholderText("Collection name")).toHaveValue("My Coll");
      expect(screen.getByPlaceholderText("Description")).toHaveValue("Hello");
    });

    it("renders an empty canvas when no initial data is provided", () => {
      renderBuilder();

      expect(screen.getByTestId("react-flow-canvas")).toBeInTheDocument();
      expect(rfState.latest?.nodes).toEqual([]);
      expect(rfState.latest?.edges).toEqual([]);
    });

    it("renders a populated canvas when initial nodes and edges are provided", () => {
      const nodes = [makeHatNode("n1", { name: "Planner" }), makeHatNode("n2", { name: "Builder" })];
      const edges: Edge[] = [
        { id: "e1", source: "n1", target: "n2", label: "build.task" },
      ];

      renderBuilder({ initialData: { nodes, edges } });

      expect(screen.getByTestId("rf-node-n1")).toHaveTextContent("Planner");
      expect(screen.getByTestId("rf-node-n2")).toHaveTextContent("Builder");
      expect(screen.getByTestId("rf-edge-e1")).toHaveAttribute("data-label", "build.task");
    });

    it("mounts the HatPalette and PropertiesPanel sidebars", () => {
      renderBuilder();

      expect(screen.getByTestId("hat-palette")).toBeInTheDocument();
      expect(screen.getByTestId("properties-panel")).toBeInTheDocument();
    });

    it("hides the Export YAML button when onExportYaml is not supplied", () => {
      renderBuilder({ onExportYaml: undefined });
      expect(screen.queryByRole("button", { name: /export yaml/i })).not.toBeInTheDocument();
    });

    it("shows the Export YAML button when onExportYaml is supplied", () => {
      renderBuilder();
      expect(screen.getByRole("button", { name: /export yaml/i })).toBeInTheDocument();
    });
  });

  describe("toolbar inputs", () => {
    it("forwards name edits to onNameChange", () => {
      const { onNameChange } = renderBuilder();

      fireEvent.change(screen.getByPlaceholderText("Collection name"), {
        target: { value: "New Name" },
      });

      expect(onNameChange).toHaveBeenCalledWith("New Name");
    });

    it("forwards description edits to onDescriptionChange", () => {
      const { onDescriptionChange } = renderBuilder();

      fireEvent.change(screen.getByPlaceholderText("Description"), {
        target: { value: "New description" },
      });

      expect(onDescriptionChange).toHaveBeenCalledWith("New description");
    });
  });

  describe("save validation and submit", () => {
    it("disables the Save button while isSaving is true", () => {
      renderBuilder({ isSaving: true });

      const save = screen.getByRole("button", { name: /saving/i });
      expect(save).toBeDisabled();
    });

    it("disables the Save button when the collection name is empty", () => {
      renderBuilder({ name: "" });

      expect(screen.getByRole("button", { name: /^save$/i })).toBeDisabled();
    });

    it("disables the Save button when the collection name is only whitespace", () => {
      renderBuilder({ name: "   " });

      expect(screen.getByRole("button", { name: /^save$/i })).toBeDisabled();
    });

    it("enables the Save button when a non-empty name is present", () => {
      renderBuilder({ name: "Something" });

      expect(screen.getByRole("button", { name: /^save$/i })).toBeEnabled();
    });

    it("invokes onSave with current nodes, edges, name, and description", () => {
      const nodes = [makeHatNode("n1", { name: "Planner" })];
      const edges: Edge[] = [];
      const { onSave } = renderBuilder({
        name: "My Coll",
        description: "Desc",
        initialData: { nodes, edges },
      });

      fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

      expect(onSave).toHaveBeenCalledTimes(1);
      expect(onSave).toHaveBeenCalledWith({
        nodes,
        edges,
        name: "My Coll",
        description: "Desc",
      });
    });

    it("invokes onExportYaml when the Export YAML button is clicked", () => {
      const { onExportYaml } = renderBuilder();

      fireEvent.click(screen.getByRole("button", { name: /export yaml/i }));

      expect(onExportYaml).toHaveBeenCalledTimes(1);
    });
  });

  describe("adding nodes via drop (palette drag)", () => {
    it("adds a new hat node at the drop position when a hat template is dropped", () => {
      renderBuilder();

      const canvas = screen.getByTestId("react-flow-canvas").parentElement!;
      const template: HatNodeData = {
        key: "planner",
        name: "Planner",
        description: "",
        triggersOn: [],
        publishes: [],
      };
      fireEvent.drop(
        canvas,
        buildDropEvent({ "application/reactflow": JSON.stringify(template) })
      );

      // Should have exactly one hat node with generated id
      const hatNode = rfState.latest?.nodes.find((n) => n.type === "hatNode");
      expect(hatNode).toBeDefined();
      expect(hatNode!.id).toMatch(/^planner-/);
      expect((hatNode!.data as unknown as HatNodeData).key).toBe(hatNode!.id);
      expect((hatNode!.data as unknown as HatNodeData).name).toBe("Planner");
    });

    it("adds a reroute node when a reroute is dropped", () => {
      renderBuilder();
      const canvas = screen.getByTestId("react-flow-canvas").parentElement!;

      fireEvent.drop(canvas, buildDropEvent({ "application/reroute": "true" }));

      const reroute = rfState.latest?.nodes.find((n) => n.type === "reroute");
      expect(reroute).toBeDefined();
      expect(reroute!.id).toMatch(/^reroute-/);
    });

    it("ignores drops that carry no recognizable payload", () => {
      renderBuilder();
      const canvas = screen.getByTestId("react-flow-canvas").parentElement!;

      fireEvent.drop(canvas, buildDropEvent({}));

      expect(rfState.latest?.nodes).toEqual([]);
    });
  });

  describe("removing nodes", () => {
    it("removes a node and any edges touching it when PropertiesPanel requests a delete", () => {
      const nodes = [
        makeHatNode("n1", { name: "Planner" }),
        makeHatNode("n2", { name: "Builder" }),
      ];
      const edges: Edge[] = [
        { id: "e1", source: "n1", target: "n2", label: "build.task" },
        { id: "e2", source: "n2", target: "n1", label: "build.blocked" },
      ];
      renderBuilder({ initialData: { nodes, edges } });

      // Select n1 via ReactFlow's onNodesChange (simulating click-to-select)
      runInAct(() =>
        rfState.latest?.onNodesChange?.([
          { type: "select", id: "n1", selected: true },
        ])
      );

      // PropertiesPanel exposes its selected node; the mock renders it when selected.
      // Now ask PropertiesPanel to delete.
      fireEvent.click(screen.getByTestId("delete-selected"));

      const remainingNodes = rfState.latest?.nodes ?? [];
      const remainingEdges = rfState.latest?.edges ?? [];
      expect(remainingNodes.map((n) => n.id)).toEqual(["n2"]);
      // Both edges touched n1 and should be gone
      expect(remainingEdges).toEqual([]);
    });

    it("clears selection after deleting the selected node", () => {
      const nodes = [makeHatNode("n1", { name: "Planner" })];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onNodesChange?.([
          { type: "select", id: "n1", selected: true },
        ])
      );
      expect(screen.getByTestId("selected-node-id")).toHaveTextContent("n1");

      fireEvent.click(screen.getByTestId("delete-selected"));

      expect(screen.getByTestId("selected-node-id")).toHaveTextContent("");
    });
  });

  describe("updating nodes", () => {
    it("merges property updates from PropertiesPanel into the selected node", () => {
      const nodes = [makeHatNode("n1", { name: "Planner" })];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onNodesChange?.([
          { type: "select", id: "n1", selected: true },
        ])
      );

      fireEvent.click(screen.getByTestId("rename-selected"));

      const updated = rfState.latest?.nodes.find((n) => n.id === "n1");
      expect((updated!.data as unknown as HatNodeData).name).toBe("Renamed Hat");
      // Other fields are preserved
      expect((updated!.data as unknown as HatNodeData).key).toBe("n1");
    });
  });

  describe("selection tracking", () => {
    it("surfaces a selected hat node to PropertiesPanel", () => {
      const nodes = [makeHatNode("n1", { name: "Planner" })];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onNodesChange?.([
          { type: "select", id: "n1", selected: true },
        ])
      );

      expect(screen.getByTestId("selected-node-id")).toHaveTextContent("n1");
      expect(screen.getByTestId("selected-node-name")).toHaveTextContent("Planner");
    });

    it("clears selection when a node is deselected", () => {
      const nodes = [makeHatNode("n1", { name: "Planner" })];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onNodesChange?.([{ type: "select", id: "n1", selected: true }])
      );
      runInAct(() =>
        rfState.latest?.onNodesChange?.([{ type: "select", id: "n1", selected: false }])
      );

      expect(screen.getByTestId("selected-node-id")).toHaveTextContent("");
    });

    it("does not surface reroute nodes as selected (only hat nodes are editable)", () => {
      const nodes: Node[] = [
        { id: "r1", type: "reroute", position: { x: 0, y: 0 }, data: {} },
      ];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onNodesChange?.([{ type: "select", id: "r1", selected: true }])
      );

      expect(screen.getByTestId("selected-node-id")).toHaveTextContent("");
    });
  });

  describe("connecting nodes", () => {
    it("creates an edge using the sourceHandle as the event label", () => {
      const nodes = [
        makeHatNode("n1", { name: "Planner", publishes: ["build.task"] }),
        makeHatNode("n2", { name: "Builder", triggersOn: ["build.task"] }),
      ];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onConnect?.({
          source: "n1",
          target: "n2",
          sourceHandle: "build.task",
          targetHandle: "build.task",
        })
      );

      const edges = rfState.latest?.edges ?? [];
      expect(edges).toHaveLength(1);
      expect(edges[0].source).toBe("n1");
      expect(edges[0].target).toBe("n2");
      expect(edges[0].label).toBe("build.task");
      expect(edges[0].type).toBe("offset");
    });

    it("falls back to 'event' label when sourceHandle is missing", () => {
      const nodes = [makeHatNode("n1"), makeHatNode("n2")];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onConnect?.({
          source: "n1",
          target: "n2",
          sourceHandle: null,
          targetHandle: null,
        })
      );

      expect(rfState.latest?.edges[0].label).toBe("event");
    });

    it("resolves the event label from the upstream edge when connecting through a reroute", () => {
      const nodes: Node[] = [
        makeHatNode("n1", { name: "Planner" }),
        { id: "r1", type: "reroute", position: { x: 0, y: 0 }, data: {} },
        makeHatNode("n2", { name: "Builder" }),
      ];
      const edges: Edge[] = [
        // Existing edge feeding the reroute carries the real event
        { id: "e-in", source: "n1", target: "r1", label: "build.task", type: "offset" },
      ];
      renderBuilder({ initialData: { nodes, edges } });

      // Now connect reroute -> n2 (sourceHandle is the synthetic 'default-out')
      runInAct(() =>
        rfState.latest?.onConnect?.({
          source: "r1",
          target: "n2",
          sourceHandle: "default-out",
          targetHandle: null,
        })
      );

      const newEdge = rfState.latest?.edges.find((e) => e.source === "r1" && e.target === "n2");
      expect(newEdge).toBeDefined();
      expect(newEdge!.label).toBe("build.task");
    });

    it("labels the connection 'event' when connecting from a reroute with no upstream", () => {
      const nodes: Node[] = [
        { id: "r1", type: "reroute", position: { x: 0, y: 0 }, data: {} },
        makeHatNode("n2"),
      ];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onConnect?.({
          source: "r1",
          target: "n2",
          sourceHandle: "default-out",
          targetHandle: null,
        })
      );

      expect(rfState.latest?.edges[0].label).toBe("event");
    });
  });

  describe("reorder / move handling", () => {
    it("passes position changes through to ReactFlow's applyNodeChanges", () => {
      const nodes = [makeHatNode("n1", { name: "Planner" })];
      renderBuilder({ initialData: { nodes, edges: [] } });

      runInAct(() =>
        rfState.latest?.onNodesChange?.([
          { type: "position", id: "n1", position: { x: 500, y: 250 } },
        ])
      );

      const moved = rfState.latest?.nodes.find((n) => n.id === "n1");
      expect(moved!.position).toEqual({ x: 500, y: 250 });
    });

    it("passes edge changes through to ReactFlow's applyEdgeChanges", () => {
      const nodes = [makeHatNode("n1"), makeHatNode("n2")];
      const edges: Edge[] = [{ id: "e1", source: "n1", target: "n2", label: "x" }];
      renderBuilder({ initialData: { nodes, edges } });

      runInAct(() =>
        rfState.latest?.onEdgesChange?.([{ type: "remove", id: "e1" }])
      );

      expect(rfState.latest?.edges).toEqual([]);
    });
  });
});
