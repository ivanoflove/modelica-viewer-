package IEH_CPP "远宽综合能源库 (pure C++ thermopack-cxx backend)"
  extends Modelica.Icons.Package;

  package ThermoMedium
    package Types
      record State
        Real p;
        Real T;
      end State;
    end Types;

    package Functions
      function create
        input String eos;
        output Integer handle;
        external "C" handle = create_thermo_handle(eos)
          annotation(Library = "cxx_wrapper");
      end create;

      function free
        input Integer handle;
        external "C" free_thermo_handle(handle);
      end free;
    end Functions;

    model MediumWorld
      parameter Boolean flashType = true;
    equation
      if flashType then
        when sample(0, 1) then
        end when;
      else
        assert(true, "fallback");
      end if;
    end MediumWorld;

    package Units
      model FlashUnit
        annotation(Icon(graphics={
          Text(origin={0, -130}, extent={{-50, 50}, {50, -50}},
            textString="%name")
        }));
      end FlashUnit;

      model FlashUnitCont
        algorithm
          while false loop
          end while;
      end FlashUnitCont;
    end Units;
  end ThermoMedium;
end IEH_CPP;
