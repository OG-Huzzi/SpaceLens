/**
 * Public surface of the CoreSight test harnesses.
 *
 * Import from here so a suite never reaches into a helper module directly:
 *
 * ```ts
 * import { axeViolations, focusOrder } from "./testing";
 * ```
 */

export {
  accessibleName,
  assertNoFocusTrap,
  axeViolations,
  describeElement,
  elementPath,
  findFocusTrap,
  focusOrder,
  formatViolations,
  hasAccessibleName,
  isFocusable,
  isHidden,
  isHiddenFromAT,
  isTabbable,
  tabbableElements,
  visibleText,
  type Violation,
  type ViolationImpact,
  type ViolationNode
} from "./a11y";

export {
  createCountingApi,
  createStaticApi,
  makeCategories,
  makeStorageData,
  type CountingApi
} from "./api";

export {
  PILLAR_SCREENS,
  SETTINGS_SCREEN,
  renderScreen,
  waitForScreenReady,
  type ScreenCase
} from "./screens";

export {
  measureInteraction,
  supportsLongTaskObserver,
  type InteractionMeasurement
} from "./perf";

export {
  distinctValues,
  findHexColours,
  formatOffenders,
  groupByFile,
  listCssFiles,
  scanCssFiles,
  stripCssComments,
  type HexOffender
} from "./designTokens";
