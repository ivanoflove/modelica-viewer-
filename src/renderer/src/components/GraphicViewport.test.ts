import { describe, expect, it } from "vitest";
import {
  contentViewBox,
  clientToModelicaWithViewport,
  modelToViewportRoot,
  panViewBox,
  viewportGroupTransform,
  wheelZoomFactor,
  zoomViewBox,
} from "./GraphicViewport";
import type { IconDto } from "../../../shared/modelicaGraphics";

const icon: IconDto = {
  coordinateSystem: { extent: { p1: { x: -100, y: -100 }, p2: { x: 100, y: 100 } } },
  graphics: [{
    type: "Rectangle",
    origin: { x: 20, y: -10 },
    extent: { p1: { x: -5, y: -4 }, p2: { x: 5, y: 4 } },
  }],
};

describe("GraphicViewport math", () => {
  it("fits content with padding and includes origin", () => {
    const view = contentViewBox(icon);
    expect(view.x).toBeLessThan(15);
    expect(view.y).toBeLessThan(-14);
    expect(view.width).toBeGreaterThan(10);
    expect(view.height).toBeGreaterThan(8);
  });

  it("keeps the wheel anchor fixed while zooming", () => {
    const current = { x: -100, y: -100, width: 200, height: 200 };
    const next = zoomViewBox(current, current, 2, { x: 30, y: -20 });
    const beforeRatio = (30 - current.x) / current.width;
    const afterRatio = (30 - next.x) / next.width;
    expect(afterRatio).toBeCloseTo(beforeRatio);
    expect(((-20 - current.y) / current.height)).toBeCloseTo((-20 - next.y) / next.height);
  });

  it("pans in screen space without changing zoom", () => {
    const start = { x: -100, y: -100, width: 200, height: 200 };
    expect(panViewBox(start, 40, -20, 400, 400)).toEqual({ x: -120, y: -90, width: 200, height: 200 });
  });

  it("only enables wheel zoom when Ctrl is pressed", () => {
    expect(wheelZoomFactor(100, false)).toBeNull();
    expect(wheelZoomFactor(100, true)).toBeCloseTo(Math.exp(-0.15));
  });

  it("maps the stable base viewBox to a zoomed and panned group transform", () => {
    expect(
      viewportGroupTransform(
        { x: -100, y: -100, width: 200, height: 200 },
        { x: -40, y: -50, width: 100, height: 100 },
      ),
    ).toBe("matrix(2 0 0 2 -20 0)");
  });

  it("maps Modelica points through viewport zoom and the flipped Y axis", () => {
    const viewport = {
      base: { x: -100, y: -100, width: 200, height: 200 },
      viewBox: { x: -40, y: -50, width: 100, height: 100 },
    };
    expect(modelToViewportRoot({ x: 10, y: 20 }, viewport)).toEqual({
      x: 0,
      y: -40,
    });
  });

  it("round-trips client coordinates with letterboxed SVG geometry", () => {
    const svg = {
      getBoundingClientRect: () => ({
        left: 100,
        top: 50,
        width: 800,
        height: 400,
      }),
    } as SVGSVGElement;
    const viewport = {
      base: { x: -100, y: -100, width: 200, height: 200 },
      viewBox: { x: -100, y: -100, width: 200, height: 200 },
    };
    const screen = modelToViewportRoot({ x: 20, y: 30 }, viewport);
    const client = {
      x: 100 + 200 + (screen.x - viewport.base.x) * 2,
      y: 50 + (screen.y - viewport.base.y) * 2,
    };
    expect(clientToModelicaWithViewport(svg, client.x, client.y, viewport)).toEqual({
      x: 20,
      y: 30,
    });
  });
});
