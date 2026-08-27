import { describe, it, expect } from "vitest";
import { parseAnnotationSlice, findIconCall } from "../annotation.js";
import { extractIconFromSlice, resolveIcon, resolveIconForClass } from "../iconResolver.js";
import { parseModelicaFile } from "../parser.js";

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

  it("should preserve origin, fillPattern, smooth and textStyle with safe fallbacks", () => {
    const src = `annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}, grid={2,2}, initialScale=0.2), graphics={Rectangle(origin={10,20}, extent={{-10,-10},{10,10}}, fillColor={255,0,0}, fillPattern=FillPattern.Sphere), Line(origin={-5,0}, points={{0,0},{20,20}}, smooth=Smooth.Bezier), Text(origin={0,5}, extent={{-20,-10},{20,10}}, textString="%comps", textStyle={TextStyle.Bold})}))`;
    const icon = resolveIconFromAnnotation(src, "MediumWorld")!;
    expect(icon.coordinateSystem.grid).toEqual({ x: 2, y: 2 });
    expect(icon.coordinateSystem.initialScale).toBe(0.2);
    expect(icon.graphics[0]).toMatchObject({
      origin: { x: 10, y: 20 },
      fillPattern: "FillPattern.Sphere",
    });
    expect(icon.graphics[1]).toMatchObject({
      origin: { x: -5, y: 0 },
      smooth: "Smooth.Bezier",
    });
    expect(icon.graphics[2]).toMatchObject({
      origin: { x: 0, y: 5 },
      textStyle: ["TextStyle.Bold"],
      textString: "%comps",
    });
  });

  it("should find Icon after earlier Placement annotations", () => {
    const src = `model WaterPump
      annotation(Placement(transformation(origin={-54,0}, extent={{-10,-10},{10,10}})));
      annotation(Documentation(info="<html>package and annotation are text</html>"));
      annotation(Icon(graphics={
        Ellipse(origin={0,14}, fillColor={225,251,255}, fillPattern=FillPattern.Sphere, extent={{-50,50},{50,-50}}),
        Polygon(pattern=LinePattern.None, fillPattern=FillPattern.VerticalCylinder, points={{-29,-10},{-50,-45},{50,-45},{29,-10},{-29,-10}}),
        Text(origin={0,-81}, extent={{-48,19},{48,-19}}, textString="%bound", textStyle={TextStyle.Bold})
      }));
    end WaterPump;`;
    const icon = requireIconFromClassSource(src, "WaterPump");
    expect(icon.graphics.map((item) => item.type)).toEqual([
      "Ellipse",
      "Polygon",
      "Text",
    ]);
    expect(icon.graphics[1]).toMatchObject({
      pattern: "LinePattern.None",
      fillPattern: "FillPattern.VerticalCylinder",
    });
    expect(icon.graphics[2]).toMatchObject({ textString: "%bound" });
  });

  it("should retain later graphics when a graphic kind is unsupported", () => {
    const src = `annotation(Icon(graphics={UnknownGraphic(foo=1), Rectangle(extent={{0,0},{10,10}})}))`;
    const icon = resolveIconFromAnnotation(src)!;
    expect(icon.graphics).toHaveLength(1);
    expect(icon.graphics[0]?.type).toBe("Rectangle");
  });

  it("does not collect child class Icons into a package Icon", () => {
    const source = `package P
      annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})}));
      model Child
        annotation(Icon(graphics={Ellipse(extent={{-90,-90},{90,90}})}));
      end Child;
    end P;`;
    const parsed = parseModelicaFile(source, "P.mo");
    const resolved = resolveIconForClass(parsed.classes[0]!, parsed.classes, source, "P");
    expect(resolved.icon?.graphics).toHaveLength(1);
    expect(resolved.icon?.graphics[0]?.type).toBe("Rectangle");
  });

  it("returns no package Icon when only descendants define Icons", () => {
    const source = `package P
      model Child
        annotation(Icon(graphics={Ellipse(extent={{-90,-90},{90,90}})}));
      end Child;
    end P;`;
    const parsed = parseModelicaFile(source, "P.mo");
    const resolved = resolveIconForClass(parsed.classes[0]!, parsed.classes, source, "P");
    expect(resolved.icon).toBeNull();
  });
});

function requireIconFromClassSource(source: string, name: string) {
  // Keep this test focused on annotation extraction while using the same
  // source-slice path as the main process.
  const start = source.indexOf(`model ${name}`);
  const end = source.lastIndexOf(`end ${name};`) + `end ${name};`.length;
  return extractIconFromSlice(source.slice(start, end), name)!;
}
