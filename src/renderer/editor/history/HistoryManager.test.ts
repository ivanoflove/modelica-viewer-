import { describe, expect, it } from "vitest";
import { HistoryManager } from "./HistoryManager.js";

const graphic = (x: number) => ({
  type: "Rectangle" as const,
  extent: { p1: { x, y: 0 }, p2: { x: x + 10, y: 10 } },
});

describe("HistoryManager", () => {
  it("keeps one command per committed edit and clears redo on new edits", () => {
    const history = new HistoryManager(100);
    history.push({ graphicId: "A", before: graphic(0), after: graphic(10) });
    history.push({ graphicId: "A", before: graphic(10), after: graphic(20) });
    expect(history.size).toBe(2);
    expect(history.peekUndo()?.after).toEqual(graphic(20));
    history.acceptUndo();
    expect(history.peekRedo()?.after).toEqual(graphic(20));
    history.push({ graphicId: "A", before: graphic(10), after: graphic(30) });
    expect(history.canRedo).toBe(false);
  });

  it("limits history to the configured maximum", () => {
    const history = new HistoryManager(2);
    for (let x = 0; x < 3; x++) history.push({ graphicId: "A", before: graphic(x), after: graphic(x + 1) });
    expect(history.size).toBe(2);
    expect(history.peekUndo()?.before).toEqual(graphic(2));
  });
});
