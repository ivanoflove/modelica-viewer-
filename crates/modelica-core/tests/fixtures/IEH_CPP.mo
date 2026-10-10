package IEH_CPP "远宽综合能源库 (pure C++ thermopack-cxx backend)"
  package ThermoMedium "Thermodynamic medium package — inner/outer Thermopack CPA/Cubic integration"
    package Types
      record State
      end State;
    end Types;
    package Functions
      function create
      end create;
    end Functions;
    model MediumWorld
    end MediumWorld;
    package Units
    end Units;
    package Examples
    end Examples;
  end ThermoMedium;
  package Interfaces
    package FluidInterfaces
      connector FluidPort
      end FluidPort;
      connector FluidPortIN
      end FluidPortIN;
      connector FluidPortOUT
      end FluidPortOUT;
    end FluidInterfaces;
  end Interfaces;
  package FluidUnits
  end FluidUnits;
  package Converter "能源转换设备库"
    model Mixer
    end Mixer;
    function convert
    end convert;
    record ConversionState
    end ConversionState;
  end Converter;
  package FMU "test"
  end FMU;
end IEH_CPP;
