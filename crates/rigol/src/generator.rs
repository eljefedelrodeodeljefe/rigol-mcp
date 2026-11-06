//! Safety for the oscilloscope's built-in signal generator.
//!
//! The generator drives a real signal into the device under test, so it is
//! held to the same rules as a power supply output:
//!
//! - **Configuring never energises.** Changing waveform, frequency, amplitude
//!   or offset never turns the output on; only an explicit output-on does.
//! - **The ceiling is on the peak, not the parts.** What reaches the device
//!   under test is `|offset| + amplitude / 2` (just `|offset|` for DC), so
//!   that is what is checked, alongside the amplitude itself.
//! - **Never unlimited.** Unconfigured ceilings default to the generator's
//!   own range: 10 Vpp and ±5 V peak (DHO800/DHO900 Programming Guide,
//!   `:SOURce:VOLTage:AMPLitude` and `:SOURce:VOLTage:OFFSet`).
//! - **Every intermediate state is inside the envelope.** The output may
//!   already be on while settings change, so the order of the individual
//!   commands matters; [`plan`] picks one that never steps outside.
//!
//! Everything here is pure, so the rules are tested without an instrument.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Largest amplitude the generator can produce at or below 10 MHz.
pub const HARDWARE_MAX_VPP: f64 = 10.0;
/// Largest instantaneous voltage the generator can produce: at full
/// amplitude the offset range shrinks to zero, so the peak is half of 10 Vpp.
pub const HARDWARE_MAX_PEAK_VOLTS: f64 = 5.0;
/// Smallest amplitude the generator accepts.
const MIN_VPP: f64 = 0.002;
/// Lowest frequency for every periodic waveform.
const MIN_HZ: f64 = 0.002;

/// Basic waveforms (`:SOURce:FUNCtion`). Arbitrary waveforms need uploaded
/// data and are not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Waveshape {
    Sine,
    Square,
    Ramp,
    /// A constant level: the output is the offset, and amplitude and
    /// frequency do not apply.
    Dc,
    Noise,
}

impl Waveshape {
    pub(crate) fn scpi(self) -> &'static str {
        match self {
            Self::Sine => "SINusoid",
            Self::Square => "SQUare",
            Self::Ramp => "RAMP",
            Self::Dc => "DC",
            Self::Noise => "NOISe",
        }
    }

    /// Parses the query form (`SIN`, `SQU`, `RAMP`, `DC`, `NOIS`). `ARB`
    /// and anything unknown are `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "SIN" | "SINUSOID" => Some(Self::Sine),
            "SQU" | "SQUARE" => Some(Self::Square),
            "RAMP" => Some(Self::Ramp),
            "DC" => Some(Self::Dc),
            "NOIS" | "NOISE" => Some(Self::Noise),
            _ => None,
        }
    }

    /// Highest frequency for this shape, or `None` where frequency does not
    /// apply.
    fn max_hz(self) -> Option<f64> {
        match self {
            Self::Sine => Some(25e6),
            Self::Square => Some(15e6),
            Self::Ramp => Some(150e3),
            Self::Dc | Self::Noise => None,
        }
    }
}

/// Ceilings on what the generator may put on its output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GeneratorLimits {
    pub max_vpp: f64,
    /// Largest `|offset| + amplitude / 2` (the signal's furthest excursion
    /// from 0 V).
    pub max_peak_volts: f64,
}

impl Default for GeneratorLimits {
    fn default() -> Self {
        Self {
            max_vpp: HARDWARE_MAX_VPP,
            max_peak_volts: HARDWARE_MAX_PEAK_VOLTS,
        }
    }
}

