/**
 * PropertiesPanel Component Tests
 *
 * Tests for the properties editor panel for selected hat nodes:
 * - Empty-state when no selection
 * - Field editing for name, description, triggers, publishes, instructions
 * - TagEditor behavior (add, remove, backspace, task.* restriction)
 * - Delete node confirmation
 * - Collapse/expand state
 * - Sync of local state with selection changes
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { PropertiesPanel } from "./PropertiesPanel";
import type { HatNodeData } from "./HatNode";

function makeNode(overrides: Partial<HatNodeData> = {}): {
  id: string;
  data: HatNodeData;
} {
  return {
    id: "node-1",
    data: {
      key: "planner",
      name: "Planner",
      description: "Plans things",
      triggersOn: ["work.start"],
      publishes: ["build.task"],
      ...overrides,
    },
  };
}

describe("PropertiesPanel", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  describe("empty state", () => {
    it("renders instructional text when no node is selected", () => {
      render(
        <PropertiesPanel
          selectedNode={null}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      expect(
        screen.getByText("Select a hat on the canvas to edit its properties"),
      ).toBeInTheDocument();
    });

    it("does not render form fields when no node is selected", () => {
      render(
        <PropertiesPanel
          selectedNode={null}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      expect(screen.queryByDisplayValue("Planner")).not.toBeInTheDocument();
      expect(screen.queryByText("Delete Hat")).not.toBeInTheDocument();
    });
  });

  describe("form rendering", () => {
    it("renders all fields for a selected node", () => {
      render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      expect(screen.getByDisplayValue("planner")).toBeInTheDocument(); // key
      expect(screen.getByDisplayValue("Planner")).toBeInTheDocument(); // name
      expect(screen.getByDisplayValue("Plans things")).toBeInTheDocument(); // description
      expect(screen.getByText("work.start")).toBeInTheDocument(); // trigger tag
      expect(screen.getByText("build.task")).toBeInTheDocument(); // publish tag
    });

    it("renders the key as disabled (read-only)", () => {
      render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      const keyInput = screen.getByDisplayValue("planner");
      expect(keyInput).toBeDisabled();
    });

    it("renders instructions textarea even when value is undefined", () => {
      render(
        <PropertiesPanel
          selectedNode={makeNode({ instructions: undefined })}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      expect(
        screen.getByPlaceholderText("Optional instructions for this hat..."),
      ).toBeInTheDocument();
    });
  });

  describe("editing fields", () => {
    it("calls onUpdateNode when name is changed", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const nameInput = screen.getByDisplayValue("Planner");
      fireEvent.change(nameInput, { target: { value: "Super Planner" } });

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        name: "Super Planner",
      });
    });

    it("calls onUpdateNode when description is changed", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const descInput = screen.getByDisplayValue("Plans things");
      fireEvent.change(descInput, { target: { value: "Now plans more" } });

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        description: "Now plans more",
      });
    });

    it("calls onUpdateNode with undefined when instructions cleared", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ instructions: "Be thorough" })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const instructionsTextarea = screen.getByDisplayValue("Be thorough");
      fireEvent.change(instructionsTextarea, { target: { value: "" } });

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        instructions: undefined,
      });
    });
  });

  describe("TagEditor - adding tags", () => {
    it("adds a trigger tag when Enter is pressed", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ triggersOn: [] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const triggerInput = screen.getByPlaceholderText("Add trigger events...");
      fireEvent.change(triggerInput, { target: { value: "review.start" } });
      fireEvent.keyDown(triggerInput, { key: "Enter" });

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        triggersOn: ["review.start"],
      });
    });

    it("adds a publish tag when Enter is pressed", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ publishes: [] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const publishInput = screen.getByPlaceholderText("Add publish events...");
      fireEvent.change(publishInput, { target: { value: "review.done" } });
      fireEvent.keyDown(publishInput, { key: "Enter" });

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        publishes: ["review.done"],
      });
    });

    it("does not add duplicate tags", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ triggersOn: ["work.start"] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const triggerInput = screen.getByPlaceholderText("Add trigger events...");
      fireEvent.change(triggerInput, { target: { value: "work.start" } });
      fireEvent.keyDown(triggerInput, { key: "Enter" });

      expect(onUpdateNode).not.toHaveBeenCalled();
    });

    it("trims whitespace from tag input", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ triggersOn: [] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const triggerInput = screen.getByPlaceholderText("Add trigger events...");
      fireEvent.change(triggerInput, { target: { value: "  review.start  " } });
      fireEvent.keyDown(triggerInput, { key: "Enter" });

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        triggersOn: ["review.start"],
      });
    });

    it("does not add empty tags", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ triggersOn: [] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const triggerInput = screen.getByPlaceholderText("Add trigger events...");
      fireEvent.change(triggerInput, { target: { value: "   " } });
      fireEvent.keyDown(triggerInput, { key: "Enter" });

      expect(onUpdateNode).not.toHaveBeenCalled();
    });

    it("shows error and blocks task.* triggers (reserved for orchestrator)", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ triggersOn: [] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const triggerInput = screen.getByPlaceholderText("Add trigger events...");
      fireEvent.change(triggerInput, { target: { value: "task.done" } });
      fireEvent.keyDown(triggerInput, { key: "Enter" });

      expect(onUpdateNode).not.toHaveBeenCalled();
      expect(
        screen.getByText("task.* events are reserved for the orchestrator"),
      ).toBeInTheDocument();
    });

    it("allows task.* prefix in publishes (output is not restricted)", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ publishes: [] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const publishInput = screen.getByPlaceholderText("Add publish events...");
      fireEvent.change(publishInput, { target: { value: "task.custom" } });
      fireEvent.keyDown(publishInput, { key: "Enter" });

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        publishes: ["task.custom"],
      });
    });
  });

  describe("TagEditor - removing tags", () => {
    it("removes a tag when its X button is clicked", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ triggersOn: ["a", "b", "c"] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      // Find the "b" tag and its remove button
      const bTag = screen.getByText("b");
      const removeButton = bTag.querySelector("button");
      expect(removeButton).toBeInTheDocument();

      fireEvent.click(removeButton!);

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        triggersOn: ["a", "c"],
      });
    });

    it("removes last tag on Backspace when input is empty", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ triggersOn: ["a", "b"] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const triggerInput = screen.getByPlaceholderText("Add trigger events...");
      // Input is empty; Backspace should remove last tag
      fireEvent.keyDown(triggerInput, { key: "Backspace" });

      expect(onUpdateNode).toHaveBeenCalledWith("node-1", {
        triggersOn: ["a"],
      });
    });

    it("does not remove tag on Backspace when input has text", () => {
      const onUpdateNode = vi.fn();

      render(
        <PropertiesPanel
          selectedNode={makeNode({ triggersOn: ["a", "b"] })}
          onUpdateNode={onUpdateNode}
          onDeleteNode={vi.fn()}
        />,
      );

      const triggerInput = screen.getByPlaceholderText("Add trigger events...");
      fireEvent.change(triggerInput, { target: { value: "typing" } });
      fireEvent.keyDown(triggerInput, { key: "Backspace" });

      expect(onUpdateNode).not.toHaveBeenCalled();
    });
  });

  describe("delete node", () => {
    it("calls onDeleteNode when user confirms delete dialog", () => {
      const onDeleteNode = vi.fn();
      vi.spyOn(window, "confirm").mockReturnValue(true);

      render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={vi.fn()}
          onDeleteNode={onDeleteNode}
        />,
      );

      fireEvent.click(screen.getByText("Delete Hat"));

      expect(window.confirm).toHaveBeenCalled();
      expect(onDeleteNode).toHaveBeenCalledWith("node-1");
    });

    it("does NOT call onDeleteNode when user cancels delete dialog", () => {
      const onDeleteNode = vi.fn();
      vi.spyOn(window, "confirm").mockReturnValue(false);

      render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={vi.fn()}
          onDeleteNode={onDeleteNode}
        />,
      );

      fireEvent.click(screen.getByText("Delete Hat"));

      expect(window.confirm).toHaveBeenCalled();
      expect(onDeleteNode).not.toHaveBeenCalled();
    });
  });

  describe("collapse/expand", () => {
    it("collapses when collapse button is clicked", () => {
      render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      const buttons = screen.getAllByRole("button");
      const collapseBtn = buttons.find((btn) =>
        btn.querySelector(".lucide-chevron-right"),
      );
      expect(collapseBtn).toBeDefined();

      fireEvent.click(collapseBtn!);

      // Collapsed vertical label
      expect(screen.getByText("PROPERTIES")).toBeInTheDocument();
      // Form no longer rendered
      expect(screen.queryByDisplayValue("Planner")).not.toBeInTheDocument();
    });

    it("expands when expand button is clicked in collapsed mode", () => {
      render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      // Collapse
      const initialButtons = screen.getAllByRole("button");
      const collapseBtn = initialButtons.find((btn) =>
        btn.querySelector(".lucide-chevron-right"),
      );
      fireEvent.click(collapseBtn!);

      // Expand
      const expandBtn = screen.getByRole("button");
      fireEvent.click(expandBtn);

      expect(screen.getByDisplayValue("Planner")).toBeInTheDocument();
    });
  });

  describe("selection changes", () => {
    it("resets form when selection changes to a different node", () => {
      const { rerender } = render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      expect(screen.getByDisplayValue("Planner")).toBeInTheDocument();

      rerender(
        <PropertiesPanel
          selectedNode={{
            id: "node-2",
            data: {
              key: "builder",
              name: "Builder",
              description: "Builds things",
              triggersOn: [],
              publishes: [],
            },
          }}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      expect(screen.getByDisplayValue("Builder")).toBeInTheDocument();
      expect(screen.queryByDisplayValue("Planner")).not.toBeInTheDocument();
    });

    it("shows empty state when selection is cleared", () => {
      const { rerender } = render(
        <PropertiesPanel
          selectedNode={makeNode()}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      expect(screen.getByDisplayValue("Planner")).toBeInTheDocument();

      rerender(
        <PropertiesPanel
          selectedNode={null}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
        />,
      );

      expect(
        screen.getByText("Select a hat on the canvas to edit its properties"),
      ).toBeInTheDocument();
    });
  });

  describe("className prop", () => {
    it("applies custom className to the panel", () => {
      const { container } = render(
        <PropertiesPanel
          selectedNode={null}
          onUpdateNode={vi.fn()}
          onDeleteNode={vi.fn()}
          className="extra-class"
        />,
      );

      expect(container.querySelector(".extra-class")).toBeInTheDocument();
    });
  });
});
