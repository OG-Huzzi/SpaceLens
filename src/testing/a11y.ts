/**
 * Native accessibility harness for CoreSight.
 *
 * `axe-core` is deliberately NOT a dependency of this project. This module
 * implements the checks the design system actually needs, against the DOM, so
 * the rules stay readable and the failures name the offending element instead
 * of a rule id from a third-party bundle.
 *
 * WHAT THIS MODULE ENFORCES
 * -------------------------
 * Each rule below exists because `docs/DESIGN_SYSTEM.md` lists it as an
 * anti-pattern, or because it is the mechanical form of a rule in
 * `docs/UX_ARCHITECTURE.md`:
 *
 * | Rule                     | Design-system / UX rule it enforces                |
 * |--------------------------|----------------------------------------------------|
 * | `interactive-name`       | "An icon-only button with no `aria-label`"          |
 * | `form-label`             | "Keyboard-first" — every control must be nameable   |
 * | `image-alt`              | Every number/element shows its evidence (alt text) |
 * | `tabindex-positive`      | "Keyboard-first" — natural tab order               |
 * | `dialog-name-modal`      | A dialog must be modal *and* nameable               |
 * | `heading-order`          | One question per screen, readable structure        |
 * | `live-region`            | "Scanning is ambient, never blocking"               |
 * | `aria-hidden-focusable`  | Never hide something a keyboard can still reach     |
 * | `focus-trap`             | Nothing may trap the keyboard outside Cleanup      |
 *
 * EVERY HELPER RETURNS STRUCTURED DATA. `hasAccessibleName` is a convenience
 * boolean, but `accessibleName`, `axeViolations`, `focusOrder` and
 * `tabbableElements` return the evidence a test needs to make a precise
 * assertion (or to print a useful failure message).
 */

export type ViolationImpact = "critical" | "serious" | "moderate" | "minor";

export interface ViolationNode {
  /** Outer HTML of the offending element, truncated for readability. */
  html: string;
  /** A stable-ish path to the element, e.g. `["main", "li:nth-of-type(3)"]`. */
  target: string[];
  /** Human sentence describing what is wrong with this node. */
  failureSummary: string;
}

export interface Violation {
  /** Stable rule id, e.g. `interactive-name`. */
  id: string;
  impact: ViolationImpact;
  /** What the rule requires. */
  description: string;
  /** Why the rule exists in this product. */
  help: string;
  nodes: ViolationNode[];
}

/* ------------------------------------------------------------------------- */
/* Element description                                                        */
/* ------------------------------------------------------------------------- */

/** Truncate an HTML snippet so a failure message stays readable. */
function truncate(value: string, max = 220): string {
  return value.length > max ? `${value.slice(0, max)}…` : value;
}

/**
 * A short, human-readable description of an element:
 * `<button class="cs-button">Preview</button>`.
 */
export function describeElement(element: Element): string {
  const tag = element.tagName.toLowerCase();
  const id = element.getAttribute("id");
  const classes = element.getAttribute("class");
  const text = (element.textContent ?? "").replace(/\s+/g, " ").trim();
  const parts = [tag];
  if (id) parts.push(`#${id}`);
  if (classes) parts.push(`.${classes.replace(/\s+/g, ".").trim()}`);
  const head = parts.join("");
  return text.length > 0 ? `${head} "${truncate(text, 60)}"` : head;
}

/** A DOM path to an element, used as the `target` of a violation node. */
export function elementPath(element: Element): string[] {
  const path: string[] = [];
  let current: Element | null = element;
  while (current && current.nodeType === 1 && path.length < 6) {
    const node: Element = current;
    const tag = node.tagName.toLowerCase();
    const id = node.getAttribute("id");
    if (id) {
      path.unshift(`#${id}`);
      break;
    }
    const parent: Element | null = node.parentElement;
    if (!parent) {
      path.unshift(tag);
      break;
    }
    const siblings = Array.from(parent.children).filter(
      (child) => child.tagName === node.tagName
    );
    if (siblings.length > 1) {
      path.unshift(`${tag}:nth-of-type(${siblings.indexOf(node) + 1})`);
    } else {
      path.unshift(tag);
    }
    current = parent;
  }
  return path;
}

