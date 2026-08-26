import { describe, expect, it } from "vitest";
import { buildDeleteEdit, formatModelicaExtent } from "./IconViewer";

describe("IconViewer source edit formatting", () => {
  it("keeps both point braces and the outer extent braces", () => {
    expect(
      formatModelicaExtent({
        p1: { x: -76, y: 14 },
        p2: { x: -66, y: 4 },
      }),
    ).toBe("{{-76,14},{-66,4}}");
  });

  it("rounds fractional coordinates without dropping the closing brace", () => {
    expect(
      formatModelicaExtent({
        p1: { x: -63.2064749, y: 5.4709832 },
        p2: { x: -53.2064749, y: -4.5290168 },
      }),
    ).toBe("{{-63.206475,5.470983},{-53.206475,-4.529017}}");
  });

  it("deletes the following comma without damaging the next graphic", () => {
    const source = "graphics={Rectangle(...), Line(...), Ellipse(...)}";
    const editable = {
      id: "line",
      graphic: {} as never,
      selected: false,
      transform: { translate: { x: 0, y: 0 }, scale: { x: 1, y: 1 }, rotate: 0 },
      source: {
        itemRange: {
          start: source.indexOf("Line"),
          end: source.indexOf("Line") + "Line(...)".length,
        },
      },
    };
    const edit = buildDeleteEdit(editable, source);
    expect(edit?.replacement).toBe("");
    expect(source.slice(0, edit!.start) + edit!.replacement + source.slice(edit!.end)).toBe(
      "graphics={Rectangle(...), Ellipse(...)}",
    );
  });
});
