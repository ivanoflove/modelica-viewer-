import { useMemo, useRef } from "react";
import type { IconDto, GraphicItemDto } from "../../../shared/modelicaGraphics";
import type { ComponentInstanceDto, DiagramSceneDto } from "../../../shared/modelica";
import { resolveModelicaTextString } from "../../../shared/modelicaText";
import { computePlacementTransform, placementScale } from "../../../shared/diagram";
import { ResolvedGraphicRenderer } from "./GraphicItem";
import { GraphicViewport } from "./GraphicViewport";

function viewportIcon(scene: DiagramSceneDto): IconDto {
  const content = scene.contentBounds;
  if (content) {
    return {
      coordinateSystem: scene.coordinateSystem,
      graphics: [{
        type: "Rectangle",
        extent: {
          p1: { x: content.x, y: content.y },
          p2: { x: content.x + content.width, y: content.y + content.height },
        },
        fillPattern: "FillPattern.None",
      }],
    };
  }
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

function connectionPoints(line: NonNullable<DiagramSceneDto["connections"][number]["line"]>): string {
  const origin = line.origin ?? { x: 0, y: 0 };
  return line.points
    .map((point) => `${point.x + origin.x},${point.y + origin.y}`)
    .join(" ");
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
  const textReflection = icon ? placementScale(icon.coordinateSystem, transformation) : null;
  const renderGraphic = (graphic: GraphicItemDto, graphicIndex: number) => {
    const item = graphic.type === "Text" && icon
      ? {
          ...graphic,
          textString: resolveModelicaTextString(graphic.textTemplate ?? graphic.textString, {
            classQualifiedName: component.typeName,
            className: component.typeName.split(".").pop(),
            instanceName: component.name,
            parameterBindings: component.parameterBindings,
            parameterDefaults: icon.parameterDefaults,
          }),
        }
      : graphic;
    if (item.type !== "Text" || !textReflection) {
      return <ResolvedGraphicRenderer
        key={`${component.id}:${graphicIndex}`}
        item={item}
        styleId={`diagram-style-${index}-${graphicIndex}`}
      />;
    }
    // Keep the text box at the mirrored position, but cancel the reflection
    // around that box so SVG does not draw the glyph backwards/upside down.
    const origin = item.origin ?? { x: 0, y: 0 };
    const centerX = (item.extent.p1.x + item.extent.p2.x) / 2 + origin.x;
    const centerY = (item.extent.p1.y + item.extent.p2.y) / 2 + origin.y;
    const reflectX = textReflection.scaleX < 0 ? -1 : 1;
    const reflectY = textReflection.scaleY < 0 ? -1 : 1;
    return (
      <g
        key={`${component.id}:${graphicIndex}`}
        transform={`translate(${centerX},${centerY}) scale(${reflectX},${reflectY}) translate(${-centerX},${-centerY})`}
      >
        <ResolvedGraphicRenderer
          item={item}
          styleId={`diagram-style-${index}-${graphicIndex}`}
        />
      </g>
    );
  };
  return (
    <g className="diagram-component" data-component-id={component.id}>
      {icon && component.classKind !== "connector" ? (
        <g transform={computePlacementTransform(icon.coordinateSystem, transformation)}>
          {icon.graphics.map(renderGraphic)}
        </g>
      ) : (
        <g transform={`translate(${transformation.origin.x},${transformation.origin.y}) rotate(${transformation.rotation})`} className="diagram-placeholder">
          <rect x={-10} y={-10} width={20} height={20} />
          <text x={0} y={5} textAnchor="middle">?</text>
        </g>
      )}
    </g>
  );
}

export function DiagramViewer({ scene }: { scene: DiagramSceneDto }) {
  const svgRef = useRef<SVGSVGElement>(null);
  const viewportIcon = useMemo(() => viewportIconForScene(scene), [scene]);
  return (
    <div className="diagram-viewer">
      {scene.diagnostics.length > 0 && (
        <div className="diagram-warning" title={scene.diagnostics.join("\n")}>
          {scene.diagnostics.length} 个 Diagram 提示
        </div>
      )}
      <GraphicViewport
        icon={viewportIcon}
        resetKey={`diagram:${scene.classQualifiedName ?? "selected"}`}
        svgRef={svgRef}
        onPointerMove={() => undefined}
        onPointerUp={() => undefined}
        onPointerCancel={() => undefined}
        canvasLabel="Diagram 画布"
        initialFitMode="coordinateSystem"
        showFitModes={false}
      >
        <g transform="scale(1,-1)" className="diagram-layer">
          {scene.backgroundGraphics.map((graphic, graphicIndex) => (
            <ResolvedGraphicRenderer
              key={`diagram-background:${graphicIndex}`}
              item={graphic}
              styleId={`diagram-background-style-${graphicIndex}`}
            />
          ))}
          {scene.connections.map((connection, connectionIndex) => (
            connection.line ? (
              <g key={connection.id} className="diagram-connection">
                <ResolvedGraphicRenderer
                  item={connection.line}
                  styleId={`diagram-connection-style-${connectionIndex}`}
                />
                <polyline
                  className="connection-hit-target"
                  points={connectionPoints(connection.line)}
                  stroke="transparent"
                  strokeWidth={10}
                  fill="none"
                  vectorEffect="non-scaling-stroke"
                  pointerEvents="stroke"
                />
              </g>
            ) : null
          ))}
          {scene.components.map((component, index) => (
            <ComponentRenderer
              key={component.id}
              component={component}
              index={index}
            />
          ))}
        </g>
      </GraphicViewport>
    </div>
  );
}

function viewportIconForScene(scene: DiagramSceneDto): IconDto {
  return viewportIcon(scene);
}
