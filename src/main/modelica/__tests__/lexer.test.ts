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
});
