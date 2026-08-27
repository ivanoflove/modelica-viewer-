import { useMemo } from "react";
import { tokenizeForHighlighting, type Token } from "../../../main/modelica/lexer";

const BUILTIN_TYPES = new Set(["Boolean", "Integer", "Real", "String"]);
const ANNOTATION_CALLS = new Set([
  "Icon",
  "Diagram",
  "Placement",
  "transformation",
  "iconTransformation",
  "Line",
  "Polygon",
  "Rectangle",
  "Ellipse",
  "Text",
  "Bitmap",
]);

function previousCodeToken(tokens: Token[], index: number): Token | undefined {
  for (let i = index - 1; i >= 0; i--) {
    if (tokens[i]!.type !== "WHITESPACE" && tokens[i]!.type !== "COMMENT") {
      return tokens[i];
    }
  }
  return undefined;
}

function nextCodeToken(tokens: Token[], index: number): Token | undefined {
  for (let i = index + 1; i < tokens.length; i++) {
    if (tokens[i]!.type !== "WHITESPACE" && tokens[i]!.type !== "COMMENT") {
      return tokens[i];
    }
  }
  return undefined;
}

/** Return a stable CSS token class without changing the source text. */
export function sourceTokenClass(tokens: Token[], index: number): string {
  const token = tokens[index]!;
  if (token.type === "WHITESPACE") return "source-token-whitespace";
  if (token.type === "COMMENT") return "source-token-comment";
  if (token.type === "NUMBER") return "source-token-number";
  if (token.type === "STRING") return "source-token-string";
  if (token.type === "OPERATOR" || token.type === "EQUALS") return "source-token-operator";
  if (["DOT", "SEMICOLON", "COMMA", "LPAREN", "RPAREN", "LBRACE", "RBRACE", "LBRACKET", "RBRACKET"].includes(token.type)) {
    return "source-token-punctuation";
  }
  if (token.type === "KEYWORD") {
    return token.value === "annotation" ? "source-token-annotation" : "source-token-keyword";
  }
  if (token.type !== "IDENT") return "source-token-identifier";

  const next = nextCodeToken(tokens, index);
  const previous = previousCodeToken(tokens, index);
  if (BUILTIN_TYPES.has(token.value)) return "source-token-builtin";
  if (ANNOTATION_CALLS.has(token.value) && next?.type === "LPAREN") {
    return "source-token-annotation";
  }
  if (/^[A-Z]/.test(token.value) || previous?.type === "DOT") {
    return "source-token-type";
  }
  return "source-token-identifier";
}

export function SourceViewer({ source }: { source: string }): JSX.Element {
  const tokens = useMemo(() => tokenizeForHighlighting(source), [source]);
  return (
    <pre className="modelica-source" aria-label="Modelica source">
      <code>
        {tokens
          .filter((token) => token.type !== "EOF")
          .map((token, index) => (
            <span
              className={sourceTokenClass(tokens, index)}
              key={`${token.start}:${token.end}`}
            >
              {source.slice(token.start, token.end)}
            </span>
          ))}
      </code>
    </pre>
  );
}
