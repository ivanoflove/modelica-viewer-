import { useState } from "react";
import type { PackageNodeDto, ClassNodeDto } from "../../../shared/modelica";

type Selection =
  | { kind: "package"; node: PackageNodeDto }
  | { kind: "class"; node: ClassNodeDto };

interface Props {
  root: PackageNodeDto;
  selected: Selection | null;
  onSelect: (s: Selection) => void;
}

function ClassIcon({ kind }: { kind: string }) {
  const iconKind = kind.replace(/\s+/g, "-");
  return (
    <span className={`node-icon node-icon-${iconKind}`} title={kind} aria-label={kind}>
      <svg viewBox="0 0 16 16" aria-hidden="true">
        {kind === "package" && <><path d="M2 5.5h12v7H2z" /><path d="M2 5.5V3.5h4l1.2 2H14" /></>}
        {kind === "model" && <><rect x="2.5" y="2.5" width="11" height="11" rx="2" /><path d="M5 6h6M5 9h4" /></>}
        {kind === "block" && <><rect x="2.5" y="2.5" width="11" height="11" rx="1" /><path d="M5 5v6M8 5v6M11 5v6" /></>}
        {kind.indexOf("connector") >= 0 && <><circle cx="8" cy="8" r="4.5" /><path d="M1.5 8h2M12.5 8h2M8 1.5v2M8 12.5v2" /></>}
        {kind === "record" && <><path d="M3 2.5h7l3 3v8H3z" /><path d="M10 2.5v3h3M5 8h6M5 10.5h4" /></>}
        {kind === "function" && <><path d="M3 4h2l3 8 3-8h2" /><path d="M2 13.5h12" /></>}
        {kind === "type" && <><path d="M3 3h10M8 3v10" /><path d="M5 13h6" /></>}
        {!["package", "model", "block", "connector", "record", "function", "type"].includes(kind) && kind.indexOf("connector") < 0 && <><rect x="2.5" y="2.5" width="11" height="11" rx="3" /><path d="M5 8h6M8 5v6" /></>}
      </svg>
    </span>
  );
}

function ClassRow({
  cls,
  selected,
  onSelect,
}: {
  cls: ClassNodeDto;
  selected: Selection | null;
  onSelect: (s: Selection) => void;
}) {
  const isSelected =
    selected?.kind === "class" &&
    selected.node.qualifiedName === cls.qualifiedName;
  return (
    <div
      className={`tree-row ${isSelected ? "selected" : ""}`}
      onClick={() => onSelect({ kind: "class", node: cls })}
      role="button"
      tabIndex={0}
    >
      <span className="tree-indent" />
      <span className="tree-toggle placeholder" />
      <ClassIcon kind={cls.kind} />
      <span className="node-label">{cls.name}</span>
      <span className="node-kind">{cls.kind}</span>
    </div>
  );
}

function PackageRow({
  node,
  depth,
  selected,
  onSelect,
}: {
  node: PackageNodeDto;
  depth: number;
  selected: Selection | null;
  onSelect: (s: Selection) => void;
}) {
  const [expanded, setExpanded] = useState(depth < 2);
  const isSelected =
    selected?.kind === "package" &&
    selected.node.qualifiedName === node.qualifiedName;
  const hasChildren = node.children.length > 0 || node.classes.length > 0;

  return (
    <div className="tree-package">
      <div
        className={`tree-row ${isSelected ? "selected" : ""}`}
        onClick={() => onSelect({ kind: "package", node })}
        role="button"
        tabIndex={0}
      >
        <span className="tree-indent" style={{ width: depth * 12 }} />
        <span
          className={`tree-toggle ${hasChildren ? "" : "placeholder"} ${expanded ? "expanded" : ""}`}
          onClick={(e) => {
            e.stopPropagation();
            if (hasChildren) setExpanded((v) => !v);
          }}
        >
          {hasChildren ? "▸" : ""}
        </span>
        <ClassIcon kind="package" />
        <span className="node-label">{node.name}</span>
        <span className="node-kind">package</span>
      </div>

      {expanded && (
        <div className="tree-children">
          {node.children
            .slice()
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((ch) => (
              <PackageRow
                key={ch.qualifiedName}
                node={ch}
                depth={depth + 1}
                selected={selected}
                onSelect={onSelect}
              />
            ))}
          {node.classes
            .slice()
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((cls) => (
              <div
                key={cls.qualifiedName}
                style={{ marginLeft: (depth + 1) * 12 }}
              >
                <ClassRow cls={cls} selected={selected} onSelect={onSelect} />
              </div>
            ))}
        </div>
      )}
    </div>
  );
}

export function PackageTree({ root, selected, onSelect }: Props) {
  return (
    <div className="package-tree" role="tree">
      <PackageRow
        node={root}
        depth={0}
        selected={selected}
        onSelect={onSelect}
      />
    </div>
  );
}

export type { Selection };
