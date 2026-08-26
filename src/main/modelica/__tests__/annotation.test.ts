import { describe, it, expect } from "vitest";
import { parseAnnotationSlice, findIconCall } from "../annotation.js";

describe("annotation parser", () => {
  it("should parse annotation with Icon and graphics", () => {
    const src = `annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-80,-50},{80,50}})}))`;
    const anno = parseAnnotationSlice(src);
    expect(anno).not.toBeNull();
    expect(anno!.name).toBe("annotation");
    const icon = findIconCall(anno!);
    expect(icon).not.toBeNull();
    expect(icon!.name).toBe("Icon");
  });

  it("should parse nested arrays for extent", () => {
    const src = `annotation(Icon(graphics={Rectangle(extent={{-80,-50},{80,50}}, lineColor={0,0,255})}))`;
    const anno = parseAnnotationSlice(src)!;
    const icon = findIconCall(anno)!;
    const rectArg = icon.arguments.find((a) => a.name === "graphics")?.value;
    expect(rectArg?.type).toBe("array");
    if (rectArg && rectArg.type === "array") {
      const first = rectArg.items[0];
      expect(first.type).toBe("call");
    }
  });

  it("should parse negative coordinates", () => {
    const src = `Rectangle(extent={{-100,-100},{100,100}}, lineColor={0,0,0})`;
    const call = parseAnnotationSlice(src)!;
    expect(call.name).toBe("Rectangle");
    const extent = call.arguments.find((a) => a.name === "extent")?.value;
    expect(extent?.type).toBe("array");
  });

  it("should handle numbers with decimals and exponent", () => {
    const src = `Rectangle(extent={{-80.5,-50.25},{80,50}}, lineThickness=1.5)`;
    const call = parseAnnotationSlice(src)!;
    expect(call.name).toBe("Rectangle");
    const thick = call.arguments.find((a) => a.name === "lineThickness")?.value;
    expect(thick?.type).toBe("number");
    if (thick && thick.type === "number") expect(thick.value).toBe(1.5);
  });

  it("should parse Text with string and %name", () => {
    const src = `Text(extent={{-80,60},{80,90}}, textString="%name")`;
    const call = parseAnnotationSlice(src)!;
    expect(call.name).toBe("Text");
    const txt = call.arguments.find((a) => a.name === "textString")?.value;
    expect(txt?.type).toBe("string");
    if (txt && txt.type === "string") expect(txt.value).toBe("%name");
  });

  it("should parse Line points as nested arrays", () => {
    const src = `Line(points={{-80,0},{0,50},{80,0}}, color={0,0,255})`;
    const call = parseAnnotationSlice(src)!;
    expect(call.name).toBe("Line");
    const pts = call.arguments.find((a) => a.name === "points")?.value;
    expect(pts?.type).toBe("array");
    if (pts && pts.type === "array") expect(pts.items.length).toBe(3);
  });

  it("should ignore unknown properties gracefully via resolver", () => {
    const src = `annotation(Icon(graphics={Rectangle(extent={{0,0},{10,10}}, unknownProp=123, lineColor={0,0,0})}))`;
    const anno = parseAnnotationSlice(src)!;
    expect(anno).not.toBeNull();
    // parsing itself should succeed even with unknown prop
    const icon = findIconCall(anno)!;
    expect(icon.arguments.some((a) => a.name === "graphics")).toBe(true);
  });

  it("should handle empty graphics", () => {
    const src = `annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={}))`;
    const anno = parseAnnotationSlice(src)!;
    const icon = findIconCall(anno)!;
    const g = icon.arguments.find((a) => a.name === "graphics")?.value;
    expect(g?.type).toBe("array");
    if (g && g.type === "array") expect(g.items.length).toBe(0);
  });

  it("should preserve qualified enum values as qualifiedName AST nodes", () => {
    const anno = parseAnnotationSlice(
      "annotation(Icon(graphics={Line(points={{0,0},{10,10}}, smooth=Smooth.Bezier, pattern=LinePattern.None)}))",
    )!;
    const icon = findIconCall(anno)!;
    const line = icon.arguments.find((a) => a.name === "graphics")!.value;
    expect(line.type).toBe("array");
    if (line.type === "array") {
      const smooth = line.items[0]!.type === "call"
        ? line.items[0]!.call.arguments.find((a) => a.name === "smooth")!.value
        : null;
      expect(smooth?.type).toBe("qualifiedName");
      if (smooth?.type === "qualifiedName") {
        expect(smooth.parts).toEqual(["Smooth", "Bezier"]);
      }
    }
  });

  it("should parse scientific notation in graphic origins", () => {
    const call = parseAnnotationSlice(
      "Ellipse(origin={1.77636e-15,-8.88178e-16}, extent={{-10,-10},{10,10}})",
    )!;
    const origin = call.arguments.find((a) => a.name === "origin")?.value;
    expect(origin?.type).toBe("array");
    if (origin?.type === "array") {
      expect(origin.items[0]).toMatchObject({ type: "number", value: 1.77636e-15 });
      expect(origin.items[1]).toMatchObject({ type: "number", value: -8.88178e-16 });
    }
  });
});
