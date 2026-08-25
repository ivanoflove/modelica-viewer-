export type TokenType =
  | "IDENT"
  | "DOT"
  | "SEMICOLON"
  | "STRING"
  | "KEYWORD"
  | "EOF";

export interface Token {
  type: TokenType;
  value: string;
  line: number;
  col: number;
}

const KEYWORDS = new Set([
  "within",
  "package",
  "model",
  "block",
  "connector",
  "record",
  "function",
  "class",
  "type",
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
  "annotation",
]);

export function tokenize(input: string): Token[] {
  const tokens: Token[] = [];
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
      advance();
      continue;
    }

    // line comment //
    if (ch === "/" && peek(1) === "/") {
      advance(2);
      while (i < input.length && input[i] !== "\n") advance();
      continue;
    }

    // block comment /* */
    if (ch === "/" && peek(1) === "*") {
      advance(2);
      while (i < input.length) {
        if (input[i] === "*" && peek(1) === "/") {
          advance(2);
          break;
        }
        advance();
      }
      continue;
    }

    // string literal " ... "  Modelica uses "" as escaped quote inside string
    if (ch === '"') {
      const startLine = line;
      const startCol = col;
      advance(); // opening "
      let str = "";
      while (i < input.length) {
        if (input[i] === '"') {
          if (peek(1) === '"') {
            // escaped quote
            str += '"';
            advance(2);
            continue;
          }
          advance(); // closing "
          break;
        }
        if (input[i] === "\n") {
          str += input[i];
        } else {
          str += input[i];
        }
        advance();
      }
      tokens.push({
        type: "STRING",
        value: str,
        line: startLine,
        col: startCol,
      });
      continue;
    }

    // dot and semicolon
    if (ch === ".") {
      tokens.push({ type: "DOT", value: ".", line, col });
      advance();
      continue;
    }
    if (ch === ";") {
      tokens.push({ type: "SEMICOLON", value: ";", line, col });
      advance();
      continue;
    }

    // identifier / keyword
    if (/[A-Za-z_]/.test(ch)) {
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
        });
      } else {
        tokens.push({
          type: "IDENT",
          value: word,
          line: startLine,
          col: startCol,
        });
      }
      continue;
    }

    // any other char: skip (numbers, operators, brackets, etc.)
    advance();
  }

  tokens.push({ type: "EOF", value: "", line, col });
  return tokens;
}