/* ------------------------------------------------------------------------- */
/* Visibility                                                                 */
/* ------------------------------------------------------------------------- */

function isHiddenAttribute(element: Element): boolean {
  return element.hasAttribute("hidden");
}

function isAriaHidden(element: Element): boolean {
  return element.getAttribute("aria-hidden") === "true";
}

/**
 * Stylesheet-driven hiding (`display: none` / `visibility: hidden`) is cached
 * per element: jsdom computes styles slowly, and the value only changes when
 * the stylesheet does. Inline styles are never cached, so a test that toggles
 * `element.style.display` always gets an honest answer.
 */
const stylesheetHiddenCache = new WeakMap<Element, boolean>();

function isHiddenInline(element: Element): boolean {
  const style = element.getAttribute("style");
  if (!style) return false;
  return /display\s*:\s*none/i.test(style) || /visibility\s*:\s*hidden/i.test(style);
}

function isHiddenByStylesheet(element: Element): boolean {
  const cached = stylesheetHiddenCache.get(element);
  if (cached !== undefined) return cached;
  const view = element.ownerDocument?.defaultView;
  let hidden = false;
  if (view) {
    const style = view.getComputedStyle(element);
    hidden = style.display === "none" || style.visibility === "hidden";
  }
  stylesheetHiddenCache.set(element, hidden);
  return hidden;
}

/** True when the element itself is hidden from rendering. */
export function isHidden(element: Element): boolean {
  if (isHiddenAttribute(element)) return true;
  if (isHiddenInline(element)) return true;
  return isHiddenByStylesheet(element);
}

/**
 * True when the element is hidden from the accessibility tree: either it is
 * not rendered at all, or it (or an ancestor) is marked `aria-hidden="true"`.
 */
export function isHiddenFromAT(element: Element): boolean {
  if (isHidden(element)) return true;
  let current: Element | null = element;
  while (current) {
    if (isAriaHidden(current)) return true;
    current = current.parentElement;
  }
  return false;
}

/**
 * True when the element is not rendered — and therefore cannot be focused.
 *
 * `aria-hidden` is deliberately *not* part of this: hiding a control from the
 * accessibility tree does not take it out of the tab order, which is exactly
 * why the `aria-hidden-focusable` rule exists.
 */
export function isRenderHidden(element: Element): boolean {
  let current: Element | null = element;
  while (current) {
    if (isHidden(current)) return true;
    current = current.parentElement;
  }
  return false;
}

function isInert(element: Element): boolean {
  let current: Element | null = element;
  while (current) {
    if (current.hasAttribute("inert")) return true;
    current = current.parentElement;
  }
  return false;
}

/* ------------------------------------------------------------------------- */
/* Accessible name                                                            */
/* ------------------------------------------------------------------------- */

function isLabelable(element: Element): element is HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement {
  return (
    element instanceof HTMLInputElement ||
    element instanceof HTMLSelectElement ||
    element instanceof HTMLTextAreaElement
  );
}

/**
 * Text of a subtree, ignoring parts that are hidden from assistive
 * technology (an `aria-hidden` icon must not contribute a name).
 */
export function visibleText(element: Element): string {
  let out = "";
  const visit = (node: Node): void => {
    if (node.nodeType === 3) {
      out += node.textContent ?? "";
      return;
    }
    if (node.nodeType !== 1) return;
    const child = node as Element;
    if (isAriaHidden(child) || isHiddenAttribute(child)) return;
    if (child instanceof HTMLInputElement || child instanceof HTMLTextAreaElement) {
      out += child.value ?? "";
      return;
    }
    if (child instanceof HTMLSelectElement) {
      const selected = child.options[child.selectedIndex];
      out += selected?.textContent ?? "";
      return;
    }
    for (const grandChild of Array.from(child.childNodes)) visit(grandChild);
  };
  for (const child of Array.from(element.childNodes)) visit(child);
  return out.replace(/\s+/g, " ").trim();
}

