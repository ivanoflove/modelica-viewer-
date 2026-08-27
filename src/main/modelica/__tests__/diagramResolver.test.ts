import { describe, expect, it } from "vitest";
import { parseModelicaFile } from "../parser.js";
import { resolveDiagramForClass } from "../diagramResolver.js";
import { buildClassIndex } from "../iconResolver.js";
import { computePlacementTransform, transformPlacementPoint } from "../../../shared/diagram.js";

describe("Diagram M1 resolver", () => {
  it("indexes every freshly parsed qualified class, including nested classes", () => {
    const parsed = parseModelicaFile("within Demo; package Sub model Inner end Inner; end Sub;", "demo.mo");
    const index = buildClassIndex(parsed.classes);
    expect(Array.from(index.keys())).toEqual(["Demo.Sub", "Demo.Sub.Inner"]);
  });

  it("extracts placed components and resolves each component Icon", () => {
    const source = `
      within Demo;
      model Pump
        annotation(Icon(graphics={Ellipse(extent={{-100,-50},{100,50}})}));
      end Pump;
      model System
        Pump pump annotation(Placement(transformation(origin={-40,0}, extent={{-20,-10},{20,10}}, rotation=90)));
      end System;
    `;
    const parsed = parseModelicaFile(source, "demo.mo");
    const pump = parsed.classes.find((item) => item.name === "Pump")!;
    const system = parsed.classes.find((item) => item.name === "System")!;
    const scene = resolveDiagramForClass(system, source, (_owner, typeName) =>
      typeName === "Pump"
        ? { target: pump, allClasses: parsed.classes, source }
        : null,
    );
    expect(scene.components).toHaveLength(1);
    expect(scene.components[0]?.name).toBe("pump");
    expect(scene.components[0]?.resolvedIcon?.graphics[0]?.type).toBe("Ellipse");
    expect(scene.components[0]?.placement?.transformation?.rotation).toBe(90);
  });

  it("maps icon coordinates into a placement extent and rotation", () => {
    const icon = { extent: { p1: { x: -100, y: -100 }, p2: { x: 100, y: 100 } } };
    const transformation = {
      origin: { x: 40, y: 20 },
      extent: { p1: { x: -10, y: -20 }, p2: { x: 10, y: 20 } },
      rotation: 90,
    };
    expect(computePlacementTransform(icon, transformation)).toContain("rotate(90)");
    expect(transformPlacementPoint({ x: 0, y: -100 }, icon, transformation)).toEqual({ x: 60, y: 20 });
  });

  it("reports resolvable components without Placement without rendering them", () => {
    const source = "model Pump annotation(Icon(graphics={})); end Pump; model System Pump pump; end System;";
    const parsed = parseModelicaFile(source, "demo.mo");
    const pump = parsed.classes.find((item) => item.name === "Pump")!;
    const system = parsed.classes.find((item) => item.name === "System")!;
    const scene = resolveDiagramForClass(system, source, (_owner, typeName) =>
      typeName === "Pump" ? { target: pump, allClasses: parsed.classes, source } : null,
    );
    expect(scene.components[0]?.placement).toBeUndefined();
    expect(scene.diagnostics[0]).toContain("no Placement");
  });
});
