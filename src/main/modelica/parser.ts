import { tokenize, type Token } from "./lexer.js";
import type { ClassKind, ClassNode, ModelicaFile } from "./types.js";

const CLASS_KINDS = new Set<string>([
  "package",
  "model",
  "block",
  "connector",
  "record",
  "function",
  "class",
  "type",
]);

class Parser {
  private tokens: Token[];
  private pos = 0;
  private sourceFile: string;

  constructor(tokens: Token[], sourceFile: string) {
    this.tokens = tokens;
    this.sourceFile = sourceFile;
  }

  private peek(offset = 0): Token {
    return (
      this.tokens[this.pos + offset] ?? this.tokens[this.tokens.length - 1]!
    );
  }

  private advance(): Token {
    const t = this.tokens[this.pos]!;
    if (this.pos < this.tokens.length - 1) this.pos++;
    return t;
  }

  private expectIdent(): string {
    const t = this.peek();
    if (t.type === "IDENT") {
      this.advance();
      return t.value;
    }
    // If we hit keyword that is not a class kind, treat as missing
    throw new Error(
      `Expected identifier at ${t.line}:${t.col}, got ${t.type} ${t.value}`,
    );
  }

  private parseQualifiedName(): string {
    const parts: string[] = [];
    parts.push(this.expectIdent());
    while (this.peek().type === "DOT") {
      this.advance(); // dot
      // dot may be followed by IDENT; if not, break
      if (this.peek().type === "IDENT") {
        parts.push(this.expectIdent());
      } else {
        break;
      }
    }
    return parts.join(".");
  }

  private isClassStartAt(pos: number): {
    isStart: boolean;
    modifiers: { partial: boolean; encapsulated: boolean };
    kind: string | null;
    name: string | null;
  } {
    let p = pos;
    let partial = false;
    let encapsulated = false;
    const t0 = this.tokens[p];
    if (!t0)
      return {
        isStart: false,
        modifiers: { partial, encapsulated },
        kind: null,
        name: null,
      };
    if (t0.value === "partial") {
      partial = true;
      p++;
    }
    const t1 = this.tokens[p];
    if (t1?.value === "encapsulated") {
      encapsulated = true;
      p++;
      // encapsulated may be followed by partial
      if (this.tokens[p]?.value === "partial") {
        partial = true;
        p++;
      }
    } else if (t1?.value === "partial" && !partial) {
      partial = true;
      p++;
      if (this.tokens[p]?.value === "encapsulated") {
        encapsulated = true;
        p++;
      }
    }

    const kindTok = this.tokens[p];
    if (!kindTok || !CLASS_KINDS.has(kindTok.value)) {
      return {
        isStart: false,
        modifiers: { partial, encapsulated },
        kind: null,
        name: null,
      };
    }
    const nameTok = this.tokens[p + 1];
    if (!nameTok || nameTok.type !== "IDENT") {
      return {
        isStart: false,
        modifiers: { partial, encapsulated },
        kind: null,
        name: null,
      };
    }
    return {
      isStart: true,
      modifiers: { partial, encapsulated },
      kind: kindTok.value,
      name: nameTok.value,
    };
  }