/** Labels associated with a form control, in DOM order. */
function associatedLabels(element: Element): HTMLLabelElement[] {
  if (isLabelable(element) && element.labels) {
    return Array.from(element.labels);
  }
  const found: HTMLLabelElement[] = [];
  const id = element.getAttribute("id");
  if (id) {
    const root = element.ownerDocument;
    if (root) {
      for (const label of Array.from(root.querySelectorAll("label"))) {
        if (label.getAttribute("for") === id) found.push(label);
      }
    }
  }
  const wrapping = element.closest("label");
  if (wrapping && !found.includes(wrapping)) found.push(wrapping);
  return found;
}

/**
 * The accessible name of an element, computed the way the platform computes
 * it (aria-labelledby → aria-label → native label → content → title).
 *
 * Returns an empty string when the element has no name. The string is trimmed
 * so that a whitespace-only `aria-label` counts as "no name" — which is what
 * assistive technology does.
 */
export function accessibleName(element: Element): string {
  if (isAriaHidden(element)) return "";

  const labelledBy = element.getAttribute("aria-labelledby");
  if (labelledBy) {
    const root = element.ownerDocument;
    const names = labelledBy
      .split(/\s+/)
      .map((id) => root?.getElementById(id))
      .filter((node): node is HTMLElement => node !== null && node !== undefined)
      .map((node) => visibleText(node))
      .filter((text) => text.length > 0);
    if (names.length > 0) return names.join(" ").trim();
  }

  const ariaLabel = element.getAttribute("aria-label");
  if (ariaLabel && ariaLabel.trim().length > 0) return ariaLabel.trim();

  if (element instanceof HTMLInputElement) {
    const type = (element.getAttribute("type") ?? "text").toLowerCase();
    if (type === "submit" || type === "button" || type === "reset") {
      const value = element.getAttribute("value");
      if (value && value.trim().length > 0) return value.trim();
    }
    if (type === "image") {
      const alt = element.getAttribute("alt");
      if (alt !== null) return alt.trim();
    }
  }

  const labels = associatedLabels(element);
  if (labels.length > 0) {
    const text = labels.map((label) => visibleText(label)).join(" ").trim();
    if (text.length > 0) return text;
  }

  // Buttons, links and disclosure summaries take their name from their
  // content, the way the platform computes it.
  const tag = element.tagName.toLowerCase();
  if (tag === "button" || tag === "a" || tag === "summary") {
    const text = visibleText(element);
    if (text.length > 0) return text;
  }

  if (element instanceof HTMLFieldSetElement) {
    const legend = element.querySelector("legend");
    const text = legend ? visibleText(legend) : "";
    if (text.length > 0) return text;
  }

  if (element instanceof HTMLImageElement) {
    const alt = element.getAttribute("alt");
    if (alt !== null) return alt.trim();
  }

  const title = element.getAttribute("title");
  if (title && title.trim().length > 0) return title.trim();

  return "";
}

/**
 * True when the element has a non-empty accessible name.
 *
 * A hidden element (`aria-hidden="true"`) never has one: it is not exposed to
 * assistive technology at all.
 */
export function hasAccessibleName(element: HTMLElement): boolean {
  return accessibleName(element).length > 0;
}

/* ------------------------------------------------------------------------- */
/* Focusability                                                               */
/* ------------------------------------------------------------------------- */

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "area[href]",
  "button",
  "input",
  "select",
  "textarea",
  "summary",
  "iframe",
  "object",
  "embed",
  "audio[controls]",
  "video[controls]",
  "[tabindex]",
  "[contenteditable]:not([contenteditable='false'])"
].join(",");

