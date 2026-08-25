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

  it("should set sourceRange and slice equals original class text", () => {
    const src = `package P\n  model A\n    Real x;\n  end A;\n  model B\n    Real y;\n  end B;\nend P;`;
    const file = parseModelicaFile(src, "test.mo");
    const P = file.classes[0]!;
    const A = P.children[0]!;
    const B = P.children[1]!;
    expect(src.slice(A.sourceRange.start, A.sourceRange.end)).toBe(
      "model A\n    Real x;\n  end A;",
    );
    expect(src.slice(B.sourceRange.start, B.sourceRange.end)).toBe(
      "model B\n    Real y;\n  end B;",
    );
    // P should cover from 'package P' to 'end P;'
    expect(src.slice(P.sourceRange.start, P.sourceRange.end)).toBe(src);
  });

  it("should include partial/encapsulated in sourceRange start", () => {
    const src = `partial model A end A; encapsulated package B end B; partial encapsulated block C end C;`;
    const file = parseModelicaFile(src, "test.mo");
    expect(
      src.slice(
        file.classes[0]!.sourceRange.start,
        file.classes[0]!.sourceRange.end,
      ),
    ).toBe("partial model A end A;");
    expect(
      src.slice(
        file.classes[1]!.sourceRange.start,
        file.classes[1]!.sourceRange.end,
      ),
    ).toBe("encapsulated package B end B;");
    expect(
      src.slice(
        file.classes[2]!.sourceRange.start,
        file.classes[2]!.sourceRange.end,
      ),
    ).toBe("partial encapsulated block C end C;");
  });

  it("should not include trailing content after end;", () => {
    const src = `package P model A end A; model B end B; end P;`;
    const file = parseModelicaFile(src, "test.mo");
    const A = file.classes[0]!.children[0]!;
    expect(src.slice(A.sourceRange.start, A.sourceRange.end)).toBe(
      "model A end A;",
    );
    expect(
      src.slice(A.sourceRange.start, A.sourceRange.end).includes("model B"),
    ).toBe(false);
  });
});