impl GeneratorLimits {
    /// Tighter ceilings than the hardware's. Neither may exceed what the
    /// generator can do, nor be zero or negative.
    pub fn new(max_vpp: f64, max_peak_volts: f64) -> Result<Self> {
        let check = |what: &'static str, v: f64, hw: f64| {
            if v.is_finite() && v > 0.0 && v <= hw {
                Ok(())
            } else {
                Err(Error::OutOfRange {
                    what,
                    value: v,
                    allowed: format!("above 0 and at most {hw}"),
                })
            }
        };
        check(
            "generator amplitude ceiling (Vpp)",
            max_vpp,
            HARDWARE_MAX_VPP,
        )?;
        check(
            "generator peak ceiling (V)",
            max_peak_volts,
            HARDWARE_MAX_PEAK_VOLTS,
        )?;
        Ok(Self {
            max_vpp,
            max_peak_volts,
        })
    }

    /// Refuses a combination whose amplitude or peak is over a ceiling, or
    /// that is not finite (`NaN > ceiling` is false, so a plain comparison
    /// would pass it).
    pub fn check(&self, s: &Signal) -> Result<()> {
        for (what, v) in [
            ("generator amplitude (Vpp)", s.amplitude_vpp),
            ("generator offset (V)", s.offset_volts),
        ] {
            if !v.is_finite() {
                return Err(Error::OutOfRange {
                    what,
                    value: v,
                    allowed: "a finite number".into(),
                });
            }
        }
        if s.shape != Waveshape::Dc && s.amplitude_vpp > self.max_vpp {
            return Err(Error::GeneratorLimit {
                what: "amplitude",
                unit: "Vpp",
                requested: s.amplitude_vpp,
                ceiling: self.max_vpp,
            });
        }
        let peak = s.peak_volts();
        if peak > self.max_peak_volts {
            return Err(Error::GeneratorLimit {
                what: "peak voltage (|offset| + amplitude/2)",
                unit: "V",
                requested: peak,
                ceiling: self.max_peak_volts,
            });
        }
        Ok(())
    }
}

/// The parts of the generator's setup that decide what reaches the output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Signal {
    pub shape: Waveshape,
    pub frequency_hz: f64,
    pub amplitude_vpp: f64,
    pub offset_volts: f64,
}

impl Signal {
    /// Furthest excursion from 0 V.
    pub fn peak_volts(&self) -> f64 {
        match self.shape {
            Waveshape::Dc => self.offset_volts.abs(),
            _ => self.offset_volts.abs() + self.amplitude_vpp / 2.0,
        }
    }
}

/// Requested changes. `None` keeps the current value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeneratorConfig {
    pub shape: Option<Waveshape>,
    pub frequency_hz: Option<f64>,
    pub amplitude_vpp: Option<f64>,
    pub offset_volts: Option<f64>,
}

impl GeneratorConfig {
    /// The signal that results from applying this to `current`.
    pub fn apply_to(&self, current: &Signal) -> Signal {
        Signal {
            shape: self.shape.unwrap_or(current.shape),
            frequency_hz: self.frequency_hz.unwrap_or(current.frequency_hz),
            amplitude_vpp: self.amplitude_vpp.unwrap_or(current.amplitude_vpp),
            offset_volts: self.offset_volts.unwrap_or(current.offset_volts),
        }
    }
}

/// One generator command, in the order [`plan`] decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change {
    Shape(Waveshape),
    Frequency(f64),
    Amplitude(f64),
    Offset(f64),
}

impl Change {
    pub(crate) fn scpi(&self) -> String {
        match self {
            Self::Shape(s) => format!(":SOURce:FUNCtion {}", s.scpi()),
            Self::Frequency(f) => format!(":SOURce:FREQuency {f}"),
            Self::Amplitude(a) => format!(":SOURce:VOLTage:AMPLitude {a}"),
            Self::Offset(o) => format!(":SOURce:VOLTage:OFFSet {o}"),
        }
    }
}

/// Validates `target` and orders the commands that take the generator from
/// `current` to it, so that every intermediate state is also within
/// `limits` whenever the current one is.
pub fn plan(current: &Signal, target: &Signal, limits: &GeneratorLimits) -> Result<Vec<Change>> {
    validate_hardware(target)?;
    limits.check(target)?;

    let mut out = Vec::new();
    let shape_changes = target.shape != current.shape;

    // Leaving DC first is never safe: amplitude starts to count. Entering DC
    // first always is: only the offset counts.
    if shape_changes && target.shape == Waveshape::Dc {
        out.push(Change::Shape(target.shape));
    }

    // Raising frequency past 10 MHz lowers the amplitude ceiling, so lower
    // the amplitude before raising the frequency, and the reverse.
    let freq = (target.frequency_hz != current.frequency_hz && target.shape.max_hz().is_some())
        .then_some(target.frequency_hz);
    let freq_first = freq.is_some_and(|f| f <= current.frequency_hz);
    if freq_first {
        out.extend(freq.map(Change::Frequency));
    }

    // Whichever change shrinks the excursion goes first. Shrinking the
    // offset first: |new offset| + old amplitude/2 <= the current peak.
    // Otherwise amplitude first: |old offset| + new amplitude/2 <= the
    // target peak.
    let amp = (target.amplitude_vpp != current.amplitude_vpp).then_some(target.amplitude_vpp);
    let off = (target.offset_volts != current.offset_volts).then_some(target.offset_volts);
    if target.offset_volts.abs() < current.offset_volts.abs() {
        out.extend(off.map(Change::Offset));
        out.extend(amp.map(Change::Amplitude));
    } else {
        out.extend(amp.map(Change::Amplitude));
        out.extend(off.map(Change::Offset));
    }

    if !freq_first {
        out.extend(freq.map(Change::Frequency));
    }
    if shape_changes && target.shape != Waveshape::Dc {
        out.push(Change::Shape(target.shape));
    }
    Ok(out)
}

