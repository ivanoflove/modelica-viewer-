export type TokenType =
  | "IDENT"
  | "DOT"
  | "SEMICOLON"
  | "STRING"
  | "KEYWORD"
  | "NUMBER"
  | "LPAREN"
  | "RPAREN"
  | "LBRACE"
  | "RBRACE"
  | "COMMA"
  | "EQUALS"
  | "LBRACKET"
  | "RBRACKET"
  | "OPERATOR"
  | "WHITESPACE"
  | "COMMENT"
  | "EOF";

export interface Token {
  type: TokenType;
  value: string;
  line: number;
  col: number;
  start: number;
  end: number;
}

const KEYWORDS = new Set([
  "algorithm",
  "and",
  "annotation",
  "within",
  "package",
  "model",
  "block",
  "connector",
  "record",
  "function",
  "class",
  "type",
  "break",
  "connect",
  "constant",
  "constrainedby",
  "der",
  "discrete",
  "each",
  "else",
  "elseif",
  "elsewhen",
  "enumeration",
  "false",
  "final",
  "flow",
  "initial",
  "impure",
  "in",
  "inner",
  "input",
  "not",
  "operator",
  "or",
  "outer",
  "output",
  "parameter",
  "pure",
  "redeclare",
  "replaceable",
  "return",
  "stream",
  "then",
  "true",
  "partial",
  "end",
  "encapsulated",
  "extends",
  "import",
  "if",
  "for",
  "when",
  "while",
  "loop",
  "equation",
  "algorithm",
  "protected",
  "public",
  "external",
  "expandable",
]);

export interface TokenizeOptions {
  /** Keep whitespace/comments and otherwise skipped characters for highlighting. */
  preserveTrivia?: boolean;
}

