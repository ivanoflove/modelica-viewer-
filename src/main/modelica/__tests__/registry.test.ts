import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { parseModelicaFile } from "../parser.js";
import { ModelicaLibraryRegistry, typeNameCandidates } from "../registry.js";
import { buildClassIndex, resolveDiagramLayerForClass, resolveIconForClass } from "../iconResolver.js";

describe("ModelicaLibraryRegistry", () => {
  it("resolves relative type names from enclosing package scopes", () => {
    const source = `within IEH_CPP;
      package Interfaces package FluidInterfaces connector FluidPortIN end FluidPortIN; end FluidInterfaces; end Interfaces;
      package FluidUnits model Boundary Interfaces.FluidInterfaces.FluidPortIN port; end Boundary; end FluidUnits;`;
    const parsed = parseModelicaFile(source, "IEH_CPP.mo");
    const registry = new ModelicaLibraryRegistry();
    registry.registerSource("IEH_CPP.mo", source, parsed);
    const boundary = buildClassIndex(parsed.classes).get("IEH_CPP.FluidUnits.Boundary")!;
    expect(typeNameCandidates(boundary, "Interfaces.FluidInterfaces.FluidPortIN")).toContain("IEH_CPP.Interfaces.FluidInterfaces.FluidPortIN");
    expect(registry.resolveFor(boundary, "Interfaces.FluidInterfaces.FluidPortIN")?.target.qualifiedName).toBe("IEH_CPP.Interfaces.FluidInterfaces.FluidPortIN");
  });

  it("resolves a base Icon from a separately registered library source", () => {
    const standard = `within Modelica.Icons;
      partial package Example
        annotation(Icon(graphics={Ellipse(extent={{-80,-80},{80,80}}, fillColor={200,220,255})}));
      end Example;`;
    const app = `within Demo;
      model Pump
        extends Modelica.Icons.Example;
        annotation(Icon(graphics={Text(extent={{-60,-10},{60,10}}, textString="Pump")}));
      end Pump;`;
    const registry = new ModelicaLibraryRegistry();
    registry.registerSource("/msl/Icons/package.mo", standard);
    const parsed = parseModelicaFile(app, "/app/Pump.mo");
    const target = parsed.classes[0]!;
    const resolved = resolveIconForClass(
      target,
      parsed.classes,
      app,
      "Pump",
      new Set<string>(),
      (owner, baseName) => registry.resolveFor(owner, baseName),
    );
    expect(resolved.warnings).toEqual([]);
    expect(resolved.icon?.graphics.map((item) => item.type)).toEqual(["Ellipse", "Text"]);
  });

  it("keeps own graphics when an external base is not registered", () => {
    const source = `model Pump
      extends Modelica.Icons.Example;
      annotation(Icon(graphics={Text(extent={{-60,-10},{60,10}}, textString="Pump")}));
    end Pump;`;
    const parsed = parseModelicaFile(source, "Pump.mo");
    const resolved = resolveIconForClass(parsed.classes[0]!, parsed.classes, source, "Pump");
    expect(resolved.icon?.graphics.map((item) => item.type)).toEqual(["Rectangle", "Text"]);
    expect(resolved.warnings).toContain("Base icon not resolved: Modelica.Icons.Example");
  });

  it("resolves core classes from the bundled Modelica 4.1.0 library", () => {
    const root = join(process.cwd(), "resources/modelica/msl-4.1.0/Modelica");
    if (!existsSync(join(root, "package.mo"))) return;
    const registry = new ModelicaLibraryRegistry();
    const info = registry.addRoot(root, {
      name: "Modelica",
      version: "4.1.0",
      builtin: true,
      readOnly: true,
    });
    expect(info).toMatchObject({ name: "Modelica", version: "4.1.0", builtin: true, readOnly: true });
    const realInput = registry.resolve("Modelica.Blocks.Interfaces.RealInput");
    const realOutput = registry.resolve("Modelica.Blocks.Interfaces.RealOutput");
    const constant = registry.resolve("Modelica.Blocks.Sources.Constant");
    expect(realInput?.target.kind).toBe("connector");
    expect(realOutput?.target.kind).toBe("connector");
    expect(constant?.target.kind).toBe("block");
    expect(registry.resolve("Modelica.Icons.Example")?.target).toBeDefined();
    expect(realInput && resolveIconForClass(realInput.target, realInput.allClasses, realInput.source, "u").icon?.graphics.length).toBeGreaterThan(0);
    expect(realInput && resolveDiagramLayerForClass(realInput.target, realInput.source, "u")?.graphics.length).toBeGreaterThan(0);
    expect(constant && resolveIconForClass(constant.target, constant.allClasses, constant.source, "const").icon?.graphics.length).toBeGreaterThan(0);
    expect(registry.isReadOnlyPath(join(root, "Blocks/Interfaces.mo"))).toBe(true);
  });
});
