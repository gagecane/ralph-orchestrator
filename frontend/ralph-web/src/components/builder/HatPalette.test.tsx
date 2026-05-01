/**
 * HatPalette Component Tests
 *
 * Tests for the sidebar palette that displays draggable hat templates:
 * - Preset template listing
 * - Search/filter functionality
 * - Collapse/expand state
 * - Draggable templates set correct dataTransfer payload
 * - Reroute utility draggable
 */

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { HatPalette } from "./HatPalette";

function makeDragEvent() {
  const setData = vi.fn();
  const event = {
    dataTransfer: {
      setData,
      get effectAllowed() {
        return this._effectAllowed;
      },
      set effectAllowed(value: string) {
        this._effectAllowed = value;
      },
      _effectAllowed: "",
    },
  };
  return { event, setData };
}

describe("HatPalette", () => {
  describe("rendering templates", () => {
    it("renders all preset hat templates by default", () => {
      render(<HatPalette />);

      expect(screen.getByText("Planner")).toBeInTheDocument();
      expect(screen.getByText("Builder")).toBeInTheDocument();
      expect(screen.getByText("Reviewer")).toBeInTheDocument();
      expect(screen.getByText("Validator")).toBeInTheDocument();
      expect(screen.getByText("Confessor")).toBeInTheDocument();
      expect(screen.getByText("Custom Hat")).toBeInTheDocument();
    });

    it("renders the palette title", () => {
      render(<HatPalette />);

      expect(screen.getByText("Hat Palette")).toBeInTheDocument();
    });

    it("renders the helper text for drag-and-drop", () => {
      render(<HatPalette />);

      expect(
        screen.getByText("Drag a hat template onto the canvas to add it"),
      ).toBeInTheDocument();
    });

    it("renders description text for templates", () => {
      render(<HatPalette />);

      expect(
        screen.getByText("Analyzes tasks and creates implementation plans"),
      ).toBeInTheDocument();
      expect(
        screen.getByText("Implements code, runs tests, creates commits"),
      ).toBeInTheDocument();
    });

    it("shows input/output count badges for templates with triggers/publishes", () => {
      render(<HatPalette />);

      // Planner has 2 triggers, 1 publish
      expect(screen.getByText("2 in")).toBeInTheDocument();
      // Multiple templates have 1 out
      expect(screen.getAllByText("1 out").length).toBeGreaterThan(0);
    });

    it("renders the reroute utility item", () => {
      render(<HatPalette />);

      expect(screen.getByText("Reroute")).toBeInTheDocument();
      expect(
        screen.getByText("Waypoint for connection routing"),
      ).toBeInTheDocument();
    });
  });

  describe("search filter", () => {
    it("renders a search input", () => {
      render(<HatPalette />);

      expect(
        screen.getByPlaceholderText("Search templates..."),
      ).toBeInTheDocument();
    });

    it("filters templates by name", () => {
      render(<HatPalette />);
      const searchInput = screen.getByPlaceholderText("Search templates...");

      fireEvent.change(searchInput, { target: { value: "plan" } });

      expect(screen.getByText("Planner")).toBeInTheDocument();
      expect(screen.queryByText("Builder")).not.toBeInTheDocument();
      expect(screen.queryByText("Reviewer")).not.toBeInTheDocument();
    });

    it("filters templates by description", () => {
      render(<HatPalette />);
      const searchInput = screen.getByPlaceholderText("Search templates...");

      fireEvent.change(searchInput, { target: { value: "implements code" } });

      expect(screen.getByText("Builder")).toBeInTheDocument();
      expect(screen.queryByText("Planner")).not.toBeInTheDocument();
    });

    it("is case insensitive", () => {
      render(<HatPalette />);
      const searchInput = screen.getByPlaceholderText("Search templates...");

      fireEvent.change(searchInput, { target: { value: "PLANNER" } });

      expect(screen.getByText("Planner")).toBeInTheDocument();
    });

    it("shows 'No matching templates' when no results", () => {
      render(<HatPalette />);
      const searchInput = screen.getByPlaceholderText("Search templates...");

      fireEvent.change(searchInput, { target: { value: "xyzzy-no-match" } });

      expect(screen.getByText("No matching templates")).toBeInTheDocument();
    });

    it("restores all templates when search is cleared", () => {
      render(<HatPalette />);
      const searchInput = screen.getByPlaceholderText("Search templates...");

      fireEvent.change(searchInput, { target: { value: "plan" } });
      expect(screen.queryByText("Builder")).not.toBeInTheDocument();

      fireEvent.change(searchInput, { target: { value: "" } });
      expect(screen.getByText("Builder")).toBeInTheDocument();
      expect(screen.getByText("Planner")).toBeInTheDocument();
    });
  });

  describe("collapse/expand", () => {
    it("renders expanded by default", () => {
      render(<HatPalette />);

      expect(screen.getByText("Hat Palette")).toBeInTheDocument();
      expect(
        screen.getByPlaceholderText("Search templates..."),
      ).toBeInTheDocument();
    });

    it("collapses when collapse button is clicked", () => {
      render(<HatPalette />);

      // Find collapse button (first button with chevron-left icon)
      const buttons = screen.getAllByRole("button");
      const collapseButton = buttons.find((btn) =>
        btn.querySelector(".lucide-chevron-left"),
      );
      expect(collapseButton).toBeDefined();

      fireEvent.click(collapseButton!);

      // After collapsing, "HAT PALETTE" vertical label should appear
      expect(screen.getByText("HAT PALETTE")).toBeInTheDocument();
      // Search input no longer rendered
      expect(
        screen.queryByPlaceholderText("Search templates..."),
      ).not.toBeInTheDocument();
    });

    it("expands when expand button is clicked while collapsed", () => {
      render(<HatPalette />);

      // Collapse first
      const buttons = screen.getAllByRole("button");
      const collapseButton = buttons.find((btn) =>
        btn.querySelector(".lucide-chevron-left"),
      );
      fireEvent.click(collapseButton!);

      // Now expand
      const expandButton = screen.getByRole("button");
      fireEvent.click(expandButton);

      expect(screen.getByText("Hat Palette")).toBeInTheDocument();
      expect(
        screen.getByPlaceholderText("Search templates..."),
      ).toBeInTheDocument();
    });
  });

  describe("drag interaction", () => {
    it("sets reactflow dataTransfer payload on template drag", () => {
      const { container } = render(<HatPalette />);

      const plannerItem = screen.getByText("Planner").closest('[draggable="true"]');
      expect(plannerItem).toBeInTheDocument();

      const { event, setData } = makeDragEvent();
      fireEvent.dragStart(plannerItem!, event);

      expect(setData).toHaveBeenCalledWith(
        "application/reactflow",
        expect.stringContaining('"key":"planner"'),
      );

      // Payload must parse to HatNodeData with expected fields
      const payload = JSON.parse(setData.mock.calls[0][1]);
      expect(payload).toMatchObject({
        key: "planner",
        name: "Planner",
        triggersOn: expect.arrayContaining(["work.start"]),
        publishes: expect.arrayContaining(["build.task"]),
      });

      // container keeps tsc happy (unused var avoidance)
      expect(container).toBeDefined();
    });

    it("sets reroute dataTransfer payload on reroute drag", () => {
      render(<HatPalette />);

      const rerouteItem = screen.getByText("Reroute").closest('[draggable="true"]');
      expect(rerouteItem).toBeInTheDocument();

      const { event, setData } = makeDragEvent();
      fireEvent.dragStart(rerouteItem!, event);

      expect(setData).toHaveBeenCalledWith("application/reroute", "true");
    });

    it("custom template drags with empty triggers/publishes arrays", () => {
      render(<HatPalette />);

      const customItem = screen.getByText("Custom Hat").closest('[draggable="true"]');

      const { event, setData } = makeDragEvent();
      fireEvent.dragStart(customItem!, event);

      const payload = JSON.parse(setData.mock.calls[0][1]);
      expect(payload.triggersOn).toEqual([]);
      expect(payload.publishes).toEqual([]);
    });
  });

  describe("className prop", () => {
    it("applies custom className to the palette card", () => {
      const { container } = render(<HatPalette className="extra-class" />);

      expect(container.querySelector(".extra-class")).toBeInTheDocument();
    });

    it("applies custom className when collapsed", () => {
      const { container } = render(<HatPalette className="extra-class" />);

      const buttons = screen.getAllByRole("button");
      const collapseButton = buttons.find((btn) =>
        btn.querySelector(".lucide-chevron-left"),
      );
      fireEvent.click(collapseButton!);

      expect(container.querySelector(".extra-class")).toBeInTheDocument();
    });
  });
});
