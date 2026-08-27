import { describe, expect, it } from "vitest";
import { parseModelicaFile } from "../parser.js";
import {
  findClassByQualifiedName,
  resolveIconForClass,
} from "../iconResolver.js";

const modelNames = [
  "WaterPump",
  "Compressor",
  "Boundary",
  "BoundarySig",
  "Heater",
  "Dryer",
  "HeatX",
  "HeatXNTU",
  "HeatXEnth",
  "Mixer",
  "Split",
  "FluidLag",
  "Flash",
] as const;

const modelSource = modelNames
  .map((name) => {
    const graphics = name === "Flash"
      ? "Ellipse(origin={1.77636e-15,-8.88178e-16}, extent={{-50,-50},{50,50}})"
      : name === "Compressor"
        ? "Polygon(fillColor={225,249,255}, fillPattern=FillPattern.HorizontalCylinder, points={{-66,54},{-66,-54},{66,-30},{66,30},{-66,54}})"
        : name === "Boundary" || name === "BoundarySig"
          ? "Rectangle(extent={{-100,-100},{100,100}}), Text(extent={{-80,-20},{80,20}}, textString=\"%bound\", textStyle={TextStyle.Bold})"
          : "Ellipse(origin={0,14}, fillColor={225,251,255}, fillPattern=FillPattern.Sphere, extent={{-50,50},{50,-50}}), Polygon(pattern=LinePattern.None, fillPattern=FillPattern.VerticalCylinder, points={{-29,-10},{-50,-45},{50,-45},{29,-10},{-29,-10}}), Line(origin={-59.95,9.83}, points={{9.94721,2.17082},{-10.0528,2.17082}}, thickness=2)";
    return `model ${name}
      annotation(Placement(transformation(origin={0,0}, extent={{-10,-10},{10,10}})));
      annotation(Icon(graphics={${graphics}}));
    end ${name};`;
  })
  .join("\n");

const source = `package IEH_CPP
  package FluidUnits
    ${modelSource}
  end FluidUnits;
end IEH_CPP;`;

describe("FluidUnits icon regressions", () => {
  it("resolves the real FluidUnits graphic syntax used by IEH_CPP", () => {
    const parsed = parseModelicaFile(source, "IEH_CPP.mo");
    for (const name of modelNames) {
      const qualifiedName = `IEH_CPP.FluidUnits.${name}`;
      const cls = findClassByQualifiedName(parsed.classes, qualifiedName);
      expect(cls, qualifiedName).not.toBeNull();
      const resolved = resolveIconForClass(cls!, parsed.classes, source, name);
      expect(resolved.icon?.graphics.length, qualifiedName).toBeGreaterThan(0);
    }
  });

  it("keeps the key WaterPump, Boundary and Flash primitives", () => {
    const parsed = parseModelicaFile(source, "IEH_CPP.mo");
    const get = (name: string) => {
      const cls = findClassByQualifiedName(parsed.classes, `IEH_CPP.FluidUnits.${name}`)!;
      return resolveIconForClass(cls, parsed.classes, source, name).icon!;
    };
    expect(get("WaterPump").graphics.map((g) => g.type)).toEqual([
      "Ellipse",
      "Polygon",
      "Line",
    ]);
    expect(get("Boundary").graphics.map((g) => g.type)).toEqual([
      "Rectangle",
      "Text",
    ]);
    expect(get("Flash").graphics[0]).toMatchObject({
      type: "Ellipse",
      origin: { x: 1.77636e-15, y: -8.88178e-16 },
    });
  });

  it("preserves Compressor local Polygon coordinates during Icon resolution", () => {
    const parsed = parseModelicaFile(source, "IEH_CPP.mo");
    const cls = findClassByQualifiedName(parsed.classes, "IEH_CPP.FluidUnits.Compressor")!;
    const icon = resolveIconForClass(cls, parsed.classes, source, "Compressor").icon!;
    const polygon = icon.graphics.find((graphic) => graphic.type === "Polygon");
    expect(polygon).toMatchObject({
      points: [
        { x: -66, y: 54 },
        { x: -66, y: -54 },
        { x: 66, y: -30 },
        { x: 66, y: 30 },
        { x: -66, y: 54 },
      ],
    });
    expect(icon.coordinateSystem.extent).toEqual({
      p1: { x: -100, y: -100 },
      p2: { x: 100, y: 100 },
    });
  });
});
