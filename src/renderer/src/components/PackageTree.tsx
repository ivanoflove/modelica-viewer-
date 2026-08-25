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
  const map: Record<string, string> = {
    package: "📦",
    model: "📘",
    block: "🧱",
    connector: "🔌",
    record: "🗃️",
    function: "ƒ",
    class: "📄",
    type: "🏷️",
  };
  return <span className="node-icon">{map[kind] ?? "📄"}</span>;
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
        <span className="node-icon">📦</span>
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