export function tokenize(input: string, options: TokenizeOptions = {}): Token[] {
  const tokens: Token[] = [];
  const preserveTrivia = options.preserveTrivia === true;
  let i = 0;
  let line = 1;
  let col = 1;

  const advance = (n = 1) => {
    for (let k = 0; k < n; k++) {
      if (input[i] === "\n") {
        line++;
        col = 1;
      } else {
        col++;
      }
      i++;
    }
  };

  const peek = (offset = 0) => input[i + offset] ?? "";

  while (i < input.length) {
    const ch = input[i];

    // whitespace
    if (ch === " " || ch === "\t" || ch === "\r" || ch === "\n") {
      const start = i;
      const startLine = line;
      const startCol = col;
      advance();
      while (i < input.length && " \t\r\n".includes(input[i]!)) advance();
      if (preserveTrivia) {
        tokens.push({
          type: "WHITESPACE",
          value: input.slice(start, i),
          line: startLine,
          col: startCol,
          start,
          end: i,
        });
      }
      continue;
    }

    // line comment //
    if (ch === "/" && peek(1) === "/") {
      const start = i;
      const startLine = line;
      const startCol = col;
      advance(2);
      while (i < input.length && input[i] !== "\n") advance();
      if (preserveTrivia) {
        tokens.push({
          type: "COMMENT",
          value: input.slice(start, i),
          line: startLine,
          col: startCol,
          start,
          end: i,
        });
      }
      continue;
    }

    // block comment /* */
    if (ch === "/" && peek(1) === "*") {
      const start = i;
      const startLine = line;
      const startCol = col;
      advance(2);
      while (i < input.length) {
        if (input[i] === "*" && peek(1) === "/") {
          advance(2);
          break;
        }
        advance();
      }
      if (preserveTrivia) {
        tokens.push({
          type: "COMMENT",
          value: input.slice(start, i),
          line: startLine,
          col: startCol,
          start,
          end: i,
        });
      }
      continue;
    }

    // string literal " ... "  Modelica uses "" as escaped quote inside string
    if (ch === '"') {
      const start = i;
      const startLine = line;
      const startCol = col;
      advance(); // opening "
      let str = "";
      while (i < input.length) {
        if (input[i] === '"') {
          if (peek(1) === '"') {
            str += '"';
            advance(2);
            continue;
          }
          advance(); // closing "
          break;
        }
        str += input[i];
        advance();
      }
      tokens.push({
        type: "STRING",
        value: str,
        line: startLine,
        col: startCol,
        start,
        end: i,
      });
      continue;
    }

    // single-char punctuation for annotation
    // A dot followed by a digit starts a decimal such as .5. A standalone
    // dot remains the qualified-name punctuation token.
    if (ch === "." && !/[0-9]/.test(peek(1))) {
      const start = i;
      const startLine = line;
      const startCol = col;
      advance();
      tokens.push({
        type: "DOT",
        value: ".",
        line: startLine,
        col: startCol,
        start,
        end: i,
      });
      continue;
    }
    if (ch === ";") {
      const start = i;
      const startLine = line;
      const startCol = col;
      advance();
      tokens.push({
        type: "SEMICOLON",
        value: ";",
        line: startLine,
        col: startCol,
        start,
        end: i,
      });
      continue;
    }
    if (ch === "(") {
      const start = i;
      const sL = line;
      const sC = col;
      advance();
      tokens.push({
        type: "LPAREN",
        value: "(",
        line: sL,
        col: sC,
        start,
        end: i,
      });
      continue;
    }
    if (ch === ")") {
      const start = i;
      const sL = line;
      const sC = col;
      advance();
      tokens.push({
        type: "RPAREN",
        value: ")",
        line: sL,
        col: sC,
        start,
        end: i,
      });
      continue;
    }
    if (ch === "{") {
      const start = i;
      const sL = line;
      const sC = col;
      advance();
      tokens.push({
        type: "LBRACE",
        value: "{",
        line: sL,
        col: sC,
        start,
        end: i,
      });
      continue;
    }
    if (ch === "}") {
      const start = i;
      const sL = line;
      const sC = col;
      advance();
      tokens.push({
        type: "RBRACE",
        value: "}",
        line: sL,
        col: sC,
        start,
        end: i,
      });
      continue;
    }
    if (ch === ",") {
      const start = i;
      const sL = line;
      const sC = col;
      advance();
      tokens.push({
        type: "COMMA",
        value: ",",
        line: sL,
        col: sC,
        start,
        end: i,
      });
      continue;
    }
    if (ch === "=") {
      const start = i;
      const sL = line;
      const sC = col;
      advance();
      tokens.push({
        type: "EQUALS",
        value: "=",
        line: sL,
        col: sC,
        start,
        end: i,
      });
      continue;
    }
    if (ch === "[") {
      const start = i;
      const sL = line;
      const sC = col;
      advance();
      tokens.push({ type: "LBRACKET", value: "[", line: sL, col: sC, start, end: i });
      continue;
    }
    if (ch === "]") {
      const start = i;
      const sL = line;
      const sC = col;
      advance();
      tokens.push({ type: "RBRACKET", value: "]", line: sL, col: sC, start, end: i });
      continue;
    }

    // number: optional sign, digits, optional dot, optional exponent
    if (
      /[0-9]/.test(ch) ||
      (ch === "-" && /[0-9.]/.test(peek(1))) ||
      (ch === "." && /[0-9]/.test(peek(1)))
    ) {
      const start = i;
      const sL = line;
      const sC = col;
      let num = "";
      if (ch === "-") {
        num += "-";
        advance();
      }
      // integer part
      while (i < input.length && /[0-9]/.test(input[i])) {
        num += input[i];
        advance();
      }
      // dot part
      if (input[i] === "." && /[0-9]/.test(peek(1))) {
        num += ".";
        advance();
        while (i < input.length && /[0-9]/.test(input[i])) {
          num += input[i];
          advance();
        }
      } else if (input[i] === "." && num.length > 0 && !/[0-9]/.test(peek(1))) {
        // trailing dot not part of number, leave it for DOT token
      }
      // exponent
      if (
        (input[i] === "e" || input[i] === "E") &&
        (/[0-9]/.test(peek(1)) ||
          ((peek(1) === "+" || peek(1) === "-") && /[0-9]/.test(peek(2))))
      ) {
        num += input[i];
        advance();
        if (input[i] === "+" || input[i] === "-") {
          num += input[i];
          advance();
        }
        while (i < input.length && /[0-9]/.test(input[i])) {
          num += input[i];
          advance();
        }
      }
      // ensure we consumed at least one digit
      if (num === "-" || num === "" || num === ".") {
        // not a valid number, treat as skip and continue
        continue;
      }
      tokens.push({
        type: "NUMBER",
        value: num,
        line: sL,
        col: sC,
        start,
        end: i,
      });
      continue;
    }

    // identifier / keyword
    if (/[A-Za-z_]/.test(ch)) {
      const start = i;
      const startLine = line;
      const startCol = col;
      let word = "";
      while (i < input.length && /[A-Za-z0-9_]/.test(input[i])) {
        word += input[i];
        advance();
      }
      if (KEYWORDS.has(word)) {
        tokens.push({
          type: "KEYWORD",
          value: word,
          line: startLine,
          col: startCol,
          start,
          end: i,
        });
      } else {
        tokens.push({
          type: "IDENT",
          value: word,
          line: startLine,
          col: startCol,
          start,
          end: i,
        });
      }
      continue;
    }

    // Keep operators in highlighting mode. The parser intentionally only
    // needs the punctuation above, but the source viewer must preserve the
    // exact source coverage.
    const start = i;
    const startLine = line;
    const startCol = col;
    advance();
    if (preserveTrivia) {
      tokens.push({
        type: "OPERATOR",
        value: input.slice(start, i),
        line: startLine,
        col: startCol,
        start,
        end: i,
      });
    }
  }

  tokens.push({ type: "EOF", value: "", line, col, start: i, end: i });
  return tokens;
}

/** Lexer mode for the read-only Modelica source viewer. */
export function tokenizeForHighlighting(input: string): Token[] {
  return tokenize(input, { preserveTrivia: true });
}
