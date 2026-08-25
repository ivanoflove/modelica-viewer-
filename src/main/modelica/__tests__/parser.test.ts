import { describe, it, expect } from "vitest";
import { parseModelicaFile } from "../parser.js";

describe("parser", () => {
  it("should parse single-file inline nested package A -> B -> C", () => {
    const src = `package A package B model C end C; end B; end A;`;
    const file = parseModelicaFile(src, "test.mo");
    expect(file.within).toBeNull();
    expect(file.classes).toHaveLength(1);
    const A = file.classes[0]!;
    expect(A.name).toBe("A");
    expect(A.kind).toBe("package");
    expect(A.qualifiedName).toBe("A");
    expect(A.children).toHaveLength(1);
    const B = A.children[0]!;
    expect(B.name).toBe("B");
    expect(B.qualifiedName).toBe("A.B");
    expect(B.children[0]!.name).toBe("C");
    expect(B.children[0]!.qualifiedName).toBe("A.B.C");
  });

  it("should parse within and qualified names", () => {
    const src = `within Modelica.Electrical.Analog; package Basic model Resistor end Resistor; end Basic;`;
    const file = parseModelicaFile(src, "Basic/package.mo");
    expect(file.within).toBe("Modelica.Electrical.Analog");
    expect(file.classes[0]!.name).toBe("Basic");
    expect(file.classes[0]!.qualifiedName).toBe(
      "Modelica.Electrical.Analog.Basic",
    );
    expect(file.classes[0]!.children[0]!.qualifiedName).toBe(
      "Modelica.Electrical.Analog.Basic.Resistor",
    );
  });

  it("should not terminate package on end if / end for", () => {
    const src = `package Basic model Resistor equation if true then R=1; end if; end Resistor; end Basic;`;
    const file = parseModelicaFile(src, "test.mo");
    expect(file.classes[0]!.name).toBe("Basic");
    expect(file.classes[0]!.children).toHaveLength(1);
    expect(file.classes[0]!.children[0]!.name).toBe("Resistor");
  });

  it("should parse partial encapsulated modifiers", () => {
    const src = `partial package P end P; encapsulated model M end M; partial encapsulated block B end B;`;
    const file = parseModelicaFile(src, "test.mo");
    expect(file.classes[0]!.isPartial).toBe(true);
    expect(file.classes[0]!.isEncapsulated).toBe(false);
    expect(file.classes[1]!.isEncapsulated).toBe(true);
    expect(file.classes[2]!.isPartial).toBe(true);
    expect(file.classes[2]!.isEncapsulated).toBe(true);
  });

  it("should ignore annotation strings containing package", () => {
    const src = `package MyLibrary annotation(Documentation(info="<html> package ABC end ABC; </html>")); model Resistor end Resistor; end MyLibrary;`;
    const file = parseModelicaFile(src, "test.mo");
    expect(file.classes).toHaveLength(1);
    expect(file.classes[0]!.name).toBe("MyLibrary");
    expect(file.classes[0]!.children).toHaveLength(1);
    expect(file.classes[0]!.children[0]!.name).toBe("Resistor");
  });

  it("should handle within; (empty within) as null", () => {
    const src = `within; package MyLibrary end MyLibrary;`;
    const file = parseModelicaFile(src, "test.mo");
    expect(file.within).toBeNull();
  });

  it("should parse multiple top-level classes with connector/record/function", () => {
    const src = `within Test; connector Pin end Pin; record R end R; function F end F; block B end B;`;
    const file = parseModelicaFile(src, "test.mo");
    expect(file.within).toBe("Test");
    const kinds = file.classes.map((c) => c.kind);
    expect(kinds).toEqual(["connector", "record", "function", "block"]);
    expect(file.classes[0]!.qualifiedName).toBe("Test.Pin");
  });
});
