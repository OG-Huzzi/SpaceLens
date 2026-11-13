/**
 * The category tree is mounted lazily.
 *
 * The engine's category tree is recursive and uncapped. A screen that mounts
 * every descendant the moment it opens pays for the whole volume's shape
 * before the reader has asked for any of it. A collapsed branch must cost
 * nothing to display, so it must cost nothing to mount.
 *
 * These assertions are about DOM shape, which is machine-independent. They
 * look at the tree nodes themselves rather than at page text, because a
 * category's name appears in more than one place: the map, the list row and
 * the tree.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createMockApi } from "../api";
import { StorageScreen } from "./StorageScreen";

function makeApi() {
  return createMockApi({ latencyMs: 0 });
}

/** A top-level category of the shipped fixture, and one of its children. */
const PARENT = "Applications";
const CHILD = "Design suite";

/** Every category node currently mounted, outermost first. */
function mountedNodes(container: HTMLElement): Element[] {
  return Array.from(container.querySelectorAll("details.storage-node"));
}

describe("the storage category tree", () => {
  it("does not mount the children of a collapsed category", async () => {
    const { container } = render(<StorageScreen api={makeApi()} focusCategoryId={null} />);
    await screen.findByRole("heading", { name: "Categories" });

    const parent = mountedNodes(container).find((node) =>
      node.textContent?.includes(PARENT)
    );

    expect(parent, "the parent category must be mounted").toBeDefined();
    expect(
      parent?.textContent,
      "a collapsed category must not mount its children"
    ).not.toContain(CHILD);
  });

  it("mounts a matching child once the reader searches for it", async () => {
    const { container } = render(<StorageScreen api={makeApi()} focusCategoryId={null} />);
    await screen.findByRole("heading", { name: "Categories" });

    fireEvent.change(screen.getByRole("searchbox"), { target: { value: CHILD } });

    await screen.findByText(CHILD);
    expect(
      mountedNodes(container).some((node) => node.textContent?.includes(CHILD)),
      "the matching child must be mounted once it is searched for"
    ).toBe(true);
  });
});
