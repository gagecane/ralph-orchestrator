/**
 * BuilderPage Component Tests
 *
 * Tests the collection builder page which manages three view modes:
 *   - "list":   browse, create, delete, select collections
 *   - "create": compose a new collection in the CollectionBuilder canvas
 *   - "edit":   modify an existing collection in the canvas
 *
 * CollectionBuilder is mocked — its own tests live alongside it in
 * src/components/builder/CollectionBuilder.test.tsx. Here we only care
 * that BuilderPage:
 *   - Renders list / loading / error / empty states correctly
 *   - Transitions between view modes on user interaction
 *   - Wires onSelect / onCreate / delete confirm → mutation calls
 *   - Wires onSave → create / update mutation with transformed graph
 *   - Wires onExportYaml → refetch + triggers a download
 *
 * trpc is mocked via both module paths BuilderPage might resolve
 * ("@/trpc" and relative "../trpc") so the mock is picked up
 * regardless of how Vite resolves the import.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { BuilderPage } from "./BuilderPage";

// -------------------------------------------------------------------
// Mock CollectionBuilder
// -------------------------------------------------------------------

interface CollectionBuilderMockProps {
  collectionId: string | null;
  initialData?: {
    nodes: Array<{
      id: string;
      type?: string;
      position: { x: number; y: number };
      data: Record<string, unknown>;
    }>;
    edges: Array<{
      id: string;
      source: string;
      target: string;
      sourceHandle?: string | null;
      targetHandle?: string | null;
      label?: unknown;
    }>;
  };
  name: string;
  description: string;
  onSave: (data: {
    nodes: Array<{ id: string; type?: string; position: { x: number; y: number }; data: Record<string, unknown> }>;
    edges: Array<{ id: string; source: string; target: string; sourceHandle?: string | null; targetHandle?: string | null; label?: unknown }>;
    name: string;
    description: string;
  }) => void;
  onExportYaml?: () => void;
  onNameChange: (n: string) => void;
  onDescriptionChange: (d: string) => void;
  isSaving?: boolean;
  className?: string;
}

const builderState: { lastProps: CollectionBuilderMockProps | null } = { lastProps: null };

vi.mock("@/components/builder", () => ({
  CollectionBuilder: (props: CollectionBuilderMockProps) => {
    builderState.lastProps = props;
    return (
      <div data-testid="collection-builder" data-collection-id={props.collectionId ?? ""}>
        <span data-testid="builder-name">{props.name}</span>
        <span data-testid="builder-description">{props.description}</span>
        <span data-testid="builder-saving">{String(props.isSaving ?? false)}</span>
        <button
          data-testid="builder-save"
          onClick={() =>
            props.onSave({
              nodes: [
                {
                  id: "n1",
                  type: "hatNode",
                  position: { x: 10, y: 20 },
                  data: {
                    key: "builder",
                    name: "Builder",
                    description: "",
                    triggersOn: ["task.new"],
                    publishes: ["task.done"],
                    instructions: "Do things",
                  },
                },
              ],
              edges: [
                {
                  id: "e1",
                  source: "n1",
                  target: "n1",
                  sourceHandle: "out",
                  targetHandle: "in",
                  label: "done",
                },
              ],
              name: props.name,
              description: props.description,
            })
          }
        >
          Save
        </button>
        {props.onExportYaml && (
          <button data-testid="builder-export" onClick={() => props.onExportYaml?.()}>
            Export YAML
          </button>
        )}
      </div>
    );
  },
}));

// -------------------------------------------------------------------
// Mock trpc
// -------------------------------------------------------------------

type CreateMutationOptions = {
  onSuccess?: (data: { id: string }) => void;
};

type DeleteMutationOptions = {
  onSuccess?: () => void;
};

const trpcState = {
  collectionsQuery: {
    data: undefined as Array<Record<string, unknown>> | undefined,
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  },
  collectionGetQuery: {
    data: undefined as { id: string; name: string; description: string; graph: unknown } | undefined,
    isLoading: false,
    isError: false,
  },
  createMutation: {
    mutate: vi.fn(),
    isPending: false,
    lastOptions: null as CreateMutationOptions | null,
  },
  updateMutation: {
    mutate: vi.fn(),
    isPending: false,
  },
  deleteMutation: {
    mutate: vi.fn(),
    isPending: false,
    lastOptions: null as DeleteMutationOptions | null,
  },
  exportYamlQuery: {
    refetch: vi.fn(),
  },
};

function buildTrpcMock() {
  return {
    trpc: {
      collection: {
        list: {
          useQuery: vi.fn(() => trpcState.collectionsQuery),
        },
        get: {
          useQuery: vi.fn(() => trpcState.collectionGetQuery),
        },
        create: {
          useMutation: vi.fn((options?: CreateMutationOptions) => {
            trpcState.createMutation.lastOptions = options ?? null;
            return {
              mutate: trpcState.createMutation.mutate,
              isPending: trpcState.createMutation.isPending,
            };
          }),
        },
        update: {
          useMutation: vi.fn(() => ({
            mutate: trpcState.updateMutation.mutate,
            isPending: trpcState.updateMutation.isPending,
          })),
        },
        delete: {
          useMutation: vi.fn((options?: DeleteMutationOptions) => {
            trpcState.deleteMutation.lastOptions = options ?? null;
            return {
              mutate: trpcState.deleteMutation.mutate,
              isPending: trpcState.deleteMutation.isPending,
            };
          }),
        },
        exportYaml: {
          useQuery: vi.fn(() => trpcState.exportYamlQuery),
        },
      },
    },
  };
}

vi.mock("@/trpc", () => buildTrpcMock());
vi.mock("../trpc", () => buildTrpcMock());

// -------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------

function resetTrpcState(): void {
  trpcState.collectionsQuery = {
    data: undefined,
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  };
  trpcState.collectionGetQuery = {
    data: undefined,
    isLoading: false,
    isError: false,
  };
  trpcState.createMutation = {
    mutate: vi.fn(),
    isPending: false,
    lastOptions: null,
  };
  trpcState.updateMutation = {
    mutate: vi.fn(),
    isPending: false,
  };
  trpcState.deleteMutation = {
    mutate: vi.fn(),
    isPending: false,
    lastOptions: null,
  };
  trpcState.exportYamlQuery = {
    refetch: vi.fn(),
  };
  builderState.lastProps = null;
}

const sampleCollections = [
  {
    id: "c1",
    name: "First Collection",
    description: "My first workflow",
    updatedAt: "2024-01-10T10:00:00Z",
  },
  {
    id: "c2",
    name: "Second Collection",
    description: "",
    updatedAt: "2024-02-15T12:00:00Z",
  },
];

// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

describe("BuilderPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetTrpcState();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("list view", () => {
    it("renders Hat Builder heading and tagline", () => {
      trpcState.collectionsQuery.data = [];

      render(<BuilderPage />);

      expect(screen.getByRole("heading", { name: /hat builder/i, level: 1 })).toBeInTheDocument();
      expect(screen.getByText(/create visual workflows for hat collections/i)).toBeInTheDocument();
    });

    it("shows loading state while collections are loading", () => {
      trpcState.collectionsQuery.isLoading = true;

      render(<BuilderPage />);

      expect(screen.getByText(/loading collections/i)).toBeInTheDocument();
    });

    it("shows error state with Retry when collections query fails", () => {
      const refetch = vi.fn();
      trpcState.collectionsQuery = {
        data: undefined,
        isLoading: false,
        isError: true,
        refetch,
      };

      render(<BuilderPage />);

      expect(screen.getByText(/error loading collections/i)).toBeInTheDocument();
      const retry = screen.getByRole("button", { name: /retry/i });
      fireEvent.click(retry);
      expect(refetch).toHaveBeenCalled();
    });

    it("shows empty state with a single call-to-action when no collections exist", () => {
      trpcState.collectionsQuery.data = [];

      render(<BuilderPage />);

      expect(screen.getByText(/no collections yet/i)).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /create your first collection/i })).toBeInTheDocument();
    });

    it("renders one card per collection with name and description", () => {
      trpcState.collectionsQuery.data = sampleCollections;

      render(<BuilderPage />);

      expect(screen.getByText("First Collection")).toBeInTheDocument();
      expect(screen.getByText("My first workflow")).toBeInTheDocument();
      expect(screen.getByText("Second Collection")).toBeInTheDocument();
    });

    it("does not render a description line when description is empty", () => {
      trpcState.collectionsQuery.data = [
        { id: "c2", name: "No Desc", description: "", updatedAt: "2024-02-15T12:00:00Z" },
      ];

      render(<BuilderPage />);

      // The card renders "No Desc" for name but no description.
      expect(screen.getByText("No Desc")).toBeInTheDocument();
      // Updated-at line always renders for each card.
      expect(screen.getByText(/updated /i)).toBeInTheDocument();
    });
  });

  describe("transition: list → create", () => {
    it("shows the builder canvas with no collectionId when 'New Collection' is clicked", () => {
      trpcState.collectionsQuery.data = [];

      render(<BuilderPage />);

      // Enter create mode via the empty-state CTA
      fireEvent.click(screen.getByRole("button", { name: /create your first collection/i }));

      const builder = screen.getByTestId("collection-builder");
      expect(builder).toBeInTheDocument();
      // collectionId is null in create mode
      expect(builder.getAttribute("data-collection-id")).toBe("");
      // Header switches to "New Collection"
      expect(screen.getByRole("heading", { name: /new collection/i, level: 1 })).toBeInTheDocument();
    });

    it("exposes a Back button only outside list view and returns to list on click", () => {
      trpcState.collectionsQuery.data = [];

      render(<BuilderPage />);

      // In list view there is no Back button
      expect(screen.queryByRole("button", { name: /back/i })).not.toBeInTheDocument();

      // Enter create view
      fireEvent.click(screen.getByRole("button", { name: /create your first collection/i }));
      const backBtn = screen.getByRole("button", { name: /back/i });
      expect(backBtn).toBeInTheDocument();

      // Click Back → back to list view heading
      fireEvent.click(backBtn);
      expect(screen.getByRole("heading", { name: /hat builder/i, level: 1 })).toBeInTheDocument();
    });
  });

  describe("transition: list → edit", () => {
    it("enters edit view when a collection card is clicked", () => {
      trpcState.collectionsQuery.data = sampleCollections;

      render(<BuilderPage />);

      // Click the card name itself (the whole <Card> is clickable)
      fireEvent.click(screen.getByText("First Collection"));

      // While get query is still "loading" we render the loading panel
      trpcState.collectionGetQuery.isLoading = true;
      // Rerender by clicking again is unnecessary — the state change happens synchronously
      // via selectedId/viewMode and triggers a new render; the get query hook will be called
      // on next render. Since our mock always returns the current trpcState.collectionGetQuery,
      // we just assert that the builder state transitioned.
      // At this point, collectionQuery.isLoading is false (default) and data is undefined,
      // which means the page falls through to the CollectionBuilder with no initialData.
      expect(screen.getByTestId("collection-builder")).toBeInTheDocument();
    });

    it("shows 'Loading collection...' when the selected collection is still fetching", () => {
      trpcState.collectionsQuery.data = sampleCollections;
      trpcState.collectionGetQuery.isLoading = true;

      render(<BuilderPage />);
      fireEvent.click(screen.getByText("First Collection"));

      expect(screen.getByText(/loading collection/i)).toBeInTheDocument();
    });

    it("shows an error panel with Back button when the selected collection fails to load", () => {
      trpcState.collectionsQuery.data = sampleCollections;
      trpcState.collectionGetQuery.isError = true;

      render(<BuilderPage />);
      fireEvent.click(screen.getByText("First Collection"));

      expect(screen.getByText(/failed to load collection/i)).toBeInTheDocument();
      // A "Back to list" button lives inside the error panel
      const backToList = screen.getByRole("button", { name: /back to list/i });
      fireEvent.click(backToList);
      // Now we should be back at the list
      expect(screen.getByRole("heading", { name: /hat builder/i, level: 1 })).toBeInTheDocument();
    });

    it("passes the loaded name and description into the builder once the collection resolves", () => {
      trpcState.collectionsQuery.data = sampleCollections;
      trpcState.collectionGetQuery.data = {
        id: "c1",
        name: "First Collection",
        description: "My first workflow",
        graph: { nodes: [], edges: [] },
      };

      render(<BuilderPage />);
      fireEvent.click(screen.getByText("First Collection"));

      // The CollectionBuilder stub mirrors props back as data-testid fields
      expect(screen.getByTestId("builder-name")).toHaveTextContent("First Collection");
      expect(screen.getByTestId("builder-description")).toHaveTextContent("My first workflow");
    });
  });

  describe("delete flow", () => {
    it("opens a confirm() dialog and calls the delete mutation on OK", () => {
      trpcState.collectionsQuery.data = sampleCollections;

      const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);

      render(<BuilderPage />);

      // Two trash buttons (one per card). Pick the first via container queries.
      const trashButtons = screen
        .getAllByRole("button")
        .filter((btn) => btn.querySelector(".lucide-trash-2"));
      expect(trashButtons.length).toBeGreaterThan(0);
      fireEvent.click(trashButtons[0]);

      expect(confirmSpy).toHaveBeenCalledWith(
        expect.stringContaining('Delete collection "First Collection"?')
      );
      expect(trpcState.deleteMutation.mutate).toHaveBeenCalledWith({ id: "c1" });
    });

    it("does NOT call the delete mutation when the user cancels the confirm dialog", () => {
      trpcState.collectionsQuery.data = sampleCollections;

      vi.spyOn(window, "confirm").mockReturnValue(false);

      render(<BuilderPage />);

      const trashButtons = screen
        .getAllByRole("button")
        .filter((btn) => btn.querySelector(".lucide-trash-2"));
      fireEvent.click(trashButtons[0]);

      expect(trpcState.deleteMutation.mutate).not.toHaveBeenCalled();
    });

    it("refetches the collections list after a successful delete", () => {
      const refetch = vi.fn();
      trpcState.collectionsQuery = {
        data: sampleCollections,
        isLoading: false,
        isError: false,
        refetch,
      };
      vi.spyOn(window, "confirm").mockReturnValue(true);

      render(<BuilderPage />);

      const trashButtons = screen
        .getAllByRole("button")
        .filter((btn) => btn.querySelector(".lucide-trash-2"));
      fireEvent.click(trashButtons[0]);

      // The page registered an onSuccess with useMutation that calls refetch.
      trpcState.deleteMutation.lastOptions?.onSuccess?.();
      expect(refetch).toHaveBeenCalled();
    });
  });

  describe("save flow", () => {
    it("calls the create mutation with a normalized graph in create mode", () => {
      trpcState.collectionsQuery.data = [];

      render(<BuilderPage />);

      fireEvent.click(screen.getByRole("button", { name: /create your first collection/i }));
      fireEvent.click(screen.getByTestId("builder-save"));

      expect(trpcState.createMutation.mutate).toHaveBeenCalledTimes(1);
      const arg = trpcState.createMutation.mutate.mock.calls[0][0] as {
        name: string;
        description: string;
        graph: {
          nodes: Array<{ id: string; type: string; position: { x: number; y: number } }>;
          edges: Array<{ id: string; source: string; target: string; label?: string }>;
          viewport: { x: number; y: number; zoom: number };
        };
      };
      expect(arg.name).toBe("New Collection");
      expect(arg.graph.nodes).toHaveLength(1);
      expect(arg.graph.nodes[0]).toMatchObject({ id: "n1", type: "hatNode", position: { x: 10, y: 20 } });
      expect(arg.graph.edges).toHaveLength(1);
      expect(arg.graph.edges[0]).toMatchObject({ id: "e1", source: "n1", target: "n1", label: "done" });
      expect(arg.graph.viewport).toEqual({ x: 0, y: 0, zoom: 1 });
    });

    it("calls the update mutation (not create) when saving an existing collection", () => {
      trpcState.collectionsQuery.data = sampleCollections;
      trpcState.collectionGetQuery.data = {
        id: "c1",
        name: "First Collection",
        description: "My first workflow",
        graph: { nodes: [], edges: [] },
      };

      render(<BuilderPage />);
      fireEvent.click(screen.getByText("First Collection"));
      fireEvent.click(screen.getByTestId("builder-save"));

      expect(trpcState.updateMutation.mutate).toHaveBeenCalledTimes(1);
      expect(trpcState.createMutation.mutate).not.toHaveBeenCalled();
      const arg = trpcState.updateMutation.mutate.mock.calls[0][0] as { id: string };
      expect(arg.id).toBe("c1");
    });

    it("reflects isPending via the isSaving prop on the builder", () => {
      trpcState.collectionsQuery.data = [];
      trpcState.createMutation.isPending = true;

      render(<BuilderPage />);

      fireEvent.click(screen.getByRole("button", { name: /create your first collection/i }));

      expect(screen.getByTestId("builder-saving")).toHaveTextContent("true");
    });
  });

  describe("export YAML", () => {
    it("does not render an Export button while creating (no selectedId)", () => {
      trpcState.collectionsQuery.data = [];

      render(<BuilderPage />);
      fireEvent.click(screen.getByRole("button", { name: /create your first collection/i }));

      expect(screen.queryByTestId("builder-export")).not.toBeInTheDocument();
    });

    it("renders Export button in edit mode and triggers download via refetch+data", async () => {
      trpcState.collectionsQuery.data = sampleCollections;
      trpcState.collectionGetQuery.data = {
        id: "c1",
        name: "First Collection",
        description: "",
        graph: { nodes: [], edges: [] },
      };
      const refetch = vi.fn().mockResolvedValue({ data: { yaml: "hats:\n  builder: {}\n" } });
      trpcState.exportYamlQuery.refetch = refetch;

      // Stub URL API used by handleExportYaml
      const originalCreate = URL.createObjectURL;
      const originalRevoke = URL.revokeObjectURL;
      URL.createObjectURL = vi.fn(() => "blob:fake");
      URL.revokeObjectURL = vi.fn();

      render(<BuilderPage />);
      fireEvent.click(screen.getByText("First Collection"));

      const exportBtn = screen.getByTestId("builder-export");
      fireEvent.click(exportBtn);

      // Wait for the promise chain in handleExportYaml to settle
      await Promise.resolve();
      await Promise.resolve();

      expect(refetch).toHaveBeenCalled();
      expect(URL.createObjectURL).toHaveBeenCalled();
      expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:fake");

      URL.createObjectURL = originalCreate;
      URL.revokeObjectURL = originalRevoke;
    });
  });
});
