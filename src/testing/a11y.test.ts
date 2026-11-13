/**
 * Self-test for the accessibility harness.
 *
 * An enforcement suite is only worth its runtime if it *fails* when the UI is
 * broken. These tests build small, deliberately broken fragments and prove
 * that each rule fires — and that a correct fragment stays silent.
 *
 * Without this file, a harness that returned "no violations" for everything
 * would still make `src/a11y.test.tsx` pass.
 */

import { describe, expect, it } from "vitest";
import {
  accessibleName,
  axeViolations,
  describeElement,
  findFocusTrap,
  focusOrder,
  formatViolations,
  hasAccessibleName,
  tabbableElements
} from "./a11y";

function fragment(html: string): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = html;
  document.body.append(host);
  return host;
}

function ruleIds(violations: Awaited<ReturnType<typeof axeViolations>>): string[] {
  return violations.map((entry) => entry.id);
}

describe("the harness detects what it claims to detect", () => {
  it("flags a control with no accessible name", async () => {
    const host = fragment('<button><span aria-hidden="true">★</span></button>');
    const violations = await axeViolations(host);
    expect(ruleIds(violations)).toContain("interactive-name");
    expect(formatViolations(violations)).toContain("has no accessible name");
  });

  it("accepts a control named by aria-label, aria-labelledby, text or a label", async () => {
    const host = fragment(`
      <button aria-label="Close">×</button>
      <span id="label-text">Delete</span>
      <button aria-labelledby="label-text"></button>
      <button>Save</button>
      <label for="named-input">Name</label>
      <input id="named-input" />
    `);
    const violations = await axeViolations(host);
    expect(ruleIds(violations)).not.toContain("interactive-name");
  });

  it("flags an image without an alt attribute but accepts a decorative one", async () => {
    const missing = fragment('<img src="chart.png" />');
    expect(ruleIds(await axeViolations(missing))).toContain("image-alt");

    const decorative = fragment('<img src="spark.png" alt="" />');
    expect(ruleIds(await axeViolations(decorative))).not.toContain("image-alt");
  });

  it("flags a form control with no label", async () => {
    const host = fragment('<input type="text" />');
    const violations = await axeViolations(host);
    expect(ruleIds(violations)).toContain("form-label");
  });

  it("flags a positive tabindex", async () => {
    const host = fragment('<button tabindex="3">Third</button>');
    const violations = await axeViolations(host);
    expect(ruleIds(violations)).toContain("tabindex-positive");
  });

  it("flags a dialog that is not modal or has no name", async () => {
    const host = fragment('<div role="dialog"><button>Close</button></div>');
    const violations = await axeViolations(host);
    expect(ruleIds(violations)).toContain("dialog-name-modal");
  });

  it("accepts a modal dialog with an accessible name", async () => {
    const host = fragment(
      '<div role="dialog" aria-modal="true" aria-label="Command palette"><button>Close</button></div>'
    );
    expect(ruleIds(await axeViolations(host))).not.toContain("dialog-name-modal");
  });

  it("flags a skipped heading level", async () => {
    const host = fragment("<h1>One</h1><h3>Three</h3>");
    const violations = await axeViolations(host);
    expect(ruleIds(violations)).toContain("heading-order");
  });

  it("flags an invalid live region and a live region wrapped around a whole container", async () => {
    const invalid = fragment('<div aria-live="loud">Status</div>');
    expect(ruleIds(await axeViolations(invalid))).toContain("live-region");

    const container = fragment(
      `<div aria-live="polite">${"<span>x</span>".repeat(60)}</div>`
    );
    expect(ruleIds(await axeViolations(container))).toContain("live-region");
  });

  it("flags an aria-hidden container that still holds a focusable child", async () => {
    const host = fragment('<div aria-hidden="true"><button>Hidden</button></div>');
    const violations = await axeViolations(host);
    expect(ruleIds(violations)).toContain("aria-hidden-focusable");
  });

  it("detects a focus trap and lets a clean region through", () => {
    const trapped = fragment('<div role="dialog" aria-modal="true" aria-label="Modal"></div>');
    expect(findFocusTrap(trapped).length).toBeGreaterThan(0);

    const clean = fragment("<button>Fine</button><a href='#x'>Link</a>");
    expect(findFocusTrap(clean)).toEqual([]);
  });
});

describe("focus helpers", () => {
  it("lists tab stops in the order the keyboard reaches them", () => {
    const host = fragment(`
      <button id="first">First</button>
      <a href="#second" id="second">Second</a>
      <input id="third" />
      <button id="disabled" disabled>Disabled</button>
      <div id="hidden" aria-hidden="true"><button id="invisible">Invisible</button></div>
      <button id="programmatic" tabindex="-1">Programmatic</button>
    `);

    // `aria-hidden` does NOT take a control out of the tab order — it only
    // hides it from the accessibility tree. That mismatch is the defect the
    // `aria-hidden-focusable` rule reports.
    expect(tabbableElements(host).map((element) => element.id)).toEqual([
      "first",
      "second",
      "third",
      "invisible"
    ]);
    expect(focusOrder(host).map((element) => element.id)).toEqual([
      "first",
      "second",
      "third",
      "invisible",
      "programmatic"
    ]);
  });

  it("puts positive tabindex elements first, as the platform does", () => {
    const host = fragment(`
      <button id="natural">Natural</button>
      <button id="later" tabindex="2">Later</button>
      <button id="earlier" tabindex="1">Earlier</button>
    `);
    expect(tabbableElements(host).map((element) => element.id)).toEqual([
      "earlier",
      "later",
      "natural"
    ]);
  });

  it("computes accessible names the way the platform does", () => {
    const host = fragment(`
      <button id="text">Save changes</button>
      <button id="aria" aria-label="Close dialog"></button>
      <span id="referenced">Delete file</span>
      <button id="labelled" aria-labelledby="referenced"></button>
      <label for="control">Email</label>
      <input id="control" />
      <button id="title" title="Refresh"></button>
    `);

    expect(accessibleName(host.querySelector("#text") ?? host)).toBe("Save changes");
    expect(accessibleName(host.querySelector("#aria") ?? host)).toBe("Close dialog");
    expect(accessibleName(host.querySelector("#labelled") ?? host)).toBe("Delete file");
    expect(accessibleName(host.querySelector("#control") ?? host)).toBe("Email");
    expect(accessibleName(host.querySelector("#title") ?? host)).toBe("Refresh");
    expect(hasAccessibleName(host.querySelector("#title") ?? host)).toBe(true);
  });

  it("describes an element for a failure message", () => {
    const host = fragment('<button class="cs-button cs-button--primary">Preview</button>');
    const button = host.querySelector("button");
    expect(button).not.toBeNull();
    if (button === null) return;
    expect(describeElement(button)).toBe('button.cs-button.cs-button--primary "Preview"');
  });
});
