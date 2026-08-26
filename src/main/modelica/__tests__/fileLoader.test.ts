import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { loadModelicaFile } from "../loader.js";
import {
  findClassByQualifiedName,
  resolveIconForClass,
} from "../iconResolver.js";
import { parseModelicaFile } from "../parser.js";

const fixturePath = join(
  dirname(fileURLToPath(import.meta.url)),
  "fixtures",
  "IEH_CPP.mo",
);

function findPackage(root: ReturnType<typeof loadModelicaFile>, name: string) {
  if (root.name === name) return root;
  const pending = [...root.children];
  while (pending.length > 0) {
    const candidate = pending.shift()!;
    if (candidate.name === name) return candidate;
    pending.push(...candidate.children);
  }
  return undefined;
}

describe("loadModelicaFile", () => {
  it("loads a standalone top-level package without package.mo", () => {
    const root = loadModelicaFile(fixturePath);

    expect(root.name).toBe("IEH_CPP");
    expect(root.qualifiedName).toBe("IEH_CPP");
    expect(root.sourceFile).toBe(fixturePath);
    expect(root.within).toBeNull();

    const medium = findPackage(root, "ThermoMedium");
    expect(medium?.qualifiedName).toBe("IEH_CPP.ThermoMedium");
    expect(findPackage(root, "Types")?.qualifiedName).toBe(
      "IEH_CPP.ThermoMedium.Types",
    );
    expect(findPackage(root, "Functions")?.qualifiedName).toBe(
      "IEH_CPP.ThermoMedium.Functions",
    );
    expect(findPackage(root, "Units")?.qualifiedName).toBe(
      "IEH_CPP.ThermoMedium.Units",
    );
  });

  it("keeps nested classes, source file and source ranges", () => {
    const root = loadModelicaFile(fixturePath);
    const medium = findPackage(root, "ThermoMedium")!;
    const functions = findPackage(root, "Functions")!;
    const units = findPackage(root, "Units")!;
    const create = functions.classes.find((cls) => cls.name === "create")!;
    const free = functions.classes.find((cls) => cls.name === "free")!;
    const world = medium.classes.find((cls) => cls.name === "MediumWorld")!;
    const flash = units.classes.find((cls) => cls.name === "FlashUnit")!;

    expect(create.qualifiedName).toBe("IEH_CPP.ThermoMedium.Functions.create");
    expect(free.qualifiedName).toBe("IEH_CPP.ThermoMedium.Functions.free");
    expect(world.qualifiedName).toBe("IEH_CPP.ThermoMedium.MediumWorld");
    expect(flash.qualifiedName).toBe("IEH_CPP.ThermoMedium.Units.FlashUnit");
    for (const cls of [create, free, world, flash]) {
      expect(cls.sourceFile).toBe(fixturePath);
      expect(cls.sourceRange.end).toBeGreaterThan(cls.sourceRange.start);
    }

    const source = readFileSync(fixturePath, "utf8");
    expect(source.slice(world.sourceRange.start, world.sourceRange.end)).toContain(
      "end MediumWorld;",
    );
    expect(source.slice(flash.sourceRange.start, flash.sourceRange.end)).toContain(
      "end FlashUnit;",
    );
  });

  it("does not require the source filename to be package.mo", () => {
    expect(loadModelicaFile(fixturePath).qualifiedName).toBe("IEH_CPP");
  });

  it("resolves a safe fallback for an inherited Modelica package icon", () => {
    const source = readFileSync(fixturePath, "utf8");
    const parsed = loadModelicaFile(fixturePath);
    const classNode = {
      kind: "package" as const,
      name: parsed.name,
      qualifiedName: parsed.qualifiedName,
      sourceFile: fixturePath,
      sourceRange: parsed.sourceRange!,
      isPartial: false,
      isEncapsulated: false,
      extendsClauses: ["Modelica.Icons.Package"],
      children: [],
    };
    const resolved = resolveIconForClass(
      classNode,
      [classNode],
      source,
      "IEH_CPP",
    );
    expect(resolved.icon?.graphics.length).toBeGreaterThan(0);
    expect(resolved.warnings).toContain(
      "Base icon not resolved: Modelica.Icons.Package",
    );
  });

  it("resolves relative extends names inside the same package", () => {
    const source = `package P
      model Base
        annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})}));
      end Base;
      model Child
        extends Base;
      end Child;
    end P;`;
    const parsed = parseModelicaFile(source, "relative.mo");
    const child = findClassByQualifiedName(parsed.classes, "P.Child")!;
    const resolved = resolveIconForClass(
      child,
      parsed.classes,
      source,
      "Child",
    );
    expect(resolved.icon?.graphics[0]?.type).toBe("Rectangle");
    expect(resolved.warnings).toEqual([]);
  });

  it("keeps ownership metadata when resolving inherited graphics", () => {
    const source = `package P
      model Base
        annotation(Icon(graphics={Ellipse(extent={{-10,-10},{10,10}})}));
      end Base;
      model Child
        extends Base;
        annotation(Icon(graphics={Text(extent={{-20,-4},{20,4}}, textString="Child")}));
      end Child;
    end P;`;
    const parsed = parseModelicaFile(source, "ownership.mo");
    const child = findClassByQualifiedName(parsed.classes, "P.Child")!;
    const resolved = resolveIconForClass(child, parsed.classes, source, "Child");
    expect(resolved.icon?.graphics).toHaveLength(2);
    expect(resolved.icon?.graphics[0]).toMatchObject({
      ownerQualifiedName: "P.Base",
      inherited: true,
      inheritancePath: ["P.Child", "P.Base"],
    });
    expect(resolved.icon?.graphics[1]).toMatchObject({
      ownerQualifiedName: "P.Child",
      inherited: false,
      inheritancePath: ["P.Child"],
    });
  });
});
