//! Instrument identity (`*IDN?`) and the family it places an instrument in.
//!
//! The family decides which device model may talk to an instrument. That
//! check has to come first: on these firmwares a query the instrument does
//! not know is not an error reply, it is silence, so sending one model's
//! commands to another model's instrument is how a session hangs.

use std::fmt;
use std::str::FromStr;

use serde::Serialize;

/// `*IDN?` split into its four documented fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Identity {
    pub vendor: String,
    pub model: String,
    pub serial: String,
    pub firmware: String,
}

impl Identity {
    pub fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.split(',').map(str::trim);
        Some(Self {
            vendor: parts.next()?.to_string(),
            model: parts.next()?.to_string(),
            serial: parts.next()?.to_string(),
            firmware: parts.next()?.to_string(),
        })
    }

    /// The family this instrument belongs to, or `None` for a model this
    /// crate has no device model for.
    pub fn family(&self) -> Option<Family> {
        Family::from_model(&self.model)
    }
}

/// A group of models that share one command set and one device model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    /// DP800-series bench power supplies (DP811, DP821, DP831, DP832, DP832A, …).
    Dp800,
    /// DHO800 and DHO900-series oscilloscopes (DHO802 … DHO924S).
    Dho900,
}

impl Family {
    /// Classifies a `*IDN?` model string by prefix.
    pub fn from_model(model: &str) -> Option<Self> {
        let m = model.trim().to_ascii_uppercase();
        if m.starts_with("DP8") {
            Some(Self::Dp800)
        } else if m.starts_with("DHO8") || m.starts_with("DHO9") {
            Some(Self::Dho900)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dp800 => "dp800",
            Self::Dho900 => "dho900",
        }
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Family {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dp800" => Ok(Self::Dp800),
            "dho900" | "dho800" => Ok(Self::Dho900),
            other => Err(format!(
                "unknown instrument family `{other}` (expected dp800 or dho900)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_power_supplies() {
        for model in ["DP832A", "DP832", "DP811", "DP821", "dp831"] {
            assert_eq!(Family::from_model(model), Some(Family::Dp800), "{model}");
        }
    }

    #[test]
    fn classifies_oscilloscopes() {
        for model in ["DHO924S", "DHO914", "DHO804", "DHO802"] {
            assert_eq!(Family::from_model(model), Some(Family::Dho900), "{model}");
        }
    }

    #[test]
    fn leaves_unknown_models_unclassified() {
        for model in ["DS1054Z", "MSO5074", "DL3021", "DP711", ""] {
            assert_eq!(Family::from_model(model), None, "{model}");
        }
    }

    #[test]
    fn identity_reports_its_family() {
        let idn = Identity::parse("RIGOL TECHNOLOGIES,DHO924S,DHO9S000000001,00.01.05").unwrap();
        assert_eq!(idn.family(), Some(Family::Dho900));
    }

    #[test]
    fn parses_family_names() {
        assert_eq!("dp800".parse(), Ok(Family::Dp800));
        assert_eq!("DHO900".parse(), Ok(Family::Dho900));
        assert!("ds1000z".parse::<Family>().is_err());
    }
}
