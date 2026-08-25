import type {
  GraphicTransform,
  Extent,
  Point,
  GraphicItemDto,
} from "../../shared/modelicaGraphics.js";

export function identity(): GraphicTransform {
  return { translate: { x: 0, y: 0 }, scale: { x: 1, y: 1 }, rotate: 0 };
}

/** Round to nearest grid step (default 10) */
export function snap(v: number, grid = 10): number {
  return Math.round(v / grid) * grid;
}

/** Translate an extent by (dx, dy) */
export function translateExtent(
  extent: Extent,
  dx: number,
  dy: number,
): Extent {
  return {
    p1: { x: extent.p1.x + dx, y: extent.p1.y + dy },
    p2: { x: extent.p2.x + dx, y: extent.p2.y + dy },
  };
}

/** Translate a list of points by (dx, dy) */
export function translatePoints(
  points: Point[],
  dx: number,
  dy: number,
): Point[] {
  return points.map((p) => ({ x: p.x + dx, y: p.y + dy }));
}

/**
 * Apply a transform translate to a graphic's geometry, returning a NEW
 * graphic (pure function — never mutates the input). If the graphic has an
 * origin, the origin is moved instead of extent/points (preserves size).
 */
export function applyTransform(
  graphic: GraphicItemDto,
  transform: GraphicTransform,
): GraphicItemDto {
  const dx = transform.translate.x;
  const dy = transform.translate.y;
  if (dx === 0 && dy === 0) return graphic;

  const g = graphic as any;

  // origin-based: move origin only
  if (g.origin) {
    return {
      ...g,
      origin: { x: g.origin.x + dx, y: g.origin.y + dy },
    } as GraphicItemDto;
  }

  // extent-based (Rectangle / Ellipse / Text)
  if (g.extent) {
    return {
      ...g,
      extent: translateExtent(g.extent as Extent, dx, dy),
    } as GraphicItemDto;
  }

  // points-based (Line / Polygon)
  if (g.points) {
    return {
      ...g,
      points: translatePoints(g.points as Point[], dx, dy),
    } as GraphicItemDto;
  }

  return graphic;
}

/** SVG transform attribute string for the graphic group */
export function toSvgTransform(t: GraphicTransform): string {
  const parts: string[] = [];
  if (t.translate.x !== 0 || t.translate.y !== 0) {
    parts.push(`translate(${t.translate.x},${t.translate.y})`);
  }
  if (t.scale.x !== 1 || t.scale.y !== 1) {
    parts.push(`scale(${t.scale.x},${t.scale.y})`);
  }
  if (t.rotate !== 0) {
    parts.push(`rotate(${t.rotate})`);
  }
  return parts.join(" ");
}

/** Bounding box of a graphic (for selection outline) */
export interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export function boundsOf(graphic: GraphicItemDto): Bounds | null {
  const g = graphic as any;
  if (g.extent) {
    const x1 = g.extent.p1.x;
    const y1 = g.extent.p1.y;
    const x2 = g.extent.p2.x;
    const y2 = g.extent.p2.y;
    return {
      x: Math.min(x1, x2),
      y: Math.min(y1, y2),
      width: Math.abs(x2 - x1),
      height: Math.abs(y2 - y1),
    };
  }
  if (g.points && g.points.length > 0) {
    const xs = g.points.map((p: Point) => p.x);
    const ys = g.points.map((p: Point) => p.y);
    const minX = Math.min(...xs);
    const minY = Math.min(...ys);
    return {
      x: minX - 2,
      y: minY - 2,
      width: Math.max(...xs) - minX + 4,
      height: Math.max(...ys) - minY + 4,
    };
  }
  return null;
}
