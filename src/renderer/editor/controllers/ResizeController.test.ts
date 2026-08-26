import { describe, expect, it } from "vitest";
import { resizeExtent } from "./ResizeController.js";

const extent = { p1: { x: -10, y: -5 }, p2: { x: 10, y: 5 } };

describe("resize geometry", () => {
  it("resizes each edge from the original extent", () => {
    expect(resizeExtent(extent, "e", { x: 4, y: 0 })).toEqual({
      p1: { x: -10, y: -5 }, p2: { x: 14, y: 5 },
    });
    expect(resizeExtent(extent, "nw", { x: -3, y: 2 })).toEqual({
      p1: { x: -13, y: -5 }, p2: { x: 10, y: 7 },
    });
  });

  it("supports symmetric and proportional resize without frame accumulation", () => {
    expect(resizeExtent(extent, "e", { x: 4, y: 0 }, false, true)).toEqual({
      p1: { x: -14, y: -5 }, p2: { x: 14, y: 5 },
    });
    const proportional = resizeExtent(extent, "se", { x: 10, y: -2 }, true);
    expect((proportional.p2.x - proportional.p1.x) / (proportional.p2.y - proportional.p1.y)).toBeCloseTo(2);
  });

  it("normalizes extents when a handle crosses the opposite edge", () => {
    expect(resizeExtent(extent, "e", { x: -30, y: 0 })).toEqual({
      p1: { x: -20, y: -5 }, p2: { x: -10, y: 5 },
    });
  });
});
