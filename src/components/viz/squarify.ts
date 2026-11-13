/**
 * Squarified treemap layout.
 *
 * Bruls, Huizing & van Wijk, "Squarified Treemaps" (2000). The plain
 * slice-and-dice layout gives every item a full-height strip, so a 2 % item
 * is a 2 %-wide, 100 %-tall sliver: its area is right but its shape is a
 * needle. Squarified layout grows the items into *rows* and places each row
 * so that the worst aspect ratio inside the row stays as close to 1 as
 * possible. That is what makes area legible: a reader compares squares, not
 * slivers.
 *
 * THE MODEL
 * ---------
 * The container is a rectangle of area A and the items carry values v_i with
 * sum V. A global scale k = A / V maps a value to an area (value * k = area),
 * so every layout decision can be made in area terms and stays exact.
 *
 * A row is a strip of length L (the long side of the free rectangle) and
 * thickness t = S * k / L, where S is the sum of the row's values. Item i
 * occupies a length v_i * L / S along the strip. The aspect ratio of item i is
 *
 *     max( v_i * L^2 / (S^2 * k),  S^2 * k / (v_i * L^2) )
 *
 * which is increasing in v_i for the first term and decreasing for the second,
 * so the worst item in a row is either its largest or its smallest member:
 *
 *     worst(row) = max( L^2 * max / (S^2 * k),  S^2 * k / (L^2 * min) )
 *
 * Rows are grown greedily: keep adding the next (descending) item while the
 * worst aspect ratio does not get worse. When it would get worse, the row is
 * closed, laid out, and the free rectangle shrinks by the row's thickness.
 *
 * DETERMINISM
 * -----------
 * The algorithm is pure: same items, same rectangle, same rectangles out.
 * Callers sort before calling, so the order of the input does not change the
 * result.
 */

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SquarifiedItem {
  id: string;
  /** Strictly positive. Items with a non-positive value are not laid out. */
  value: number;
}

export interface PlacedRect {
  id: string;
  rect: Rect;
}

/** Area of a rectangle, clamped at zero for degenerate sizes. */
export function rectArea(rect: Rect): number {
  return Math.max(0, rect.width) * Math.max(0, rect.height);
}

/**
 * The aspect ratio of a rectangle: 1 for a square, larger the more elongated
 * it is. This is the metric the squarified layout minimises.
 */
export function rectAspectRatio(rect: Rect): number {
  if (rect.width <= 0 || rect.height <= 0) return Number.POSITIVE_INFINITY;
  return Math.max(rect.width / rect.height, rect.height / rect.width);
}

/**
 * Mean aspect ratio over a set of rectangles. Used to compare layouts: the
 * lower the mean, the squarer the rectangles.
 */
export function meanAspectRatio(rects: readonly Rect[]): number {
  if (rects.length === 0) return Number.POSITIVE_INFINITY;
  let total = 0;
  for (const rect of rects) total += rectAspectRatio(rect);
  return total / rects.length;
}

/**
 * The worst (largest) aspect ratio inside a candidate row.
 *
 * @param values Values in the row, in any order.
 * @param length Length of the strip the row is laid out along.
 * @param scale Area per unit of value for the container.
 */
export function worstAspectRatio(
  values: readonly number[],
  length: number,
  scale: number
): number {
  if (values.length === 0 || length <= 0 || scale <= 0) return Number.POSITIVE_INFINITY;
  let sum = 0;
  let min = Number.POSITIVE_INFINITY;
  let max = 0;
  for (const value of values) {
    sum += value;
    if (value < min) min = value;
    if (value > max) max = value;
  }
  if (sum <= 0 || min <= 0) return Number.POSITIVE_INFINITY;
  const lengthSquared = length * length;
  return Math.max(
    (lengthSquared * max) / (sum * sum * scale),
    (sum * sum * scale) / (lengthSquared * min)
  );
}

/**
 * Lay `items` out inside `rect` with the squarified algorithm.
 *
 * Items are consumed in the order given: callers MUST pass them sorted by
 * descending value (ties broken by id) so the result is deterministic and
 * reproducible. Items whose value is not a positive finite number are skipped
 * — a caller that has an unknown size must not pass it as zero.
 */
export function squarify(items: readonly SquarifiedItem[], rect: Rect): PlacedRect[] {
  const placed: PlacedRect[] = [];
  const usable = items.filter(
    (item) => Number.isFinite(item.value) && item.value > 0
  );
  if (usable.length === 0) return placed;

  const totalValue = usable.reduce((sum, item) => sum + item.value, 0);
  const area = rectArea(rect);
  if (totalValue <= 0 || area <= 0) return placed;

  // Area per unit of value for this container.
  const scale = area / totalValue;

  const remaining = usable.slice();
  let free: Rect = { ...rect };

  while (remaining.length > 0) {
    if (remaining.length === 1) {
      // The last item takes whatever is left.
      placed.push({ id: remaining[0].id, rect: free });
      break;
    }

    // The row runs along the long side of the free rectangle; successive rows
    // consume the short side.
    const wide = free.width >= free.height;
    const length = wide ? free.width : free.height;

    const row: SquarifiedItem[] = [];
    const rowValues: number[] = [];
    let best = Number.POSITIVE_INFINITY;

    while (remaining.length > 0) {
      const next = remaining[0];
      const candidate = rowValues.concat(next.value);
      const ratio = worstAspectRatio(candidate, length, scale);
      if (row.length === 0 || ratio <= best) {
        remaining.shift();
        row.push(next);
        rowValues.push(next.value);
        best = ratio;
      } else {
        break;
      }
    }

    const rowSum = rowValues.reduce((sum, value) => sum + value, 0);
    const thickness = (rowSum * scale) / length;

    let offset = wide ? free.x : free.y;
    for (const item of row) {
      const extent = (item.value * scale) / thickness;
      placed.push({
        id: item.id,
        rect: wide
          ? { x: offset, y: free.y, width: extent, height: thickness }
          : { x: free.x, y: offset, width: thickness, height: extent }
      });
      offset += extent;
    }

    free = wide
      ? {
          x: free.x,
          y: free.y + thickness,
          width: free.width,
          height: free.height - thickness
        }
      : {
          x: free.x + thickness,
          y: free.y,
          width: free.width - thickness,
          height: free.height
        };
  }

  return placed;
}
