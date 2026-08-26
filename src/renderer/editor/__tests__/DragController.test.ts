import { describe, expect, it } from "vitest";
import { dragDeltaFromStart, transformWithDragPreview } from "../DragController.js";

const identity = {
  translate: { x: 0, y: 0 },
  scale: { x: 1, y: 1 },
  rotate: 0,
};

describe("drag coordinate math", () => {
  it("uses the pointer-down position instead of accumulating frame deltas", () => {
    const start = { x: 10, y: -20 };
    expect(dragDeltaFromStart(start, { x: 10.5, y: -19.25 })).toEqual({
      x: 0.5,
      y: 0.75,
    });
    expect(dragDeltaFromStart(start, { x: 12, y: -17 })).toEqual({
      x: 2,
      y: 3,
    });
  });

  it("only applies grid snapping when explicitly requested", () => {
    const start = { x: 0, y: 0 };
    expect(dragDeltaFromStart(start, { x: 4, y: -6 })).toEqual({ x: 4, y: -6 });
    expect(dragDeltaFromStart(start, { x: 4, y: -6 }, 10)).toEqual({ x: 0, y: -10 });
  });

  it("changes the preview transform only for the active graphic", () => {
    const preview = { graphicId: "B", dx: 12, dy: -7 };
    expect(transformWithDragPreview("A", identity, preview)).toBe(identity);
    expect(transformWithDragPreview("C", identity, preview)).toBe(identity);
    expect(transformWithDragPreview("B", identity, preview)).toEqual({
      ...identity,
      translate: { x: 12, y: -7 },
    });
  });
});
