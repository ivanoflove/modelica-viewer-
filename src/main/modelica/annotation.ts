import { tokenize, type Token } from "./lexer.js";

export type AnnotationValue =
  | { type: "number"; value: number }
  | { type: "string"; value: string }
  | { type: "boolean"; value: boolean }
  | { type: "identifier"; name: string }
  | { type: "array"; items: AnnotationValue[] }
  | { type: "call"; call: AnnotationCall };

export interface AnnotationCall {
  name: string;
  arguments: AnnotationArgument[];
}

export interface AnnotationArgument {
  name?: string;
  value: AnnotationValue;
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

  // Entry: parse any value
  parseValue(): AnnotationValue {
    const tok = this.peek();
    if (tok.type === "NUMBER") {
      this.advance();
      return { type: "number", value: Number(tok.value) };
    }
    if (tok.type === "STRING") {
      this.advance();
      return { type: "string", value: tok.value };
    }
    if (tok.type === "LBRACE") {
      return this.parseArray();
    }
    // identifier or keyword that could be boolean or call or bare identifier
    if (tok.type === "IDENT" || tok.type === "KEYWORD") {
      // check if next is LPAREN => call
      const next = this.tokens[this.pos + 1];
      if (next && next.type === "LPAREN") {
        return { type: "call", call: this.parseCall() };
      }
      // boolean literals
      if (tok.value === "true" || tok.value === "false") {
        this.advance();
        return { type: "boolean", value: tok.value === "true" };
      }
      // bare identifier (e.g., FillPattern.Solid => we handle DOT later)
      this.advance();
      let name = tok.value;
      // handle dotted qualified names like FillPattern.Solid
      while (this.peek().type === "DOT") {
        this.advance(); // dot
        const id = this.peek();
        if (id.type === "IDENT" || id.type === "KEYWORD") {
          this.advance();
          name += "." + id.value;
        } else break;
      }
      return { type: "identifier", name };
    }
    throw new Error(
      `Unexpected token ${tok.type} ${tok.value} at ${tok.line}:${tok.col}`,
    );
  }

  parseArray(): AnnotationValue {
    this.expect("LBRACE");
    const items: AnnotationValue[] = [];
    // handle empty array {}
    if (this.peek().type === "RBRACE") {
      this.advance();
      return { type: "array", items };
    }
    while (!this.isAtEnd() && this.peek().type !== "RBRACE") {
      // Modelica allows array of arrays: {{-80,-50},{80,50}} and array of calls: { Rectangle(...), Ellipse(...) }
      // Also points: {{-80,0},{0,50}} etc.
      // Parse value
      const v = this.parseValue();
      items.push(v);
      if (this.peek().type === "COMMA") {
        this.advance();
      } else {
        // allow missing comma? break if RBRACE
        if (this.peek().type === "RBRACE") break;
      }
    }
    this.expect("RBRACE");
    return { type: "array", items };
  }

  parseCall(): AnnotationCall {
    const nameTok = this.advance(); // IDENT or KEYWORD
    const name = nameTok.value;
    this.expect("LPAREN");
    const args: AnnotationArgument[] = [];
    if (this.peek().type === "RPAREN") {
      this.advance();
      return { name, arguments: args };
    }
    while (!this.isAtEnd() && this.peek().type !== "RPAREN") {
      // check named arg: IDENT EQUALS value
      // Lookahead: IDENT/KEYWORD EQUALS
      const p0 = this.peek();
      const p1 = this.tokens[this.pos + 1];
      if (
        (p0.type === "IDENT" || p0.type === "KEYWORD") &&
        p1 &&
        p1.type === "EQUALS"
      ) {
        const argName = this.advance().value;
        this.advance(); // EQUALS
        const val = this.parseValue();
        args.push({ name: argName, value: val });
      } else {
        // positional arg (value)
        const val = this.parseValue();
        args.push({ value: val });
      }
      if (this.peek().type === "COMMA") {
        this.advance();
      } else if (this.peek().type === "RPAREN") break;
    }
    this.expect("RPAREN");
    return { name, arguments: args };
  }

  // Parse top-level: expect annotation call, but we can parse any call
  parseTopCall(): AnnotationCall {
    // skip leading annotation keyword if present: annotation( ... )
    return this.parseCall();
  }
}

export function parseAnnotationSlice(slice: string): AnnotationCall | null {
  try {
    const tokens = tokenize(slice);
    // filter out SEMICOLON? Modelica annotation slice ends with maybe no semicolon, but we have tokens with SEMICOLON, we can ignore trailing semicolon by not expecting it
    // Parser will stop at RPAREN, ignore trailing SEMICOLON/EOF
    const parser = new AnnotationParser(tokens);
    const call = parser.parseTopCall();
    return call;
  } catch {
    return null;
  }
}

// Helper to find Icon call inside annotation
export function findIconCall(
  annotationCall: AnnotationCall,
): AnnotationCall | null {
  if (annotationCall.name !== "annotation") return null;
  for (const arg of annotationCall.arguments) {
    if (arg.value.type === "call" && arg.value.call.name === "Icon") {
      return arg.value.call;
    }
    // annotation may have named args? Usually annotation(Icon(...), Documentation(...))
    // So positional args are calls
  }
  return null;
}

// Generic helpers for resolver
export function getArg(
  call: AnnotationCall,
  name: string,
): AnnotationValue | undefined {
  const found = call.arguments.find((a) => a.name === name);
  return found?.value;
}

export function getPositionalArg(
  call: AnnotationCall,
  index: number,
): AnnotationValue | undefined {
  const positional = call.arguments.filter((a) => !a.name);
  return positional[index]?.value;
}
