model TextBase
  annotation(Icon(
    coordinateSystem(extent={{-100,-100},{100,100}}),
    graphics={
      Text(extent={{-90,70},{90,90}}, textString="inherited", textStyle={TextStyle.Italic})
    }));
end TextBase;

model TextComponent
  parameter String label="default";
  extends TextBase;
  annotation(Icon(
    coordinateSystem(extent={{-100,-100},{100,100}}),
    graphics={
      Rectangle(extent={{-80,-50},{80,50}}, fillPattern=FillPattern.Solid,
        fillColor={230,235,245}),
      Text(extent={{-75,20},{75,40}}, textString="%label", fontSize=0,
        horizontalAlignment=TextAlignment.Center,
        textStyle={TextStyle.Bold,TextStyle.Italic}),
      Text(extent={{-75,-5},{75,15}}, textString="%{label}",
        fontName="Courier New", textStyle={TextStyle.UnderLine})
    }));
end TextComponent;

model TextParity
  TextComponent instance(label="Instance override")
    annotation(Placement(transformation(extent={{5,-4},{-5,4}}, rotation=30)));
  annotation(Icon(
    coordinateSystem(extent={{-100,-100},{100,100}}),
    graphics={
      Text(extent={{-95,75},{95,95}}, textString="ordinary text"),
      Text(extent={{-95,52},{95,72}}, textString="中文文本", fontSize=12),
      Text(extent={{-95,29},{95,49}}, textString="%name", horizontalAlignment=TextAlignment.Left),
      Text(extent={{-95,6},{95,26}}, textString="%class", horizontalAlignment=TextAlignment.Center),
      Text(extent={{-95,-17},{95,3}}, textString="%label", horizontalAlignment=TextAlignment.Right),
      Text(extent={{-95,-40},{95,-20}}, textString="%{label}"),
      Text(extent={{-95,-63},{95,-43}}, textString="%% literal percent"),
      Text(extent={{-95,-86},{-5,-66}}, textString="Bold", textStyle={TextStyle.Bold}),
      Text(extent={{5,-86},{95,-66}}, textString="Italic", textStyle={TextStyle.Italic}),
      Text(extent={{-95,-109},{-5,-89}}, textString="UnderLine", textStyle={TextStyle.UnderLine}),
      Text(extent={{5,-109},{95,-89}}, textString="Bold Italic",
        textStyle={TextStyle.Bold,TextStyle.Italic}),
      Text(extent={{-90,100},{-20,120}}, textString="Left", horizontalAlignment=TextAlignment.Left),
      Text(extent={{-35,100},{35,120}}, textString="Center", horizontalAlignment=TextAlignment.Center),
      Text(extent={{20,100},{90,120}}, textString="Right", horizontalAlignment=TextAlignment.Right),
      Text(extent={{-90,125},{-20,145}}, textString="Explicit", fontSize=14),
      Text(extent={{20,125},{90,145}}, textString="Auto size", fontSize=0),
      Text(extent={{-90,150},{-20,170}}, textString="Rotate 90", rotation=90),
      Text(extent={{20,150},{90,170}}, textString="Mirror safe", rotation=30)
    }),
    Diagram(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Text(extent={{-90,80},{90,100}}, textString="TextParity Diagram")
    }));
end TextParity;
