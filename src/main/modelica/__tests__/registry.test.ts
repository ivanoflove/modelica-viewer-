import { describe, expect, it } from "vitest";
import { parseModelicaFile } from "../parser.js";
import { ModelicaLibraryRegistry } from "../registry.js";
import { resolveIconForClass } from "../iconResolver.js";

describe("ModelicaLibraryRegistry", () => {
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
});
