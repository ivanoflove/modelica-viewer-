import { describe, it, expect } from "vitest";
import { parseAnnotationSlice, findIconCall } from "../annotation.js";
import { resolveIcon } from "../iconResolver.js";

function resolveIconFromAnnotation(
  src: string,
  modelName = "MyModel",
): ReturnType<typeof resolveIcon> {
  const anno = parseAnnotationSlice(src)!;
  const iconCall = findIconCall(anno)!;
  return resolveIcon(iconCall, modelName);
}

describe("iconResolver", () => {
  it("should resolve Rectangle", () => {
    const src = `annotation(Icon(graphics={Rectangle(extent={{-80,-50},{80,50}}, lineColor={0,0,255}, fillColor={220,220,255})}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.graphics[0]?.type).toBe("Rectangle");
    const rect = icon.graphics[0] as any;
    expect(rect.extent.p1).toEqual({ x: -80, y: -50 });
    expect(rect.extent.p2).toEqual({ x: 80, y: 50 });
    expect(rect.lineColor).toEqual([0, 0, 255]);
    expect(rect.fillColor).toEqual([220, 220, 255]);
  });

  it("should resolve Ellipse", () => {
    const src = `annotation(Icon(graphics={Ellipse(extent={{-30,-30},{30,30}}, lineColor={0,0,0})}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.graphics[0]?.type).toBe("Ellipse");
  });

  it("should resolve Line", () => {
    const src = `annotation(Icon(graphics={Line(points={{-80,0},{0,50},{80,0}}, color={0,0,255})}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.graphics[0]?.type).toBe("Line");
    const line = icon.graphics[0] as any;
    expect(line.points).toEqual([
      { x: -80, y: 0 },
      { x: 0, y: 50 },
      { x: 80, y: 0 },
    ]);
    expect(line.color).toEqual([0, 0, 255]);
  });

  it("should resolve Polygon", () => {
    const src = `annotation(Icon(graphics={Polygon(points={{-40,-40},{40,-40},{0,40}}, lineColor={0,0,0}, fillColor={255,0,0})}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.graphics[0]?.type).toBe("Polygon");
  });

  it("should resolve Text with %name", () => {
    const src = `annotation(Icon(graphics={Text(extent={{-80,60},{80,90}}, textString="%name", textColor={0,0,255})}))`;
    const icon = resolveIconFromAnnotation(src, "Resistor")!;
    expect(icon.graphics[0]?.type).toBe("Text");
    const text = icon.graphics[0] as any;
    expect(text.textString).toBe("Resistor");
    expect(text.textColor).toEqual([0, 0, 255]);
  });

  it("should handle negative coordinates", () => {
    const src = `annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-100,-100},{100,100}})}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.coordinateSystem.extent.p1).toEqual({ x: -100, y: -100 });
    expect(icon.coordinateSystem.extent.p2).toEqual({ x: 100, y: 100 });
  });

  it("should handle RGB color and unknown props ignored", () => {
    const src = `annotation(Icon(graphics={Rectangle(extent={{0,0},{10,10}}, lineColor={1,2,3}, unknownProp=999, fillPattern=FillPattern.Solid)}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.graphics[0]?.type).toBe("Rectangle");
    // unknownProp should be ignored, lineColor should still be parsed
    const rect = icon.graphics[0] as any;
    expect(rect.lineColor).toEqual([1, 2, 3]);
  });

  it("should handle multiple graphics", () => {
    const src = `annotation(Icon(graphics={Rectangle(extent={{-80,-50},{80,50}}), Ellipse(extent={{-30,-30},{30,30}}), Text(extent={{-80,60},{80,90}}, textString="hello")}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.graphics.length).toBe(3);
    expect(icon.graphics.map((g) => g.type)).toEqual([
      "Rectangle",
      "Ellipse",
      "Text",
    ]);
  });

  it("should return null for missing Icon", () => {
    const src = `annotation(Documentation(info="<html>hello</html>"))`;
    const anno = parseAnnotationSlice(src)!;
    const iconCall = findIconCall(anno);
    expect(iconCall).toBeNull();
  });

  it("should handle coordinateSystem extent", () => {
    const src = `annotation(Icon(coordinateSystem(extent={{-50,-50},{50,50}}, preserveAspectRatio=true), graphics={Rectangle(extent={{0,0},{10,10}})}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.coordinateSystem.extent).toEqual({
      p1: { x: -50, y: -50 },
      p2: { x: 50, y: 50 },
    });
    expect(icon.coordinateSystem.preserveAspectRatio).toBe(true);
  });
});
