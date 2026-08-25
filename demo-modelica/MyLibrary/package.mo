within ;
package MyLibrary
  "My demo Modelica library — single-file package with Icon annotations"

  annotation(Documentation(info="<html><p>Demo library for Modelica Explorer extension.</p></html>"));

  model Resistor
    "Ideal linear electrical resistor"
    parameter Modelica.SIunits.Resistance R = 1 "Resistance";
    Modelica.Electrical.Analog.Interfaces.PositivePin p;
    Modelica.Electrical.Analog.Interfaces.NegativePin n;
  equation
    R * p.i = p.v - n.v;
  annotation(
      Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={0,0,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
        Line(points={{-80,0},{80,0}}, color={0,0,255}),
        Line(points={{0,-80},{0,80}}, color={0,0,255}),
        Text(extent={{-40,-40},{40,40}}, textString="R")
      }));
  end Resistor;

  model Capacitor
    "Ideal linear electrical capacitor"
    parameter Modelica.SIunits.Capacitance C = 1 "Capacitance";
    Modelica.Electrical.Analog.Interfaces.PositivePin p;
    Modelica.Electrical.Analog.Interfaces.NegativePin n;
  equation
    C * der(p.v - n.v) = p.i;
  annotation(
      Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={0,0,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
        Line(points={{-80,0},{-20,0}}, color={0,0,255}),
        Line(points={{20,0},{80,0}}, color={0,0,255}),
        Line(points={{20,-60},{20,60}}, color={0,0,255}),
        Line(points={{-20,-60},{-20,60}}, color={0,0,255}),
        Text(extent={{-40,-40},{40,40}}, textString="C")
      }));
  end Capacitor;

  model Inductor
    "Ideal linear electrical inductor"
    parameter Modelica.SIunits.Inductance L = 1 "Inductance";
  equation
    L * der(p.i) = p.v - n.v;
  annotation(
      Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={0,0,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
        Line(points={{-80,0},{-60,0}}, color={0,0,255}),
        Line(points={{-50,-40},{-30,40}}, color={0,0,255}),
        Line(points={{-25,-40},{-5,40}}, color={0,0,255}),
        Line(points={{0,-40},{20,40}}, color={0,0,255}),
        Line(points={{25,-40},{45,40}}, color={0,0,255}),
        Line(points={{60,0},{80,0}}, color={0,0,255}),
        Text(extent={{-40,-40},{40,40}}, textString="L")
      }));
  end Inductor;

  model Ground
    "Electrical ground"
  annotation(
      Icon(graphics={
        Line(points={{-80,0},{80,0}}, color={0,0,255}),
        Line(points={{0,0},{0,40}}, color={0,0,255}),
        Line(points={{-40,-20},{40,-20}}, color={0,0,255}),
        Line(points={{-20,-40},{20,-40}}, color={0,0,255}),
        Text(extent={{-40,-60},{40,-80}}, textString="GND")
      }));
  end Ground;

  package Blocks
    "Input/output blocks"

    model Integrator
      "Continuous-time integrator"
      parameter Real k = 1 "Gain";
    annotation(
        Icon(graphics={
          Rectangle(extent={{-100,100},{100,-100}}, lineColor={0,0,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Line(points={{-80,0},{20,0}}, color={0,0,255}),
          Line(points={{20,0},{20,60}}, color={0,0,255}),
          Line(points={{20,0},{80,0}}, color={0,0,255}),
          Text(extent={{-60,-20},{60,-50}}, textString="∫")
        }));
    end Integrator;

    model Gain
      "Gain block: y = k * u"
      parameter Real k = 1 "Gain";
    annotation(
        Icon(graphics={
          Rectangle(extent={{-100,100},{100,-100}}, lineColor={0,0,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Line(points={{-80,0},{-20,0}}, color={0,0,255}),
          Line(points={{20,0},{80,0}}, color={0,0,255}),
          Polygon(points={{-20,-40},{30,0},{-20,40},{-20,-40}}, lineColor={0,0,255}, fillColor={0,0,255}, fillPattern=FillPattern.Solid),
          Text(extent={{-40,-40},{40,40}}, textString="k")
        }));
    end Gain;

    model Sum
      "Sum of two signals"
      parameter Integer n = 2 "Number of inputs";
    annotation(
        Icon(graphics={
          Rectangle(extent={{-100,100},{100,-100}}, lineColor={0,0,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Line(points={{-80,40},{-20,40}}, color={0,0,255}),
          Line(points={{-80,-40},{-20,-40}}, color={0,0,255}),
          Line(points={{20,0},{80,0}}, color={0,0,255}),
          Line(points={{-20,-50},{-20,50}}, color={0,0,255}),
          Line(points={{-50,-20},{10,20}}, color={0,0,255}),
          Text(extent={{-40,-40},{40,40}}, textString="Σ")
        }));
    end Sum;
  end Blocks;

  package Sensors
    "Sensor models"

    model VoltageSensor
      "Ideal voltage sensor"
    annotation(
        Icon(graphics={
          Ellipse(extent={{-100,100},{100,-100}}, lineColor={0,0,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Line(points={{-80,0},{80,0}}, color={0,0,255}),
          Text(extent={{-40,-40},{40,40}}, textString="V")
        }));
    end VoltageSensor;

    model CurrentSensor
      "Ideal current sensor"
    annotation(
        Icon(graphics={
          Ellipse(extent={{-100,100},{100,-100}}, lineColor={0,0,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Line(points={{0,-80},{0,80}}, color={0,0,255}),
          Line(points={{-30,-40},{30,-40}}, color={0,0,255}),
          Line(points={{-30,40},{30,40}}, color={0,0,255}),
          Text(extent={{-40,-40},{40,40}}, textString="A")
        }));
    end CurrentSensor;
  end Sensors;

  package Examples
    "Example circuits"
    model RCCircuit
      "Simple RC circuit"
      MyLibrary.Resistor R(R=100);
      MyLibrary.Capacitor C(C=0.001);
    equation
      connect(R.p, C.p);
      connect(R.n, C.n);
    annotation(
        Icon(graphics={
          Rectangle(extent={{-100,100},{100,-100}}, lineColor={0,0,200}, fillColor={230,230,250}, fillPattern=FillPattern.Solid),
          Text(extent={{-80,-60},{80,80}}, textString="RC")
        }));
    end RCCircuit;
  end Examples;

  annotation(Documentation(info="<html><p>Single-file demo Modelica library.</p></html>"));
end MyLibrary;
