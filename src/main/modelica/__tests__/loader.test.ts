import { describe, it, expect } from "vitest";
import { PackageLoader } from "../loader.js";
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { randomUUID } from "node:crypto";
import path from "node:path";

function mkTmp(): string {
  const dir = join(
    tmpdir(),
    `modelica-loader-test-${randomUUID().slice(0, 8)}`,
  );
  mkdirSync(dir, { recursive: true });
  return dir;
}

describe("PackageLoader", () => {
  it("should load single-file inline package (demo MyLibrary)", () => {
    const loader = new PackageLoader();
    const root = loader.load(path.resolve("demo-modelica/MyLibrary"));
    expect(root.name).toBe("MyLibrary");
    expect(root.qualifiedName).toBe("MyLibrary");
    expect(root.classes.some((c) => c.name === "Resistor")).toBe(true);
    const blocks = root.children.find((c) => c.name === "Blocks");
    expect(blocks).toBeDefined();
    expect(blocks!.qualifiedName).toBe("MyLibrary.Blocks");
    expect(blocks!.classes.some((c) => c.name === "Integrator")).toBe(true);
    expect(root.classes.find((c) => c.name === "Resistor")!.qualifiedName).toBe(
      "MyLibrary.Resistor",
    );
  });

  it("should build fully-qualified names for directory-style layout", () => {
    const tmp = mkTmp();
    try {
      writeFileSync(join(tmp, "package.mo"), "package Modelica end Modelica;");
      mkdirSync(join(tmp, "Electrical"), { recursive: true });
      writeFileSync(
        join(tmp, "Electrical", "package.mo"),
        "within Modelica;\npackage Electrical end Electrical;",
      );
      mkdirSync(join(tmp, "Electrical", "Analog", "Basic"), {
        recursive: true,
      });
      writeFileSync(
        join(tmp, "Electrical", "Analog", "package.mo"),
        "within Modelica.Electrical;\npackage Analog end Analog;",
      );
      writeFileSync(
        join(tmp, "Electrical", "Analog", "Basic", "package.mo"),
        "within Modelica.Electrical.Analog;\npackage Basic\n  model Resistor end Resistor;\nend Basic;",
      );
      writeFileSync(
        join(tmp, "Electrical", "Analog", "Basic", "Resistor2.mo"),
        "within Modelica.Electrical.Analog.Basic;\nmodel Resistor2 end Resistor2;",
      );

      const loader = new PackageLoader();
      const root = loader.load(tmp);
      expect(root.qualifiedName).toBe("Modelica");
      const electrical = root.children.find((c) => c.name === "Electrical");
      expect(electrical!.qualifiedName).toBe("Modelica.Electrical");
      const analog = electrical!.children.find((c) => c.name === "Analog");
      expect(analog!.qualifiedName).toBe("Modelica.Electrical.Analog");
      const basic = analog!.children.find((c) => c.name === "Basic");
      expect(basic!.qualifiedName).toBe("Modelica.Electrical.Analog.Basic");
      expect(
        basic!.classes.some(
          (c) =>
            c.qualifiedName === "Modelica.Electrical.Analog.Basic.Resistor",
        ),
      ).toBe(true);
      expect(
        basic!.classes.some(
          (c) =>
            c.qualifiedName === "Modelica.Electrical.Analog.Basic.Resistor2",
        ),
      ).toBe(true);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("should handle *.mo files alongside package.mo and within mismatch fallback", () => {
    const tmp = mkTmp();
    try {
      writeFileSync(join(tmp, "package.mo"), "package MyLib end MyLib;");
      writeFileSync(
        join(tmp, "Extra.mo"),
        "within MyLib;\nmodel Extra end Extra;",
      );
      // also test directory without package.mo but with .mo files
      mkdirSync(join(tmp, "NoPkg"), { recursive: true });
      writeFileSync(join(tmp, "NoPkg", "Foo.mo"), "model Foo end Foo;");

      const loader = new PackageLoader();
      const root = loader.load(tmp);
      expect(root.classes.some((c) => c.qualifiedName === "MyLib.Extra")).toBe(
        true,
      );
      const noPkg = root.children.find((c) => c.name === "NoPkg");
      expect(noPkg).toBeDefined();
      expect(noPkg!.qualifiedName).toBe("MyLib.NoPkg");
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});