/**
 * Controls the user operates: the native ones plus every element that takes
 * an explicit interactive role.
 *
 * A heading with `tabindex="-1"` is *programmatically* focusable (the app
 * moves focus to it after a route change) but it is not a control, so it is
 * not held to the "every control has a name" rule.
 */
const INTERACTIVE_SELECTOR = [
  "a[href]",
  "button",
  "input",
  "select",
  "textarea",
  "summary",
  "[role='button']",
  "[role='link']",
  "[role='checkbox']",
  "[role='radio']",
  "[role='switch']",
  "[role='tab']",
  "[role='menuitem']",
  "[role='menuitemcheckbox']",
  "[role='menuitemradio']",
  "[role='option']",
  "[role='textbox']",
  "[role='searchbox']",
  "[role='combobox']",
  "[role='slider']",
  "[role='spinbutton']"
].join(",");

/** True for a control the user operates. */
export function isInteractiveElement(element: Element): boolean {
  return element.matches(INTERACTIVE_SELECTOR);
}

/** True for an element that sits in the natural tab order. */
export function isTabStop(element: Element): boolean {
  if (!element.hasAttribute("tabindex")) return false;
  const value = Number.parseInt(element.getAttribute("tabindex") ?? "0", 10);
  return Number.isFinite(value) && value >= 0;
}
export function isFocusable(element: Element): boolean {
  if (!element.matches(FOCUSABLE_SELECTOR)) return false;
  if (element instanceof HTMLInputElement) {
    const type = (element.getAttribute("type") ?? "text").toLowerCase();
    if (type === "hidden") return false;
  }
  if (isRenderHidden(element)) return false;
  if (isInert(element)) return false;
  if (element.matches(":disabled")) return false;
  return true;
}

/** Elements that receive focus in the natural tab order. */
export function isTabbable(element: Element): boolean {
  if (!isFocusable(element)) return false;
  const tabIndex = (element as HTMLElement).tabIndex;
  if (tabIndex < 0) return false;
  return true;
}

function descendants(root: Element): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>("*"));
}

/**
 * Every element that receives focus in the natural tab order, in the order a
 * user would reach them.
 *
 * Elements with a positive `tabindex` come first (that is what the platform
 * does); everything else follows in document order.
 */
export function tabbableElements(root: HTMLElement): HTMLElement[] {
  const candidates = descendants(root).filter((element) => isTabbable(element));
  return sortTabOrder(candidates);
}

function sortTabOrder(elements: HTMLElement[]): HTMLElement[] {
  const withIndex = elements.map((element, index) => ({
    element,
    index,
    tabIndex: element.tabIndex
  }));
  const positive = withIndex
    .filter((entry) => entry.tabIndex > 0)
    .sort((left, right) => left.tabIndex - right.tabIndex || left.index - right.index);
  const natural = withIndex
    .filter((entry) => entry.tabIndex <= 0)
    .sort((left, right) => left.index - right.index);
  return [...positive, ...natural].map((entry) => entry.element);
}

/**
 * The full focus order of a container: the tab stops first, then the
 * programmatically focusable elements that are not tab stops (`tabindex="-1"`,
 * the pattern used to move focus to a heading after a route change).
 *
 * A screen that puts a `tabindex="-1"` element in front of its content is
 * still keyboard-first — the element is skipped by Tab — so this helper keeps
 * the two groups apart instead of pretending they are the same.
 */
export function focusOrder(container: HTMLElement): HTMLElement[] {
  const candidates = descendants(container).filter((element) => isFocusable(element));
  const tabStops = sortTabOrder(candidates.filter((element) => element.tabIndex >= 0));
  const programmatic = candidates.filter((element) => element.tabIndex < 0);
  return [...tabStops, ...programmatic];
}

/* ------------------------------------------------------------------------- */
/* Focus trap detection                                                       */
/* ------------------------------------------------------------------------- */

export interface FocusTrapFinding {
  reason: string;
  element: Element;
}

