/**
 * SettingsPage Component Tests
 *
 * Tests the ralph.yml settings editor page. Covers:
 *  - Initial render with loading, error, and data states
 *  - Dirty-state tracking and "Unsaved changes" badge
 *  - Reset button behavior
 *  - Save flow (mutation fired, success/error banner, refetch)
 *  - Hat collection dropdown population from presets
 *
 * trpc is mocked at the module boundary so we can drive each state
 * independently without wiring up a real TRPC / react-query provider.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { SettingsPage } from "./SettingsPage";

// -------------------------------------------------------------------
// Mock trpc
//
// SettingsPage uses these procedures:
//   - trpc.config.get.useQuery()
//   - trpc.presets.list.useQuery()
//   - trpc.config.update.useMutation({ onSuccess, onError })
//
// We expose the update mutation's last options so tests can trigger
// onSuccess / onError to exercise the post-save UI.
// -------------------------------------------------------------------

type UpdateMutationOptions = {
  onSuccess?: () => void;
  onError?: () => void;
};

const mutationState: {
  lastOptions: UpdateMutationOptions | null;
  mutate: ReturnType<typeof vi.fn>;
  isPending: boolean;
  isError: boolean;
  error: { message: string } | null;
} = {
  lastOptions: null,
  mutate: vi.fn(),
  isPending: false,
  isError: false,
  error: null,
};

vi.mock("@/trpc", () => ({
  trpc: {
    config: {
      get: {
        useQuery: vi.fn(),
      },
      update: {
        useMutation: vi.fn((options?: UpdateMutationOptions) => {
          mutationState.lastOptions = options ?? null;
          return {
            mutate: mutationState.mutate,
            isPending: mutationState.isPending,
            isError: mutationState.isError,
            error: mutationState.error,
          };
        }),
      },
    },
    presets: {
      list: {
        useQuery: vi.fn(),
      },
    },
  },
}));

// Also mock via the page's relative import path used inside the component.
// BuilderPage imports from "../trpc" — we mock that too for consistency.
vi.mock("../trpc", () => ({
  trpc: {
    config: {
      get: {
        useQuery: vi.fn(),
      },
      update: {
        useMutation: vi.fn((options?: UpdateMutationOptions) => {
          mutationState.lastOptions = options ?? null;
          return {
            mutate: mutationState.mutate,
            isPending: mutationState.isPending,
            isError: mutationState.isError,
            error: mutationState.error,
          };
        }),
      },
    },
    presets: {
      list: {
        useQuery: vi.fn(),
      },
    },
  },
}));

// -------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------

const SAMPLE_YAML = `# Ralph configuration
backend: claude
hats:
  builder:
    name: "Builder"
    triggers: ["task.new"]
`;

interface SetupOpts {
  configQuery?: {
    data?: { raw: string; parsed?: Record<string, unknown> };
    isLoading?: boolean;
    isError?: boolean;
    error?: { message: string };
    refetch?: () => void;
  };
  presetsQuery?: {
    data?: Array<{ id: string; name: string; source: string }>;
    isLoading?: boolean;
  };
  mutation?: Partial<typeof mutationState>;
}

async function setupMocks(opts: SetupOpts = {}): Promise<void> {
  const { trpc } = await import("@/trpc");

  const configDefaults = {
    data: { raw: SAMPLE_YAML, parsed: { hats: {} } },
    isLoading: false,
    isError: false,
    error: undefined,
    refetch: vi.fn(),
  };
  const presetsDefaults = {
    data: [] as Array<{ id: string; name: string; source: string }>,
    isLoading: false,
  };

  vi.mocked(trpc.config.get.useQuery).mockReturnValue({
    ...configDefaults,
    ...opts.configQuery,
  } as ReturnType<typeof trpc.config.get.useQuery>);

  vi.mocked(trpc.presets.list.useQuery).mockReturnValue({
    ...presetsDefaults,
    ...opts.presetsQuery,
  } as ReturnType<typeof trpc.presets.list.useQuery>);

  if (opts.mutation) {
    Object.assign(mutationState, opts.mutation);
  }
}

// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

describe("SettingsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mutationState.lastOptions = null;
    mutationState.mutate = vi.fn();
    mutationState.isPending = false;
    mutationState.isError = false;
    mutationState.error = null;
  });

  describe("page layout", () => {
    it("renders page header with title and ralph.yml badge", async () => {
      await setupMocks();

      render(<SettingsPage />);

      expect(screen.getByRole("heading", { name: /settings/i, level: 1 })).toBeInTheDocument();
      expect(screen.getByText("ralph.yml")).toBeInTheDocument();
      expect(screen.getByText(/configure your ralph orchestrator/i)).toBeInTheDocument();
    });

    it("renders Hat Collection section with label and dropdown", async () => {
      await setupMocks();

      render(<SettingsPage />);

      expect(screen.getByText("Hat Collection")).toBeInTheDocument();
      expect(screen.getByLabelText(/active collection/i)).toBeInTheDocument();
    });

    it("renders Configuration card with Save and Reset buttons", async () => {
      await setupMocks();

      render(<SettingsPage />);

      expect(screen.getByRole("button", { name: /save/i })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /reset/i })).toBeInTheDocument();
    });
  });

  describe("config query states", () => {
    it("shows loading indicator while config is loading", async () => {
      await setupMocks({
        configQuery: { data: undefined, isLoading: true, isError: false },
      });

      render(<SettingsPage />);

      expect(screen.getByText(/loading configuration/i)).toBeInTheDocument();
      // Editor textarea should not be present yet
      expect(screen.queryByPlaceholderText(/# ralph configuration/i)).not.toBeInTheDocument();
    });

    it("shows error state with Retry button when config query errors", async () => {
      const refetch = vi.fn();
      await setupMocks({
        configQuery: {
          data: undefined,
          isLoading: false,
          isError: true,
          error: { message: "Config file not found" },
          refetch,
        },
      });

      render(<SettingsPage />);

      expect(screen.getByText(/config file not found/i)).toBeInTheDocument();
      const retry = screen.getByRole("button", { name: /retry/i });
      expect(retry).toBeInTheDocument();

      // Clicking Retry should call refetch
      fireEvent.click(retry);
      expect(refetch).toHaveBeenCalled();
    });

    it("populates the textarea from configQuery.data.raw", async () => {
      await setupMocks();

      render(<SettingsPage />);

      const textarea = screen.getByPlaceholderText(/# ralph configuration/i) as HTMLTextAreaElement;
      expect(textarea).toBeInTheDocument();
      expect(textarea.value).toContain("backend: claude");
    });
  });

  describe("dirty-state tracking", () => {
    it("does not show Unsaved changes badge initially", async () => {
      await setupMocks();

      render(<SettingsPage />);

      expect(screen.queryByText(/unsaved changes/i)).not.toBeInTheDocument();
    });

    it("shows Unsaved changes badge after user edits the textarea", async () => {
      await setupMocks();

      render(<SettingsPage />);

      const textarea = screen.getByPlaceholderText(/# ralph configuration/i);
      fireEvent.change(textarea, { target: { value: "backend: kiro\n" } });

      expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();
    });

    it("enables Save and Reset buttons only when dirty", async () => {
      await setupMocks();

      render(<SettingsPage />);

      const saveBtn = screen.getByRole("button", { name: /save/i });
      const resetBtn = screen.getByRole("button", { name: /reset/i });

      // Initially clean → disabled
      expect(saveBtn).toBeDisabled();
      expect(resetBtn).toBeDisabled();

      // Edit → enabled
      const textarea = screen.getByPlaceholderText(/# ralph configuration/i);
      fireEvent.change(textarea, { target: { value: "changed: true\n" } });

      expect(saveBtn).not.toBeDisabled();
      expect(resetBtn).not.toBeDisabled();
    });

    it("Reset button restores original content and clears dirty state", async () => {
      await setupMocks();

      render(<SettingsPage />);

      const textarea = screen.getByPlaceholderText(/# ralph configuration/i) as HTMLTextAreaElement;

      // Edit
      fireEvent.change(textarea, { target: { value: "changed: true\n" } });
      expect(textarea.value).toBe("changed: true\n");
      expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();

      // Reset
      fireEvent.click(screen.getByRole("button", { name: /reset/i }));

      expect(textarea.value).toContain("backend: claude");
      expect(screen.queryByText(/unsaved changes/i)).not.toBeInTheDocument();
    });
  });

  describe("save flow", () => {
    it("invokes the update mutation with current content on Save click", async () => {
      await setupMocks();

      render(<SettingsPage />);

      const textarea = screen.getByPlaceholderText(/# ralph configuration/i);
      fireEvent.change(textarea, { target: { value: "backend: codex\n" } });

      fireEvent.click(screen.getByRole("button", { name: /save/i }));

      expect(mutationState.mutate).toHaveBeenCalledWith({ content: "backend: codex\n" });
    });

    it("shows Saving... label while mutation is pending", async () => {
      await setupMocks({ mutation: { isPending: true } });

      render(<SettingsPage />);

      // Edit to enter dirty state (button is still rendered; text reflects pending)
      const textarea = screen.getByPlaceholderText(/# ralph configuration/i);
      fireEvent.change(textarea, { target: { value: "x\n" } });

      expect(screen.getByRole("button", { name: /saving/i })).toBeInTheDocument();
    });

    it("shows Saved indicator after onSuccess fires, clears dirty state, and refetches", async () => {
      const refetch = vi.fn();
      await setupMocks({
        configQuery: {
          data: { raw: SAMPLE_YAML, parsed: { hats: {} } },
          isLoading: false,
          isError: false,
          refetch,
        },
      });

      render(<SettingsPage />);

      // Edit so Save is enabled
      fireEvent.change(screen.getByPlaceholderText(/# ralph configuration/i), {
        target: { value: "y\n" },
      });

      expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();

      // Trigger the onSuccess the page registered with useMutation.
      // Wrap in act() so React flushes the setState calls that onSuccess makes.
      act(() => {
        mutationState.lastOptions?.onSuccess?.();
      });

      // After onSuccess: "Saved" indicator appears, dirty flag cleared, refetch called
      expect(screen.getByText(/^saved$/i)).toBeInTheDocument();
      expect(screen.queryByText(/unsaved changes/i)).not.toBeInTheDocument();
      expect(refetch).toHaveBeenCalled();
    });

    it("shows Error saving indicator after onError fires", async () => {
      await setupMocks();

      render(<SettingsPage />);

      // Edit to dirty
      fireEvent.change(screen.getByPlaceholderText(/# ralph configuration/i), {
        target: { value: "z\n" },
      });

      act(() => {
        mutationState.lastOptions?.onError?.();
      });

      expect(screen.getByText(/error saving/i)).toBeInTheDocument();
    });

    it("shows inline mutation error message when mutation.isError is true", async () => {
      await setupMocks({
        mutation: {
          isError: true,
          error: { message: "YAML parse error at line 3" },
        },
      });

      render(<SettingsPage />);

      expect(screen.getByText(/yaml parse error at line 3/i)).toBeInTheDocument();
    });
  });

  describe("hat collection dropdown", () => {
    it("renders a 'Default (from config)' option when config has a hats block", async () => {
      await setupMocks({
        configQuery: {
          data: { raw: SAMPLE_YAML, parsed: { hats: { builder: {} } } },
          isLoading: false,
          isError: false,
        },
      });

      render(<SettingsPage />);

      const select = screen.getByLabelText(/active collection/i) as HTMLSelectElement;
      const options = Array.from(select.options).map((o) => o.textContent);
      expect(options).toContain("Default (from config)");
    });

    it("does NOT render 'Default (from config)' option when no hats block exists", async () => {
      await setupMocks({
        configQuery: {
          data: { raw: "backend: claude\n", parsed: {} },
          isLoading: false,
          isError: false,
        },
      });

      render(<SettingsPage />);

      expect(screen.queryByText(/default \(from config\)/i)).not.toBeInTheDocument();
    });

    it("lists each preset as an option with name and source", async () => {
      await setupMocks({
        presetsQuery: {
          data: [
            { id: "wave-review", name: "Wave Review", source: "builtin" },
            { id: "mine", name: "My Preset", source: "user" },
          ],
        },
      });

      render(<SettingsPage />);

      const select = screen.getByLabelText(/active collection/i) as HTMLSelectElement;
      const optionLabels = Array.from(select.options).map((o) => o.textContent);

      expect(optionLabels).toContain("Wave Review (builtin)");
      expect(optionLabels).toContain("My Preset (user)");
    });

    it("disables the dropdown while presets are loading", async () => {
      await setupMocks({
        presetsQuery: { data: [], isLoading: true },
      });

      render(<SettingsPage />);

      const select = screen.getByLabelText(/active collection/i) as HTMLSelectElement;
      expect(select).toBeDisabled();
    });
  });
});
