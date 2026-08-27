import { describe, expect, it } from "vitest";
import { HistoryManager } from "./HistoryManager.js";

const command = (before: string, after: string) => ({
  type: "move" as const,
  target: {
    ownerQualifiedName: "Demo.Model",
    graphicPath: "Icon.graphics:0",
    property: "extent" as const,
  },
  before,
  after,
});

describe("HistoryManager", () => {
  it("keeps one command per committed edit and clears redo on new edits", () => {
    const history = new HistoryManager(100);
    history.push(command("{{0,0},{10,10}}", "{{10,0},{20,10}}"));
    history.push(command("{{10,0},{20,10}}", "{{20,0},{30,10}}"));
    expect(history.size).toBe(2);
    expect(history.peekUndo()?.after).toBe("{{20,0},{30,10}}");
    history.acceptUndo();
    expect(history.peekRedo()?.after).toBe("{{20,0},{30,10}}");
    history.push(command("{{10,0},{20,10}}", "{{30,0},{40,10}}"));
    expect(history.canRedo).toBe(false);
  });

  it("limits history to the configured maximum", () => {
    const history = new HistoryManager(2);
    for (let x = 0; x < 3; x++) {
      history.push(command(String(x), String(x + 1)));
    }
    expect(history.size).toBe(2);
    expect(history.peekUndo()?.before).toBe("2");
  });
});
