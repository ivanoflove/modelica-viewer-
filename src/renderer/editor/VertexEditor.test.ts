import { describe, expect, it } from "vitest";
import { moveVertex, serializeModelicaPoints } from "./VertexEditor";

describe("VertexEditor", () => {
  it("moves only the requested Line point", () => {
    const points = [{ x: -10, y: 0 }, { x: 0, y: 5 }, { x: 10, y: 0 }];
    expect(moveVertex(points, 1, 3, -2)).toEqual([
      { x: -10, y: 0 },
      { x: 3, y: 3 },
      { x: 10, y: 0 },
    ]);
    expect(points[1]).toEqual({ x: 0, y: 5 });
  });

  it("keeps a closed Polygon's first and last point together", () => {
    const points = [
      { x: 0, y: 0 },
      { x: 10, y: 0 },
      { x: 0, y: 0 },
    ];
    expect(moveVertex(points, 0, 2, 4, true)).toEqual([
      { x: 2, y: 4 },
      { x: 10, y: 0 },
      { x: 2, y: 4 },
    ]);
    expect(moveVertex(points, 0, 2, 4)).toEqual([
      { x: 2, y: 4 },
      { x: 10, y: 0 },
      { x: 0, y: 0 },
    ]);
  });

  it("serializes one complete Modelica points property", () => {
    expect(serializeModelicaPoints([{ x: -1.25, y: 2 }, { x: 3, y: 4.5 }])).toBe(
      "{{-1.25,2},{3,4.5}}",
    );
  });
});
