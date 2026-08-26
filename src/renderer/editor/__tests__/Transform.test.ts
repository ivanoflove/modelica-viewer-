import { describe, expect, it } from "vitest";
import { boundsOf, applyTransform } from "../Transform.js";

describe("icon editor transforms", () => {
  it("includes graphic origin in its selection bounds", () => {
    const bounds = boundsOf({
      type: "Rectangle",
      origin: { x: 20, y: -10 },
      extent: { p1: { x: -5, y: -4 }, p2: { x: 5, y: 4 } },
    });
    expect(bounds).toEqual({ x: 15, y: -14, width: 10, height: 8 });
  });

  it("moves origin-based graphics without changing their geometry", () => {
    const graphic = {
      type: "Ellipse" as const,
      origin: { x: 1, y: 2 },
      extent: { p1: { x: -5, y: -3 }, p2: { x: 5, y: 3 } },
    };
    expect(applyTransform(graphic, {
      translate: { x: 10, y: -4 },
      scale: { x: 1, y: 1 },
      rotate: 0,
    })).toMatchObject({
      origin: { x: 11, y: -2 },
      extent: graphic.extent,
    });
  });
});