/**
 * Detect the shapes that trap the keyboard, without needing a real browser:
 *
 * 1. a modal dialog (`aria-modal="true"`) nested inside the root — a modal
 *    inside a screen hides the rest of the app behind an overlay the screen
 *    owner did not ask for;
 * 2. a focus sentinel — an `aria-hidden="true"` element that is still
 *    focusable, the classic "loop the tab around the dialog" hack;
 * 3. a duplicated tab stop — the same element reachable twice, which loops
 *    the ring;
 * 4. a positive `tabindex` inside the root — it re-orders the ring and can
 *    create a cycle the user cannot leave.
 */
export function findFocusTrap(root: HTMLElement): FocusTrapFinding[] {
  const findings: FocusTrapFinding[] = [];

  for (const element of descendants(root)) {
    if (element.getAttribute("aria-modal") === "true") {
      findings.push({
        reason: "an aria-modal=\"true\" dialog is nested inside the scanned region",
        element
      });
    }
    if (isAriaHidden(element) && isFocusable(element)) {
      findings.push({
        reason: "an aria-hidden=\"true\" element is still focusable (focus sentinel)",
        element
      });
    }
    const tabIndex = (element as HTMLElement).tabIndex;
    if (tabIndex > 0) {
      findings.push({
        reason: `a positive tabindex (${tabIndex}) re-orders the tab ring`,
        element
      });
    }
  }

  const stops = tabbableElements(root);
  const seen = new Set<HTMLElement>();
  for (const stop of stops) {
    if (seen.has(stop)) {
      findings.push({
        reason: "the same element appears twice in the tab order (tab ring loops)",
        element: stop
      });
    }
    seen.add(stop);
  }

  return findings;
}

/**
 * Assert that the region does not trap the keyboard.
 *
 * Throws an `Error` describing every trap it found. Use it on a screen, a
 * dialog, or a panel — anything a user must be able to tab out of.
 */
export function assertNoFocusTrap(root: HTMLElement): void {
  const findings = findFocusTrap(root);
  if (findings.length === 0) return;
  const detail = findings
    .map((finding) => `  - ${finding.reason}: ${describeElement(finding.element)}`)
    .join("\n");
  throw new Error(`Focus trap detected in ${describeElement(root)}:\n${detail}`);
}

/* ------------------------------------------------------------------------- */
/* The scan                                                                   */
/* ------------------------------------------------------------------------- */

function group(
  id: string,
  impact: ViolationImpact,
  description: string,
  help: string,
  nodes: ViolationNode[]
): Violation {
  return { id, impact, description, help, nodes };
}

/** Form controls that need a label (buttons and hidden inputs do not). */
function isLabelNeedingControl(element: Element): boolean {
  if (element instanceof HTMLInputElement) {
    const type = (element.getAttribute("type") ?? "text").toLowerCase();
    return !["hidden", "submit", "button", "reset", "image"].includes(type);
  }
  return element instanceof HTMLSelectElement || element instanceof HTMLTextAreaElement;
}

const HEADING_TAGS = ["h1", "h2", "h3", "h4", "h5", "h6"];

/**
 * Scan a container and return every accessibility violation found.
 *
 * The scan is asynchronous to match the shape of an automated a11y service
 * (`axe.run`), so a test can `await` it without special-casing. It is
 * deterministic and runs against the live DOM — no network, no timers.
 */
