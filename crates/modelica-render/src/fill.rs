//! Semantic fill styles shared by render backends.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HatchPattern {
    Horizontal,
    Vertical,
    Cross,
    Forward,
    Backward,
    CrossDiag,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FillStyle {
    None,
    Solid,
    Hatch(HatchPattern),
    HorizontalCylinder,
    VerticalCylinder,
    Sphere,
}

impl FillStyle {
    /// Resolve a Modelica qualified FillPattern name. Missing or unknown
    /// values follow the existing viewer behavior and fall back to Solid.
    pub fn parse(pattern: Option<&str>) -> Self {
        match pattern {
            Some("FillPattern.None") => Self::None,
            Some("FillPattern.Horizontal") => Self::Hatch(HatchPattern::Horizontal),
            Some("FillPattern.Vertical") => Self::Hatch(HatchPattern::Vertical),
            Some("FillPattern.Cross") => Self::Hatch(HatchPattern::Cross),
            Some("FillPattern.Forward") => Self::Hatch(HatchPattern::Forward),
            Some("FillPattern.Backward") => Self::Hatch(HatchPattern::Backward),
            Some("FillPattern.CrossDiag") => Self::Hatch(HatchPattern::CrossDiag),
            Some("FillPattern.HorizontalCylinder") => Self::HorizontalCylinder,
            Some("FillPattern.VerticalCylinder") => Self::VerticalCylinder,
            Some("FillPattern.Sphere") => Self::Sphere,
            Some("FillPattern.Solid") | None | Some(_) => Self::Solid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FillStyle, HatchPattern};

    #[test]
    fn separates_hatches_from_gradient_fills() {
        assert_eq!(
            FillStyle::parse(Some("FillPattern.Horizontal")),
            FillStyle::Hatch(HatchPattern::Horizontal)
        );
        assert_eq!(
            FillStyle::parse(Some("FillPattern.CrossDiag")),
            FillStyle::Hatch(HatchPattern::CrossDiag)
        );
        assert_eq!(
            FillStyle::parse(Some("FillPattern.HorizontalCylinder")),
            FillStyle::HorizontalCylinder
        );
        assert_eq!(
            FillStyle::parse(Some("FillPattern.VerticalCylinder")),
            FillStyle::VerticalCylinder
        );
        assert_eq!(
            FillStyle::parse(Some("FillPattern.Sphere")),
            FillStyle::Sphere
        );
    }

    #[test]
    fn missing_and_unknown_patterns_are_solid() {
        assert_eq!(FillStyle::parse(None), FillStyle::Solid);
        assert_eq!(
            FillStyle::parse(Some("FillPattern.NotYetSupported")),
            FillStyle::Solid
        );
        assert_eq!(FillStyle::parse(Some("FillPattern.None")), FillStyle::None);
    }
}
