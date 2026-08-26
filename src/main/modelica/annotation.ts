import { tokenize, type Token } from "./lexer.js";

export type AnnotationValue =
  | { type: "number"; value: number; range: { start: number; end: number } }
  | { type: "string"; value: string; range: { start: number; end: number } }
  | { type: "boolean"; value: boolean; range: { start: number; end: number } }
  | { type: "identifier"; name: string; range: { start: number; end: number } }
  | {
      type: "qualifiedName";
      name: string;
      parts: string[];
      range: { start: number; end: number };
    }
  | {
      type: "array";
      items: AnnotationValue[];
      range: { start: number; end: number };
    }
  | {
      type: "call";
      call: AnnotationCall;
      range: { start: number; end: number };
    };

export interface AnnotationCall {
  name: string;
  arguments: AnnotationArgument[];
  sourceRange: { start: number; end: number };
  nameRange: { start: number; end: number };
}

export interface AnnotationArgument {
  name?: string;
  value: AnnotationValue;
  sourceRange: { start: number; end: number };
  nameRange?: { start: number; end: number };
}

class AnnotationParser {
  private tokens: Token[];
  private pos = 0;

  constructor(tokens: Token[]) {
    this.tokens = tokens;
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

  private expect(type: string, value?: string): Token {
    const t = this.peek();
    if (t.type !== type || (value !== undefined && t.value !== value)) {
      throw new Error(`Expected ${type} ${value ?? ""} at ${t.line}:${t.col}`);
    }
    return this.advance();
  }

  private isAtEnd(): boolean {
    return this.peek().type === "EOF";
  }

  parseValue(): AnnotationValue {
    const tok = this.peek();
    if (tok.type === "NUMBER") {
      this.advance();
      return {
        type: "number",
        value: Number(tok.value),
        range: { start: tok.start, end: tok.end },
      };
    }
    if (tok.type === "STRING") {
      this.advance();
      return {
        type: "string",
        value: tok.value,
        range: { start: tok.start, end: tok.end },
      };
    }
    if (tok.type === "LBRACE") {
      return this.parseArray();
    }
    if (tok.type === "IDENT" || tok.type === "KEYWORD") {
      const next = this.tokens[this.pos + 1];
      if (next && next.type === "LPAREN") {
        const call = this.parseCall();
        return { type: "call", call, range: call.sourceRange };
      }
      if (tok.value === "true" || tok.value === "false") {
        this.advance();
        return {
          type: "boolean",
          value: tok.value === "true",
          range: { start: tok.start, end: tok.end },
        };
      }
      this.advance();
      let name = tok.value;
      let end = tok.end;
      while (this.peek().type === "DOT") {
        this.advance();
        const id = this.peek();
        if (id.type === "IDENT" || id.type === "KEYWORD") {
          this.advance();
          name += "." + id.value;
          end = id.end;
        } else break;
      }
      const parts = name.split(".");
      if (parts.length > 1) {
        return {
          type: "qualifiedName",
          name,
          parts,
          range: { start: tok.start, end },
        };
      }
      return { type: "identifier", name, range: { start: tok.start, end } };
    }
    throw new Error(
      `Unexpected token ${tok.type} ${tok.value} at ${tok.line}:${tok.col}`,
    );
  }

  parseArray(): AnnotationValue {
    const lbrace = this.expect("LBRACE");
    const items: AnnotationValue[] = [];
    if (this.peek().type === "RBRACE") {
      const rbrace = this.advance();
      return {
        type: "array",
        items,
        range: { start: lbrace.start, end: rbrace.end },
      };
    }
    while (!this.isAtEnd() && this.peek().type !== "RBRACE") {
      const v = this.parseValue();
      items.push(v);
      if (this.peek().type === "COMMA") {
        this.advance();
      } else if (this.peek().type === "RBRACE") break;
    }
    const rbrace = this.expect("RBRACE");
    return {
      type: "array",
      items,
      range: { start: lbrace.start, end: rbrace.end },
    };
  }

  parseCall(): AnnotationCall {
    const nameTok = this.advance();
    const name = nameTok.value;
    const nameRange = { start: nameTok.start, end: nameTok.end };
    this.expect("LPAREN");
    const args: AnnotationArgument[] = [];
    if (this.peek().type === "RPAREN") {
      const rparen = this.advance();
      return {
        name,
        arguments: args,
        sourceRange: { start: nameTok.start, end: rparen.end },
        nameRange,
      };
    }
    while (!this.isAtEnd() && this.peek().type !== "RPAREN") {
      const p0 = this.peek();
      const p1 = this.tokens[this.pos + 1];
      if (
        (p0.type === "IDENT" || p0.type === "KEYWORD") &&
        p1 &&
        p1.type === "EQUALS"
      ) {
        const nameTok2 = this.advance();
        this.advance();
        const val = this.parseValue();
        const argRange = { start: nameTok2.start, end: (val as any).range.end };
        args.push({
          name: nameTok2.value,
          value: val,
          sourceRange: argRange,
          nameRange: { start: nameTok2.start, end: nameTok2.end },
        });
      } else {
        const val = this.parseValue();
        const r = (val as any).range as { start: number; end: number };
        args.push({ value: val, sourceRange: r });
      }
      if (this.peek().type === "COMMA") {
        this.advance();
      } else if (this.peek().type === "RPAREN") break;
    }
    const rparen = this.expect("RPAREN");
    return {
      name,
      arguments: args,
      sourceRange: { start: nameTok.start, end: rparen.end },
      nameRange,
    };
  }

  parseTopCall(): AnnotationCall {
    return this.parseCall();
  }
}

export function parseAnnotationSlice(slice: string): AnnotationCall | null {
  try {
    const tokens = tokenize(slice);
    const parser = new AnnotationParser(tokens);
    const call = parser.parseTopCall();
    return call;
  } catch {
    return null;
  }
}

export function findIconCall(
  annotationCall: AnnotationCall,
): AnnotationCall | null {
  if (annotationCall.name !== "annotation") return null;
  for (const arg of annotationCall.arguments) {
    if (arg.value.type === "call" && arg.value.call.name === "Icon") {
      return arg.value.call;
    }
  }
  return null;
}

export function getArg(
  call: AnnotationCall,
  name: string,
): AnnotationValue | undefined {
  const found = call.arguments.find((a) => a.name === name);
  return found?.value;
}

export function getArgWithRange(
  call: AnnotationCall,
  name: string,
): AnnotationArgument | undefined {
  return call.arguments.find((a) => a.name === name);
}

export function getPositionalArg(
  call: AnnotationCall,
  index: number,
): AnnotationValue | undefined {
  const positional = call.arguments.filter((a) => !a.name);
  return positional[index]?.value;
}
