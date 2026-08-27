import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { parseModelicaFile } from "../parser.js";
import { resolveDiagramForClass } from "../diagramResolver.js";
import { buildClassIndex, resolveIconForClass } from "../iconResolver.js";
import { ModelicaLibraryRegistry } from "../registry.js";
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

  it("inherits the base Icon coordinateSystem when the derived Icon omits it", () => {
    const source = `within Demo;
      model Base annotation(Icon(coordinateSystem(extent={{-200,-100},{200,100}}), graphics={Rectangle(extent={{-200,-100},{200,100}})})); end Base;
      model Derived extends Base;
        annotation(Icon(graphics={Ellipse(extent={{-50,-50},{50,50}})}));
      end Derived;`;
    const parsed = parseModelicaFile(source, "demo.mo");
    const derived = parsed.classes.find((item) => item.name === "Derived")!;
    const resolved = resolveIconForClass(derived, parsed.classes, source, "Derived");
    expect(resolved.icon?.coordinateSystem.extent).toEqual({
      p1: { x: -200, y: -100 },
      p2: { x: 200, y: 100 },
    });
  });

  it("keeps class, component, and connection annotations in separate scene layers", () => {
    const source = `within Demo;
      model Base
        annotation(Icon(graphics={Rectangle(extent={{-20,-20},{20,20}})}));
      end Base;
      model System
        extends Base;
        Base a(value=5e-8) annotation(
          // component annotation comment
          Placement(transformation(origin={40,20}, extent={{-10,-10},{10,10}})));
      equation
        connect(a.port_a[1], a.port_b[2]) annotation(
          Line(origin={1,2}, points={{-10,0},{0,3.5e1},{10,0}}, color={0,0,127}));
      annotation(
        Diagram(graphics={Text(extent={{-80,60},{80,80}}, textString="diagram only")}),
        experiment(StopTime=1));
      end System;`;
    const parsed = parseModelicaFile(source, "demo.mo");
    const base = parsed.classes.find((item) => item.name === "Base")!;
    const system = parsed.classes.find((item) => item.name === "System")!;
    const scene = resolveDiagramForClass(system, source, (_owner, typeName) =>
      typeName === "Base" ? { target: base, allClasses: parsed.classes, source } : null,
    );

    expect(scene.backgroundGraphics.map((graphic) => graphic.type)).toEqual(["Text"]);
    expect(scene.components.map((component) => component.name)).toEqual(["a"]);
    expect(scene.components[0]?.placement?.transformation?.origin).toEqual({ x: 40, y: 20 });
    expect(scene.connections).toHaveLength(1);
    expect(scene.connections[0]).toMatchObject({
      from: "a.port_a[1]",
      to: "a.port_b[2]",
      line: { origin: { x: 1, y: 2 }, points: [{ x: -10, y: 0 }, { x: 0, y: 35 }, { x: 10, y: 0 }] },
    });
    expect((scene.contentBounds?.x ?? 0) + (scene.contentBounds?.width ?? 0)).toBeGreaterThan(40);

    const icon = resolveIconForClass(system, parsed.classes, source, "System");
    expect(icon.icon?.graphics.map((graphic) => graphic.type)).toEqual(["Rectangle"]);
  });

  it("returns an empty Diagram scene when Diagram annotation is absent", () => {
    const source = `model A annotation(Icon(graphics={Ellipse(extent={{-10,-10},{10,10}})})); end A;
      model System
        A a annotation(Placement(transformation(origin={-220,0}, extent={{-10,-10},{10,10}})));
      equation
        connect(a.p, a.n) annotation(Line(points={{-230,0},{-210,0}}));
      end System;`;
    const parsed = parseModelicaFile(source, "demo.mo");
    const type = parsed.classes.find((item) => item.name === "A")!;
    const system = parsed.classes.find((item) => item.name === "System")!;
    const scene = resolveDiagramForClass(system, source, (_owner, typeName) =>
      typeName === "A" ? { target: type, allClasses: parsed.classes, source } : null,
    );

    expect(scene.coordinateSystem.extent).toEqual({ p1: { x: -100, y: -100 }, p2: { x: 100, y: 100 } });
    expect(scene.backgroundGraphics).toEqual([]);
    expect(scene.components).toHaveLength(1);
    expect(scene.connections).toHaveLength(1);
    expect(scene.contentBounds?.x).toBeLessThan(-220);
  });

  it.skipIf(!existsSync("/mnt/d/Documents/Dymola/Model/GH/IEH_CPP/IEH_CPP.mo"))(
    "keeps the real IEH_CPP SOECsys annotations separated",
    () => {
      const file = "/mnt/d/Documents/Dymola/Model/GH/IEH_CPP/IEH_CPP.mo";
      const source = readFileSync(file, "utf8");
      const parsed = parseModelicaFile(source, file);
      const index = buildClassIndex(parsed.classes);
      const target = index.get("IEH_CPP.Converter.Cases.SOECsys");
      expect(target).toBeDefined();
      const registry = new ModelicaLibraryRegistry();
      registry.registerSource(file, source, parsed);
      const scene = resolveDiagramForClass(target!, source, (owner, typeName) =>
        registry.resolveFor(owner, typeName),
      );

      expect(scene.components.map((component) => component.name)).toEqual([
        "medium", "h2grid", "WaterTank", "heater", "mixer", "heatXNTU",
        "rSOC1", "condensor", "flash", "si1", "si2", "air", "heatXNTU1",
        "heater1", "airO", "const",
      ]);
      expect(scene.connections).toHaveLength(16);
      expect(scene.backgroundGraphics).toEqual([]);
      expect(scene.contentBounds?.x).toBeLessThan(-280);
      expect(scene.contentBounds?.x ?? 0).toBeLessThan(0);
    },
  );
});
