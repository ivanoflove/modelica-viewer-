import { describe, it, expect } from "vitest";
import {
  snap,
  translateExtent,
  translatePoints,
  applyTransform,
  boundsOf,
  toSvgTransform,
} from "../../../renderer/editor/Transform.js";
import { computeDragTranslate } from "../../../renderer/editor/DragController.js";
import type { RectangleDto, LineDto, GraphicTransform } from "../../../shared/modelicaGraphics.js";

describe("editor Transform", () => {
  it("snap rounds to 10 grid", () => {
    expect(snap(23.6)).toBe(20);
    expect(snap(15)).toBe(20);
    expect(snap(4)).toBe(0);
    expect(snap(-13)).toBe(-10);
  });

  it("translateExtent moves both points", () => {
    const r = translateExtent({ p1: { x: -80, y: -40 }, p2: { x: 80, y: 40 } }, 20, 10);
    expect(r.p1).toEqual({ x: -60, y: -30 });
    expect(r.p2).toEqual({ x: 100, y: 50 });
  });

  it("translatePoints moves all points", () => {
    const pts = translatePoints([{ x: 0, y: 0 }, { x: 10, y: 5 }], -5, 2);
    expect(pts).toEqual([{ x: -5, y: 2 }, { x: 5, y: 7 }]);
  });

  it("applyTransform moves extent for Rectangle", () => {
    const rect: RectangleDto = { type: "Rectangle", extent: { p1: { x: -50, y: -30 }, p2: { x: 50, y: 30 } } };
    const t: GraphicTransform = { translate: { x: 20, y: 10 }, scale: { x: 1, y: 1 }, rotate: 0 };
    const out = applyTransform(rect, t) as RectangleDto;
    expect(out.extent.p1).toEqual({ x: -30, y: -20 });
    expect(out.extent.p2).toEqual({ x: 70, y: 40 });
    // original untouched (pure)
    expect(rect.extent.p1).toEqual({ x: -50, y: -30 });
  });

  it("applyTransform moves points for Line", () => {
    const line: LineDto = { type: "Line", points: [{ x: -80, y: 0 }, { x: 80, y: 0 }] };
    const t: GraphicTransform = { translate: { x: 10, y: 5 }, scale: { x: 1, y: 1 }, rotate: 0 };
    const out = applyTransform(line, t) as LineDto;
    expect(out.points).toEqual([{ x: -70, y: 5 }, { x: 90, y: 5 }]);
  });

  it("boundsOf computes box for extent", () => {
    const rect: RectangleDto = { type: "Rectangle", extent: { p1: { x: -100, y: -50 }, p2: { x: 100, y: 50 } } };
    expect(boundsOf(rect)).toEqual({ x: -100, y: -50, width: 200, height: 100 });
  });

  it("boundsOf pads box for points", () => {
    const line: LineDto = { type: "Line", points: [{ x: 0, y: 0 }, { x: 40, y: 30 }] };
    const b = boundsOf(line)!;
    expect(b.x).toBe(-2);
    expect(b.y).toBe(-2);
    expect(b.width).toBe(44);
    expect(b.height).toBe(34);
  });

  it("toSvgTransform builds attribute string", () => {
    const t: GraphicTransform = { translate: { x: 20, y: 10 }, scale: { x: 1, y: 1 }, rotate: 0 };
    expect(toSvgTransform(t)).toBe("translate(20,10)");
    const t2: GraphicTransform = { translate: { x: 0, y: 0 }, scale: { x: 1, y: 1 }, rotate: 0 };
    expect(toSvgTransform(t2)).toBe("");
  });
});

describe("editor DragController", () => {
  it("computeDragTranslate applies snap to raw delta", () => {
    const drag = {
      id: "x",
      pointerStart: { x: 100, y: 100 },
      transformStart: { translate: { x: 0, y: 0 }, scale: { x: 1, y: 1 }, rotate: 0 },
    };
    const out = computeDragTranslate(drag, { x: 123.6, y: 114 }, 10);
    expect(out).toEqual({ x: 20, y: 10 });
  });

  it("computeDragTranslate accumulates onto existing transform translate", () => {
    const drag = {
      id: "x",
      pointerStart: { x: 0, y: 0 },
      transformStart: { translate: { x: 50, y: 0 }, scale: { x: 1, y: 1 }, rotate: 0 },
    };
    const out = computeDragTranslate(drag, { x: 35, y: 0 }, 10);
    expect(out).toEqual({ x: 90, y: 0 });
  });
});
