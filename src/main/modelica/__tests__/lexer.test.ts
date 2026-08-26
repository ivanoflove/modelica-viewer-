import { describe, it, expect } from "vitest";
import { tokenize } from "../lexer.js";

describe("lexer", () => {
  it("should skip line and block comments", () => {
    const toks = tokenize(
      `// line comment\npackage A end A; /* block */ model B end B;`,
    );
    const vals = toks.filter((t) => t.type === "KEYWORD").map((t) => t.value);
    expect(vals).toEqual(["package", "end", "model", "end"]);
  });

  it("should not tokenize keywords inside string literals", () => {
    const src = `annotation(Documentation(info="<html> package ABC end ABC; </html>")); package RealPkg end RealPkg;`;
    const toks = tokenize(src);
    const pkgKeywords = toks.filter(
      (t) => t.type === "KEYWORD" && t.value === "package",
    );
    expect(pkgKeywords).toHaveLength(1);
    expect(
      toks.some((t) => t.type === "STRING" && t.value.includes("package ABC")),
    ).toBe(true);
  });

  it('should handle Modelica escaped quotes "" inside string', () => {
    const src = `"a "" quoted "" string" package P end P;`;
    // In Modelica, "" inside string is escaped quote, not end. Our lexer should produce one STRING token
    const toks = tokenize(src);
    const strings = toks.filter((t) => t.type === "STRING");
    expect(strings).toHaveLength(1);
    expect(strings[0]!.value).toBe('a " quoted " string');
  });

  it("should tokenize DOT and SEMICOLON", () => {
    const toks = tokenize(`within Modelica.Electrical.Analog;`);
    expect(toks.map((t) => t.type)).toContain("DOT");
    expect(toks.map((t) => t.type)).toContain("SEMICOLON");
    const idents = toks.filter((t) => t.type === "IDENT").map((t) => t.value);
    expect(idents).toEqual(["Modelica", "Electrical", "Analog"]);
  });

  it("should distinguish keywords from idents", () => {
    const toks = tokenize(
      `package MyPkg model MyModel end MyModel; end MyPkg;`,
    );
    const keywords = toks
      .filter((t) => t.type === "KEYWORD")
      .map((t) => t.value);
    expect(keywords).toEqual(["package", "model", "end", "end"]);
    const idents = toks.filter((t) => t.type === "IDENT").map((t) => t.value);
    expect(idents).toEqual(["MyPkg", "MyModel", "MyModel", "MyPkg"]);
  });

  it("should record start/end offsets slicing back to source", () => {
    const src = `package A end A;`;
    const toks = tokenize(src);
    for (const t of toks) {
      if (t.type === "EOF") {
        expect(t.start).toBe(src.length);
        expect(t.end).toBe(src.length);
      } else {
        expect(src.slice(t.start, t.end)).toBe(
          t.value === ";" || t.value === "." ? t.value : t.value,
        );
      }
    }
    const pkg = toks.find((t) => t.value === "package")!;
    expect(src.slice(pkg.start, pkg.end)).toBe("package");
    // string token should cover quotes
    const src2 = `"hello ""world""" package P end P;`;
    const toks2 = tokenize(src2);
    const str = toks2.find((t) => t.type === "STRING")!;
    expect(str.start).toBe(0);
    expect(src2.slice(str.start, str.end)).toBe(`"hello ""world"""`);
  });

  it("should tokenize signed scientific notation", () => {
    const numbers = tokenize("{1.77636e-15,-8.88178E-16,2e+3,.5}")
      .filter((token) => token.type === "NUMBER")
      .map((token) => Number(token.value));
    expect(numbers).toEqual([1.77636e-15, -8.88178e-16, 2000, 0.5]);
  });
});
