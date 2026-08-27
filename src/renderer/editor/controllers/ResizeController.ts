import type { Extent, Point } from "../../../shared/modelicaGraphics.js";

export interface ResizeViewportState {
  base: { x: number; y: number; width: number; height: number };
  viewBox: { x: number; y: number; width: number; height: number };
}

export const resizeHandles = [
  "nw", "n", "ne", "w", "e", "sw", "s", "se",
] as const;

export type ResizeHandle = (typeof resizeHandles)[number];

export interface ResizeSession {
  graphicId: string;
  handle: ResizeHandle;
  pointerId: number;
  startPointerModel: Point;
  originalExtent: Extent;
  viewport: ResizeViewportState;
}

function normalized(extent: Extent): { left: number; right: number; bottom: number; top: number } {
  return {
    left: Math.min(extent.p1.x, extent.p2.x),
    right: Math.max(extent.p1.x, extent.p2.x),
    bottom: Math.min(extent.p1.y, extent.p2.y),
    top: Math.max(extent.p1.y, extent.p2.y),
  };
}

/** Resize from the original extent; no previous-frame accumulation. */
export function resizeExtent(
  original: Extent,
  handle: ResizeHandle,
  delta: Point,
  keepAspectRatio = false,
  symmetric = false,
): Extent {
  const start = normalized(original);
  let { left, right, bottom, top } = start;
  const horizontal = handle.includes("w") || handle.includes("e");
  const vertical = handle.includes("n") || handle.includes("s");

  if (handle.includes("w")) {
    left += delta.x;
    if (symmetric) right -= delta.x;
  }
  if (handle.includes("e")) {
    right += delta.x;
    if (symmetric) left -= delta.x;
  }
  if (handle.includes("s")) {
    bottom += delta.y;
    if (symmetric) top -= delta.y;
  }
  if (handle.includes("n")) {
    top += delta.y;
    if (symmetric) bottom -= delta.y;
  }

  if (keepAspectRatio && horizontal && vertical) {
    const ratio = Math.max(start.right - start.left, 1e-9) / Math.max(start.top - start.bottom, 1e-9);
    const width = Math.abs(right - left);
    const height = Math.abs(top - bottom);
    if (width / Math.max(height, 1e-9) > ratio) {
      const correctedHeight = width / ratio;
      if (handle.includes("s")) bottom = top - correctedHeight;
      else if (handle.includes("n")) top = bottom + correctedHeight;
      else {
        const center = (bottom + top) / 2;
        bottom = center - correctedHeight / 2;
        top = center + correctedHeight / 2;
      }
    } else {
      const correctedWidth = height * ratio;
      if (handle.includes("w")) left = right - correctedWidth;
      else if (handle.includes("e")) right = left + correctedWidth;
      else {
        const center = (left + right) / 2;
        left = center - correctedWidth / 2;
        right = center + correctedWidth / 2;
      }
    }
  }

  const p1 = { x: Math.min(left, right), y: Math.min(bottom, top) };
  const p2 = { x: Math.max(left, right), y: Math.max(bottom, top) };
  return { p1, p2 };
}
