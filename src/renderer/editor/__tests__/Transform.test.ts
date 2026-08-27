import { describe, expect, it } from "vitest";
import { boundsOf, applyTransform } from "../Transform.js";
import { graphicLocalToModel, modelToGraphicLocal } from "../Coordinates.js";

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

  it("keeps point geometry local when moving an origin-based Polygon", () => {
    const polygon = {
      type: "Polygon" as const,
      origin: { x: 12, y: -7 },
      points: [
        { x: -66, y: 54 },
        { x: -66, y: -54 },
        { x: 66, y: -30 },
        { x: 66, y: 30 },
        { x: -66, y: 54 },
      ],
    };
    const moved = applyTransform(polygon, {
      translate: { x: -8, y: 14 },
      scale: { x: 1, y: 1 },
      rotate: 0,
    });
    expect(moved).toMatchObject({ origin: { x: 4, y: 7 } });
    expect((moved as typeof polygon).points).toEqual(polygon.points);
  });

  it("round-trips GraphicLocal points without using viewport state", () => {
    const graphic = {
      type: "Polygon" as const,
      origin: { x: 12, y: -7 },
      points: [{ x: -66, y: 54 }],
    };
    const transform = {
      translate: { x: -8, y: 14 },
      scale: { x: -1.5, y: 0.75 },
      rotate: 35,
    };
    const local = { x: -66, y: 54 };
    const model = graphicLocalToModel(local, graphic, transform);
    expect(modelToGraphicLocal(model, graphic, transform).x).toBeCloseTo(local.x, 10);
    expect(modelToGraphicLocal(model, graphic, transform).y).toBeCloseTo(local.y, 10);
  });
});