  private parseClass(parentQualified: string | null): ClassNode {
    const start = this.isClassStartAt(this.pos);
    if (!start.isStart || !start.kind || !start.name) {
      throw new Error(
        `parseClass called without class start at ${this.peek().line}:${this.peek().col}`,
      );
    }
    const isPartial = start.modifiers.partial;
    const isEncapsulated = start.modifiers.encapsulated;
    const sourceStart = this.tokens[this.pos]!.start;

    // consume modifiers
    if (
      this.peek().value === "partial" ||
      this.peek().value === "encapsulated"
    ) {
      // consume up to 2 modifiers
      if (this.peek().value === "partial") this.advance();
      if (this.peek().value === "encapsulated") this.advance();
      if (this.peek().value === "partial") this.advance();
      if (this.peek().value === "encapsulated") this.advance();
    }

    const kind = this.advance().value as ClassKind;
    const name = this.expectIdent();

    // qualified name for this class: if parentQualified provided, join; else within handling done outside
    // parser itself doesn't know within; loader will fix up if within exists
    // Here we use parentQualified if given, else name alone
    const qualifiedName = parentQualified ? `${parentQualified}.${name}` : name;

    const node: ClassNode = {
      kind,
      name,
      qualifiedName,
      sourceFile: this.sourceFile,
      sourceRange: { start: sourceStart, end: sourceStart },
      isPartial,
      isEncapsulated,
      children: [],
    };

    // Short type definitions use `type Name = BaseType(...);` and do not
    // have an `end Name;` terminator. Track annotation/array delimiters so
    // semicolons inside nested expressions do not end the declaration.
    if (kind === "type") {
      let parenDepth = 0;
      let braceDepth = 0;
      while (true) {
        const tok = this.peek();
        if (tok.type === "EOF") break;
        if (tok.type === "LPAREN") parenDepth++;
        if (tok.type === "RPAREN") parenDepth = Math.max(0, parenDepth - 1);
        if (tok.type === "LBRACE") braceDepth++;
        if (tok.type === "RBRACE") braceDepth = Math.max(0, braceDepth - 1);
        const endTok = this.advance();
        if (
          endTok.type === "SEMICOLON" &&
          parenDepth === 0 &&
          braceDepth === 0
        ) {
          node.sourceRange.end = endTok.end;
          break;
        }
      }
      return node;
    }

    // scan body until matching end <name>;
    while (true) {
      const tok = this.peek();
      if (tok.type === "EOF") {
        // unclosed class — return as-is, loader may record error
        break;
      }

      // check nested class start
      const nested = this.isClassStartAt(this.pos);
      if (nested.isStart) {
        const child = this.parseClass(qualifiedName);
        node.children.push(child);
        continue;
      }

      // check end
      if (tok.value === "end") {
        // peek ahead: end <IDENT> [;]
        const next = this.tokens[this.pos + 1];
        if (next && next.type === "IDENT") {
          if (next.value === name) {
            this.advance(); // end
            const nameToken = this.advance(); // name
            let endOffset = nameToken.end;
            if (this.peek().type === "SEMICOLON") {
              const semi = this.advance();
              endOffset = semi.end;
            }
            node.sourceRange.end = endOffset;
            break;
          }
          // end <other>; e.g., end if; end for; — not our terminator
          // consume end <ident> ; and continue
          // But only if it's one of the compound ends: if/for/when/while
          // For any other end <ident>, just advance over end and keep scanning
          // To avoid infinite loop, consume at least 'end'
          this.advance(); // end
          // peek again
          if (this.peek().type === "IDENT") {
            this.advance();
            if (this.peek().type === "SEMICOLON") this.advance();
          }
          continue;
        }
        // bare end without ident
        this.advance();
        continue;
      }

      this.advance();
    }

    return node;
  }

  parseFile(): ModelicaFile {
    let within: string | null = null;

    if (this.peek().value === "within") {
      this.advance();
      // within may be empty: "within;" (top-level) or "within Modelica.Foo;"
      if (this.peek().type === "SEMICOLON") {
        this.advance();
        within = null;
      } else if (this.peek().type === "IDENT") {
        within = this.parseQualifiedName();
        if (this.peek().type === "SEMICOLON") this.advance();
      } else if (this.peek().type === "SEMICOLON") {
        this.advance();
      }
    }

    const classes: ClassNode[] = [];

    // For qualified names inside file: top-level parent is within
    while (this.peek().type !== "EOF") {
      const start = this.isClassStartAt(this.pos);
      if (start.isStart) {
        const parentForTopLevel = within;
        const c = this.parseClass(parentForTopLevel);
        // loader will later recompute qualifiedName if within is null and parent package exists
        classes.push(c);
      } else {
        this.advance();
      }
    }

    return { within, classes };
  }
}

export function parseModelicaFile(
  content: string,
  sourceFile: string,
): ModelicaFile {
  const tokens = tokenize(content);
  const parser = new Parser(tokens, sourceFile);
  return parser.parseFile();
}

// helper to flatten qualifiedName recomputation for loader
export function requalifyClassTree(
  node: ClassNode,
  parentQualified: string,
  sourceFile: string,
): ClassNode {
  const q = parentQualified ? `${parentQualified}.${node.name}` : node.name;
  return {
    ...node,
    qualifiedName: q,
    sourceFile,
    children: node.children.map((c) => requalifyClassTree(c, q, sourceFile)),
  };
}
