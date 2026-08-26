import { describe, expect, it } from "vitest";
import { formatModelicaExtent } from "./IconViewer";

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
});
