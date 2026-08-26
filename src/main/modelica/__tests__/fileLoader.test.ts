import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { loadModelicaFile } from "../loader.js";

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
});