export async function axeViolations(container: HTMLElement): Promise<Violation[]> {
  const violations: Violation[] = [];
  const elements = descendants(container);

  /* Rule: interactive-name ---------------------------------------------- */
  const unnamedInteractive: ViolationNode[] = [];
  for (const element of elements) {
    // A control the user operates, or an element that sits in the tab order.
    if (!isInteractiveElement(element) && !isTabStop(element)) continue;
    if (!isFocusable(element)) continue;
    if (accessibleName(element).length > 0) continue;
    unnamedInteractive.push({
      html: truncate(element.outerHTML),
      target: elementPath(element),
      failureSummary: `${describeElement(element)} has no accessible name`
    });
  }
  if (unnamedInteractive.length > 0) {
    violations.push(
      group(
        "interactive-name",
        "serious",
        "Every interactive element must have an accessible name.",
        "Design system: an icon-only button with no aria-label is an anti-pattern. Name the control with aria-label, aria-labelledby, visible text, or a <label>.",
        unnamedInteractive
      )
    );
  }

  /* Rule: form-label ---------------------------------------------------- */
  const unlabelledControls: ViolationNode[] = [];
  for (const element of elements) {
    if (!isLabelNeedingControl(element)) continue;
    if (isHiddenFromAT(element)) continue;
    const hasAriaName =
      (element.getAttribute("aria-label") ?? "").trim().length > 0 ||
      (element.getAttribute("aria-labelledby") ?? "").trim().length > 0;
    const hasLabel = associatedLabels(element).length > 0;
    if (hasAriaName || hasLabel) continue;
    unlabelledControls.push({
      html: truncate(element.outerHTML),
      target: elementPath(element),
      failureSummary: `${describeElement(element)} is a form control with no label`
    });
  }
  if (unlabelledControls.length > 0) {
    violations.push(
      group(
        "form-label",
        "serious",
        "Every form control must have a label.",
        "Design system: keyboard-first. A control nobody can name is unusable with a screen reader.",
        unlabelledControls
      )
    );
  }

  /* Rule: image-alt ----------------------------------------------------- */
  const missingAlt: ViolationNode[] = [];
  for (const element of elements) {
    if (!(element instanceof HTMLImageElement)) continue;
    if (element.getAttribute("alt") !== null) continue;
    missingAlt.push({
      html: truncate(element.outerHTML),
      target: elementPath(element),
      failureSummary: `${describeElement(element)} has no alt attribute (use alt="" when decorative)`
    });
  }
  if (missingAlt.length > 0) {
    violations.push(
      group(
        "image-alt",
        "critical",
        "Every image must carry an alt attribute.",
        "Design system: every element shows its evidence. A decorative image uses alt=\"\", a meaningful one describes itself.",
        missingAlt
      )
    );
  }

  /* Rule: tabindex-positive --------------------------------------------- */
  const positiveTabIndex: ViolationNode[] = [];
  for (const element of elements) {
    const raw = element.getAttribute("tabindex");
    if (raw === null) continue;
    const value = Number.parseInt(raw, 10);
    if (!Number.isFinite(value) || value <= 0) continue;
    positiveTabIndex.push({
      html: truncate(element.outerHTML),
      target: elementPath(element),
      failureSummary: `${describeElement(element)} has tabindex="${raw}"`
    });
  }
  if (positiveTabIndex.length > 0) {
    violations.push(
      group(
        "tabindex-positive",
        "serious",
        "tabindex must never be positive.",
        "Design system: keyboard-first. A positive tabindex re-orders the natural tab sequence and is how focus traps start.",
        positiveTabIndex
      )
    );
  }

  /* Rule: dialog-name-modal --------------------------------------------- */
  const brokenDialogs: ViolationNode[] = [];
  for (const element of elements) {
    const role = element.getAttribute("role");
    if (role !== "dialog" && role !== "alertdialog") continue;
    const problems: string[] = [];
    if (element.getAttribute("aria-modal") !== "true") {
      problems.push('aria-modal="true" is missing');
    }
    if (accessibleName(element).length === 0) {
      problems.push("the dialog has no accessible name");
    }
    if (problems.length === 0) continue;
    brokenDialogs.push({
      html: truncate(element.outerHTML),
      target: elementPath(element),
      failureSummary: `${describeElement(element)}: ${problems.join("; ")}`
    });
  }
  if (brokenDialogs.length > 0) {
    violations.push(
      group(
        "dialog-name-modal",
        "critical",
        "A dialog must be modal and must have an accessible name.",
        "A dialog that is not modal lets the user tab into the page behind it; a dialog without a name is announced as nothing.",
        brokenDialogs
      )
    );
  }

  /* Rule: heading-order -------------------------------------------------- */
  const headingNodes: ViolationNode[] = [];
  const headings = elements.filter((element) =>
    HEADING_TAGS.includes(element.tagName.toLowerCase())
  );
  let previousLevel: number | null = null;
  for (const heading of headings) {
    const level = Number.parseInt(heading.tagName.slice(1), 10);
    if (previousLevel !== null && level > previousLevel + 1) {
      headingNodes.push({
        html: truncate(heading.outerHTML),
        target: elementPath(heading),
        failureSummary: `heading level jumps from h${previousLevel} to h${level}`
      });
    }
    previousLevel = level;
  }
  if (headingNodes.length > 0) {
    violations.push(
      group(
        "heading-order",
        "moderate",
        "Heading levels must not skip (h1 → h2 → h3).",
        "A skipped level breaks the screen-reader heading outline, which is how a non-sighted user understands what a screen is for.",
        headingNodes
      )
    );
  }

  /* Rule: live-region ---------------------------------------------------- */
  const liveRegionNodes: ViolationNode[] = [];
  for (const element of elements) {
    const live = element.getAttribute("aria-live");
    const role = element.getAttribute("role");
    const problems: string[] = [];

    if (live !== null && !["polite", "assertive", "off"].includes(live)) {
      problems.push(`aria-live="${live}" is not a valid value`);
    }
    if (role === "status" && live === "assertive") {
      problems.push('role="status" must be polite, not assertive');
    }
    if (role === "alert" && live === "polite") {
      problems.push('role="alert" must be assertive, not polite');
    }
    if (live !== null && live !== "off") {
      const descendantCount = element.querySelectorAll("*").length;
      if (descendantCount > 40) {
        problems.push(
          `the live region contains ${descendantCount} elements — a live region must announce a small message, not a re-rendering container`
        );
      }
    }
    if (problems.length === 0) continue;
    liveRegionNodes.push({
      html: truncate(element.outerHTML),
      target: elementPath(element),
      failureSummary: `${describeElement(element)}: ${problems.join("; ")}`
    });
  }
  if (liveRegionNodes.length > 0) {
    violations.push(
      group(
        "live-region",
        "serious",
        "Live regions must be valid, consistent with their role, and small.",
        "UX architecture: scanning is ambient, never blocking. A live region around a constantly re-rendering container announces noise.",
        liveRegionNodes
      )
    );
  }

  /* Rule: aria-hidden-focusable ------------------------------------------ */
  const hiddenFocusables: ViolationNode[] = [];
  for (const element of elements) {
    if (!isAriaHidden(element)) continue;
    const focusableInside = descendants(element).filter((child) => isFocusable(child));
    if (focusableInside.length === 0) continue;
    hiddenFocusables.push({
      html: truncate(element.outerHTML),
      target: elementPath(element),
      failureSummary: `${describeElement(element)} is aria-hidden but contains ${focusableInside.length} focusable element(s): ${focusableInside
        .map((child) => describeElement(child))
        .join(", ")}`
    });
  }
  if (hiddenFocusables.length > 0) {
    violations.push(
      group(
        "aria-hidden-focusable",
        "serious",
        "An aria-hidden element must not contain focusable children.",
        "A control that is hidden from the screen reader but reachable by Tab is announced as nothing when it takes focus.",
        hiddenFocusables
      )
    );
  }

  return violations;
}

/* ------------------------------------------------------------------------- */
/* Reporting                                                                  */
/* ------------------------------------------------------------------------- */

/** A one-line-per-violation report, for assertion messages. */
export function formatViolations(violations: Violation[]): string {
  if (violations.length === 0) return "no violations";
  return violations
    .map(
      (entry) =>
        `${entry.id} (${entry.impact}): ${entry.nodes
          .map((node) => node.failureSummary)
          .join(" | ")}`
    )
    .join("\n");
}
