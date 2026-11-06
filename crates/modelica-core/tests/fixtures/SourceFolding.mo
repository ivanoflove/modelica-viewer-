model SourceFolding
  parameter Real gain = 1;
  Real values[2];
  String label = "string with fake ( { [ ) } ] delimiters";

  annotation(
    Placement(
      transformation(
        origin={10, 20},
        extent={{-10, -10}, {10, 10}},
        rotation=0)),
    Icon(
      graphics={
        Rectangle(extent={{-80, -40}, {80, 40}}),
        Text(
          extent={{-50, -10}, {50, 10}},
          textString="fold me"),
        Polygon(
          points={{-60, -20}, {0, 20}, {60, -20}, {-60, -20}})}),
    Diagram(
      graphics={
        Ellipse(extent={{-40, -20}, {40, 20}}),
        Bitmap(extent={{-20, -10}, {20, 10}})}));

  // fake(annotation(Icon(graphics={Rectangle()})))
  equation
    values[1] = gain;
    values[2] = values[1] + 1;
end SourceFolding;
