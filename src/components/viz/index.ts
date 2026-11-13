/**
 * Public surface of the visualisation components.
 *
 * Screens import from here; the algorithms stay internal unless a test needs
 * them (`./squarify` is exported for the layout assertions).
 */

export {
  Treemap,
  type TreemapNode,
  type TreemapProps,
  type TreemapTier
} from "./Treemap";
export {
  ProportionBar,
  type ProportionBarProps,
  type ProportionSegment
} from "./ProportionBar";
export { DeltaSpark, type DeltaPoint, type DeltaSparkProps } from "./DeltaSpark";
export {
  meanAspectRatio,
  rectArea,
  rectAspectRatio,
  squarify,
  worstAspectRatio,
  type PlacedRect,
  type Rect,
  type SquarifiedItem
} from "./squarify";
export {
  effectiveValue,
  layoutTreemap,
  UNKNOWN_BAND_FRACTION,
  type TreemapBand,
  type TreemapDrawable,
  type TreemapFrame,
  type TreemapLeaf,
  type TreemapLayout
} from "./treemapLayout";