/// The generator's own ranges, from the Programming Guide.
fn validate_hardware(s: &Signal) -> Result<()> {
    let range = |what: &'static str, v: f64, lo: f64, hi: f64| {
        if v.is_finite() && (lo..=hi).contains(&v) {
            Ok(())
        } else {
            Err(Error::OutOfRange {
                what,
                value: v,
                allowed: format!("{lo} to {hi}"),
            })
        }
    };
    if let Some(max) = s.shape.max_hz() {
        range("generator frequency (Hz)", s.frequency_hz, MIN_HZ, max)?;
    }
    if s.shape != Waveshape::Dc {
        let max = if s.frequency_hz > 10e6 && s.shape.max_hz().is_some() {
            HARDWARE_MAX_VPP / 2.0
        } else {
            HARDWARE_MAX_VPP
        };
        range("generator amplitude (Vpp)", s.amplitude_vpp, MIN_VPP, max)?;
    }
    if !s.offset_volts.is_finite() {
        return Err(Error::OutOfRange {
            what: "generator offset (V)",
            value: s.offset_volts,
            allowed: "a finite number".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amp: f64, off: f64) -> Signal {
        Signal {
            shape: Waveshape::Sine,
            frequency_hz: 1e3,
            amplitude_vpp: amp,
            offset_volts: off,
        }
    }

    fn peak_after_each_step(from: &Signal, steps: &[Change]) -> Vec<f64> {
        let mut s = *from;
        steps
            .iter()
            .map(|c| {
                match *c {
                    Change::Shape(x) => s.shape = x,
                    Change::Frequency(x) => s.frequency_hz = x,
                    Change::Amplitude(x) => s.amplitude_vpp = x,
                    Change::Offset(x) => s.offset_volts = x,
                }
                s.peak_volts()
            })
            .collect()
    }

    #[test]
    fn the_peak_counts_offset_and_half_the_amplitude() {
        assert_eq!(sine(4.0, -1.5).peak_volts(), 3.5);
        let dc = Signal {
            shape: Waveshape::Dc,
            ..sine(10.0, -2.0)
        };
        assert_eq!(dc.peak_volts(), 2.0);
    }

    #[test]
    fn each_value_in_range_can_still_exceed_the_peak() {
        let limits = GeneratorLimits::new(4.0, 3.3).unwrap();
        // 3 Vpp and 2 V offset are each fine; together they reach 3.5 V.
        let err = limits.check(&sine(3.0, 2.0)).unwrap_err();
        assert!(matches!(err, Error::GeneratorLimit { .. }), "{err:?}");
        assert!(limits.check(&sine(3.0, 1.5)).is_ok());
    }

    /// A reading that parsed as NaN compares false against every ceiling;
    /// output-on checks readings, so the check must refuse it outright.
    #[test]
    fn a_non_finite_signal_is_refused() {
        let limits = GeneratorLimits::default();
        assert!(limits.check(&sine(f64::NAN, 0.0)).is_err());
        assert!(limits.check(&sine(1.0, f64::NAN)).is_err());
        assert!(limits.check(&sine(1.0, f64::INFINITY)).is_err());
    }

    #[test]
    fn defaults_are_the_hardware_range_never_unlimited() {
        let d = GeneratorLimits::default();
        assert_eq!((d.max_vpp, d.max_peak_volts), (10.0, 5.0));
        assert!(GeneratorLimits::new(12.0, 3.0).is_err());
        assert!(GeneratorLimits::new(3.0, f64::INFINITY).is_err());
        assert!(GeneratorLimits::new(0.0, 1.0).is_err());
    }

    #[test]
    fn hardware_ranges_are_enforced() {
        let limits = GeneratorLimits::default();
        let too_fast = Signal {
            frequency_hz: 30e6,
            ..sine(1.0, 0.0)
        };
        assert!(plan(&sine(1.0, 0.0), &too_fast, &limits).is_err());
        let ramp_too_fast = Signal {
            shape: Waveshape::Ramp,
            frequency_hz: 200e3,
            ..sine(1.0, 0.0)
        };
        assert!(plan(&sine(1.0, 0.0), &ramp_too_fast, &limits).is_err());
        let hf_too_big = Signal {
            frequency_hz: 12e6,
            ..sine(6.0, 0.0)
        };
        assert!(plan(&sine(1.0, 0.0), &hf_too_big, &limits).is_err());
        assert!(plan(&sine(1.0, 0.0), &sine(0.001, 0.0), &limits).is_err());
    }

    #[test]
    fn a_shrinking_offset_goes_first() {
        let limits = GeneratorLimits::new(10.0, 4.0).unwrap();
        let (from, to) = (sine(1.0, 3.0), sine(6.0, 0.5));
        let steps = plan(&from, &to, &limits).unwrap();
        assert_eq!(steps, [Change::Offset(0.5), Change::Amplitude(6.0)]);
        assert!(
            peak_after_each_step(&from, &steps)
                .iter()
                .all(|p| *p <= 4.0)
        );
    }

    #[test]
    fn a_growing_offset_goes_last() {
        let limits = GeneratorLimits::new(10.0, 4.0).unwrap();
        let (from, to) = (sine(6.0, 0.5), sine(1.0, 3.0));
        let steps = plan(&from, &to, &limits).unwrap();
        assert_eq!(steps, [Change::Amplitude(1.0), Change::Offset(3.0)]);
        assert!(
            peak_after_each_step(&from, &steps)
                .iter()
                .all(|p| *p <= 4.0)
        );
    }

    #[test]
    fn leaving_dc_happens_after_the_amplitude_is_safe() {
        let limits = GeneratorLimits::new(10.0, 3.0).unwrap();
        let from = Signal {
            shape: Waveshape::Dc,
            ..sine(10.0, 1.0)
        };
        let to = sine(2.0, 1.0);
        let steps = plan(&from, &to, &limits).unwrap();
        assert_eq!(steps.last(), Some(&Change::Shape(Waveshape::Sine)));
        assert!(
            peak_after_each_step(&from, &steps)
                .iter()
                .all(|p| *p <= 3.0)
        );
    }

    #[test]
    fn entering_dc_happens_first() {
        let limits = GeneratorLimits::default();
        let from = sine(8.0, 0.0);
        let to = Signal {
            shape: Waveshape::Dc,
            ..sine(8.0, 2.0)
        };
        let steps = plan(&from, &to, &limits).unwrap();
        assert_eq!(steps.first(), Some(&Change::Shape(Waveshape::Dc)));
    }

    #[test]
    fn raising_frequency_past_10_mhz_lowers_amplitude_first() {
        let limits = GeneratorLimits::default();
        let from = sine(8.0, 0.0);
        let to = Signal {
            frequency_hz: 20e6,
            ..sine(4.0, 0.0)
        };
        let steps = plan(&from, &to, &limits).unwrap();
        assert_eq!(steps, [Change::Amplitude(4.0), Change::Frequency(20e6)]);
    }

    /// Exhaustive check over a grid: whenever both ends are inside the
    /// envelope, so is every step between them.
    #[test]
    fn no_plan_ever_steps_outside_the_envelope() {
        let limits = GeneratorLimits::new(8.0, 4.0).unwrap();
        let shapes = [Waveshape::Sine, Waveshape::Dc, Waveshape::Square];
        let amps = [0.5, 2.0, 6.0, 8.0];
        let offs = [-3.5, -1.0, 0.0, 1.0, 3.5];
        let mut signals = Vec::new();
        for shape in shapes {
            for amp in amps {
                for off in offs {
                    let s = Signal {
                        shape,
                        ..sine(amp, off)
                    };
                    if limits.check(&s).is_ok() {
                        signals.push(s);
                    }
                }
            }
        }
        for from in &signals {
            for to in &signals {
                let steps = plan(from, to, &limits).unwrap();
                for p in peak_after_each_step(from, &steps) {
                    assert!(p <= 4.0, "{from:?} -> {to:?} via {steps:?} peaks at {p}");
                }
            }
        }
    }
}
