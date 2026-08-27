import { useMemo, useRef } from "react";
import type { IconDto, GraphicItemDto } from "../../../shared/modelicaGraphics";
import type { ComponentInstanceDto, DiagramSceneDto } from "../../../shared/modelica";
import { computePlacementTransform } from "../../../shared/diagram";
import { ResolvedGraphicRenderer } from "./GraphicItem";
import { GraphicViewport } from "./GraphicViewport";

function viewportIcon(scene: DiagramSceneDto): IconDto {
  return {
    coordinateSystem: scene.coordinateSystem,
    graphics: scene.components.flatMap((component) => {
      const transformation = component.placement?.transformation;
      if (!component.placement?.visible || !transformation) return [];
      const graphic: GraphicItemDto = {
        type: "Rectangle",
        extent: transformation.extent,
        fillPattern: "FillPattern.None",
      };
      return [graphic];
    }),
  };
}

function ComponentRenderer({
  component,
  index,
}: {
  component: ComponentInstanceDto;
  index: number;
}) {
  const placement = component.placement;
  const transformation = placement?.transformation;
  if (!placement?.visible || !transformation) return null;
  const icon = component.resolvedIcon;
  const labelY = Math.min(transformation.extent.p1.y, transformation.extent.p2.y) - 7;
  return (
    <g className="diagram-component" data-component-id={component.id}>
      {icon && component.classKind !== "connector" ? (
        <g transform={computePlacementTransform(icon.coordinateSystem, transformation)}>
          {icon.graphics.map((graphic, graphicIndex) => (
            <ResolvedGraphicRenderer
              key={`${component.id}:${graphicIndex}`}
              item={graphic}
              styleId={`diagram-style-${index}-${graphicIndex}`}
            />
          ))}
        </g>
      ) : (
        <g transform={`translate(${transformation.origin.x},${transformation.origin.y}) rotate(${transformation.rotation})`} className="diagram-placeholder">
          <rect x={-10} y={-10} width={20} height={20} />
          <text x={0} y={5} textAnchor="middle">?</text>
        </g>
      )}
      <g transform="scale(1,-1)" className="diagram-component-label">
        <text x={transformation.origin.x} y={-labelY} textAnchor="middle">{component.name}</text>
      </g>
    </g>
  );
}

export function DiagramViewer({ scene }: { scene: DiagramSceneDto }) {
  const svgRef = useRef<SVGSVGElement>(null);
  const viewportIcon = useMemo(() => viewportIconForScene(scene), [scene]);
  const visibleComponents = scene.components.filter(
    (component) => component.placement?.visible !== false && component.placement?.transformation,
  );
  return (
    <div className="diagram-viewer">
      {scene.diagnostics.length > 0 && (
        <div className="diagram-warning" title={scene.diagnostics.join("\n")}>
          {scene.diagnostics.length} 个 Diagram 提示
        </div>
      )}
      {visibleComponents.length === 0 ? (
        <div className="diagram-empty">No placed components</div>
      ) : (
        <GraphicViewport
          icon={viewportIcon}
          resetKey={`diagram:${scene.components.map((component) => component.id).join("|")}`}
          svgRef={svgRef}
          onPointerMove={() => undefined}
          onPointerUp={() => undefined}
          onPointerCancel={() => undefined}
          canvasLabel="Diagram 画布"
        >
          <g transform="scale(1,-1)" className="diagram-layer">
            {scene.components.map((component, index) => (
              <ComponentRenderer
                key={component.id}
                component={component}
                index={index}
              />
            ))}
          </g>
        </GraphicViewport>
      )}
    </div>
  );
}

function viewportIconForScene(scene: DiagramSceneDto): IconDto {
  return viewportIcon(scene);
}
