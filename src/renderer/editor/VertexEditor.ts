import type { Point } from "../../shared/modelicaGraphics.js";

export const LINE_HIT_WIDTH_PX = 12;
export const VERTEX_HIT_RADIUS_PX = 9;
export const VERTEX_VISUAL_RADIUS_PX = 4;

function formatNumber(value: number): string {
  if (Number.isInteger(value)) return String(value);
  return String(Number(value.toFixed(6)));
}

export function moveVertex(
  points: Point[],
  vertexIndex: number,
  dx: number,
  dy: number,
  syncClosed = false,
): Point[] {
  const next = points.map((point) => ({ ...point }));
  const current = next[vertexIndex];
  if (!current) return next;
  current.x += dx;
  current.y += dy;
  const last = next.length - 1;
  const closed =
    last > 0 &&
    points[0]?.x === points[last]?.x &&
    points[0]?.y === points[last]?.y;
  if (syncClosed && closed && vertexIndex === 0) next[last] = { ...current };
  if (syncClosed && closed && vertexIndex === last) next[0] = { ...current };
  return next;
}

export function serializeModelicaPoints(points: Point[]): string {
  return `{${points.map((point) => `{${formatNumber(point.x)},${formatNumber(point.y)}}`).join(",")}}`;
}
