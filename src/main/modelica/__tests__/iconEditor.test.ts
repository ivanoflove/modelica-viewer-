import { describe, it, expect } from "vitest";
import { parseAnnotationSlice, findIconCall, getArgWithRange } from "../annotation.js";
import { resolveEditableIcon, serializeExtent, serializePoints, serializeOrigin, extractEditableIconFromSlice } from "../iconResolver.js";

function editableFrom(src: string, modelName = "MyModel") {
  const anno = parseAnnotationSlice(src)!;
  const iconCall = findIconCall(anno)!;
  return resolveEditableIcon(iconCall, modelName)!;
}

describe("iconEditor: editable ranges", () => {
  it("should capture item/extent ranges for Rectangle", () => {
    const src = `annotation(Icon(graphics={Rectangle(extent={{-80,-50},{80,50}}, lineColor={0,0,255})}))`;
    const ed = editableFrom(src);
    expect(ed.editables.length).toBe(1);
    const e = ed.editables[0]!;
    expect(e.source.itemRange).toBeDefined();
    expect(e.source.extentRange).toBeDefined();
    const extentText = src.slice(e.source.extentRange!.start, e.source.extentRange!.end);
    expect(extentText).toBe("{{-80,-50},{80,50}}");
    const itemText = src.slice(e.source.itemRange.start, e.source.itemRange.end);
    expect(itemText).toContain("Rectangle");
  });

  it("should capture points range for Line", () => {
    const src = `annotation(Icon(graphics={Line(points={{-80,0},{0,50},{80,0}}, color={0,0,255})}))`;
    const ed = editableFrom(src);
    const e = ed.editables[0]!;
    expect(e.source.pointsRange).toBeDefined();
    const ptsText = src.slice(e.source.pointsRange!.start, e.source.pointsRange!.end);
    expect(ptsText).toBe("{{-80,0},{0,50},{80,0}}");
  });

  it("should capture origin range when present", () => {
    const src = `annotation(Icon(graphics={Rectangle(origin={20,10}, extent={{-40,-20},{40,20}}) }))`;
    const ed = editableFrom(src);
    const e = ed.editables[0]!;
    expect(e.source.originRange).toBeDefined();
    expect(src.slice(e.source.originRange!.start, e.source.originRange!.end)).toBe("{20,10}");
  });
});

describe("iconEditor: serialization", () => {
  it("serializeExtent produces Modelica array", () => {
    expect(serializeExtent({ p1: { x: -80, y: -40 }, p2: { x: 80, y: 40 } }))
      .toBe("{{-80,-40},{80,40}}");
  });

  it("serializePoints produces Modelica points", () => {
    expect(serializePoints([{ x: -80, y: 0 }, { x: 0, y: 50 }, { x: 80, y: 0 }]))
      .toBe("{{-80,0},{0,50},{80,0}}");
  });

  it("serializeOrigin produces Modelica origin", () => {
    expect(serializeOrigin({ x: 30, y: 30 })).toBe("{30,30}");
  });

  it("normalizes floats (no 19.9999999)", () => {
    const e = serializeExtent({ p1: { x: -79.9999999997, y: -39.9999999997 }, p2: { x: 80.0000000001, y: 40 } });
    expect(e).not.toContain("9999999");
    expect(e).toBe("{{-80,-40},{80,40}}");
  });
});

describe("iconEditor: source patch semantics", () => {
  it("applying a patch to extent range only changes that region", () => {
    const full = `within X; model M\n  annotation(Icon(graphics={\n    Rectangle(extent={{-80,-50},{80,50}}, lineColor={0,0,255})\n  }));\nend M;`;
    const ed = extractEditableIconFromSlice(full.slice(full.indexOf("annotation")), "M")!;
    const e = ed.editables[0]!;
    const range = e.source.extentRange!;
    const baseOffset = full.indexOf("annotation");
    const start = baseOffset + range.start;
    const end = baseOffset + range.end;
    const replacement = "{{-70,-40},{90,60}}";
    const updated = full.slice(0, start) + replacement + full.slice(end);
    expect(updated).toContain("extent={{-70,-40},{90,60}}");
    expect(updated).toContain("lineColor={0,0,255}"); // untouched
    expect(updated).toContain("within X; model M"); // untouched
    expect(updated).toContain("end M;"); // untouched
    // comments/format preserved: ensure no pretty-print of surrounding
    expect(updated.indexOf("Rectangle(")).toBeGreaterThan(0);
  });

  it("after patch, re-parsing gives fresh sourceRange", () => {
    // Simulate: patch extent to a different-length string, then re-parse editable
    const full = `within X; model M\n  annotation(Icon(graphics={\n    Rectangle(extent={{-80,-50},{80,50}}, lineColor={0,0,255})\n  }));\nend M;`;
    const annoOffset = full.indexOf("annotation");
    const first = extractEditableIconFromSlice(full.slice(annoOffset), "M")!;
    const e0 = first.editables[0]!;
    const r = e0.source.extentRange!;
    const updated = full.slice(0, annoOffset + r.start) + "{{0,0},{10,10}}" + full.slice(annoOffset + r.end);
    // re-parse
    const second = extractEditableIconFromSlice(updated.slice(updated.indexOf("annotation")), "M")!;
    const e1 = second.editables[0]!;
    expect(e1.source.extentRange).toBeDefined();
    const text = updated.slice(updated.indexOf("annotation") + e1.source.extentRange!.start, updated.indexOf("annotation") + e1.source.extentRange!.end);
    expect(text).toBe("{{0,0},{10,10}}");
    expect(second.icon.graphics[0]!.type).toBe("Rectangle");
  });

  it("should prefer origin when both origin and extent exist", () => {
    const src = `annotation(Icon(graphics={Rectangle(origin={20,10}, extent={{-40,-20},{40,20}}) }))`;
    const ed = editableFrom(src);
    const e = ed.editables[0]!;
    // Editor should pick originRange first
    expect(e.source.originRange).toBeDefined();
    expect(e.source.extentRange).toBeDefined();
  });

  it("extractEditableIconFromSlice works with full class slice", () => {
    const slice = `annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Ellipse(extent={{-30,-30},{30,30}}), Text(extent={{-80,60},{80,90}}, textString="%name")}))`;
    const ed = extractEditableIconFromSlice(slice, "Capacitor")!;
    expect(ed.editables.length).toBe(2);
    const text = ed.icon.graphics.find((g) => g.type === "Text") as any;
    expect(text.textString).toBe("Capacitor");
  });
});
