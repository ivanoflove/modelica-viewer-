within ;
package Showcase
  "演示用 Modelica 库 — 覆盖 M1 全部识别目标：within / package / model / block / connector / record / function / partial / encapsulated"
  annotation(Documentation(info="<html><p>Showcase library for M1 Package Browser demo.</p><!-- package Showcase end Showcase; 故意放在字符串里测试 Lexer --></html>"));

  // 单文件内嵌 package 测试：A -> B -> C
  package A
    package B
      model C "最深层 model"
        parameter Real x=1;
      end C;
      // annotation 里的 package 关键字必须被忽略
      annotation(Documentation(info="<html> package FAKE end FAKE; </html>"));
    end B;
  end A;

  package InnerSingle
    "单文件内嵌：InnerSingle 包含 2 个 model"
    model Resistor "Ideal resistor"
      parameter Real R=100;
    end Resistor;
    partial model Base "partial 必须识别"
    end Base;
    encapsulated block MyBlock "encapsulated block"
    end MyBlock;
  end InnerSingle;

  connector Pin "connector 测试"
    Real v;
    flow Real i;
  end Pin;

  record MyRecord "record 测试"
    Real a;
    Real b;
  end MyRecord;

  function MyFunc "function 测试"
    input Real x;
    output Real y;
  algorithm
    y := x*2;
  end MyFunc;

  model WithEquation
    "包含 end if; 不能误判为 package 结束"
    Real x;
  equation
    if x > 0 then
      x = 1;
    end if;
  end WithEquation;

end Showcase;
