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
//!   own range: 10 Vpp and ±5 V peak on a DHO800/DHO900 (Programming Guide,
//!   `:SOURce:VOLTage:AMPLitude` and `:SOURce:VOLTage:OFFSet`), 20 Vpp and
//!   ±10 V peak on an MHO900 (see [`GeneratorHardware`]).
//! - **Every intermediate state is inside the envelope.** The output may
//!   already be on while settings change, so the order of the individual
//!   commands matters; [`plan`] picks one that never steps outside.
//!
//! Everything here is pure, so the rules are tested without an instrument.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::idn::Family;

/// Largest amplitude the DHO800/DHO900 generator can produce at or below
/// 10 MHz.
pub const HARDWARE_MAX_VPP: f64 = 10.0;
/// Largest instantaneous voltage the DHO800/DHO900 generator can produce: at
/// full amplitude the offset range shrinks to zero, so the peak is half of
/// 10 Vpp.
pub const HARDWARE_MAX_PEAK_VOLTS: f64 = 5.0;
/// Largest open-circuit amplitude of an MHO900 generator channel at or below
/// 50 MHz: 20 Vpp into HighZ, which is 10 Vpp into 50 Ω (MHO900 Programming
/// Guide, `:SOURce<n>:VOLTage:AMPLitude`).
pub const MHO900_MAX_VPP: f64 = 20.0;
/// Largest open-circuit excursion of an MHO900 generator channel: ±10 Vpk
/// into HighZ, ±5 Vpk into 50 Ω (MHO900 Programming Guide,
/// `:SOURce<n>:VOLTage:OFFSet`).
pub const MHO900_MAX_PEAK_VOLTS: f64 = 10.0;

/// The built-in generator a scope carries, and so the ranges it accepts.
///
/// Every MHO900 figure is an **open-circuit** voltage: the most the output
/// can put on a device under test, whatever the load. The MHO900 states
/// amplitude and offset into the load its output impedance setting names
/// (`:SOURce<n>:IMPedance`); set to 50 Ω, an open circuit sees twice the
/// setting, so the device model doubles the setting before it is compared
/// with these ranges or with a ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratorHardware {
    /// DHO800/DHO900: one channel, `:SOURce`.
    Dho900,
    /// MHO900 with option MHO900-AFG100: two channels, sine to 100 MHz.
    Mho900Afg100,
    /// MHO900 with option MHO900-AFG50: two channels, sine to 50 MHz.
    Mho900Afg50,
}

impl GeneratorHardware {
    /// Highest frequency for a shape, or `None` where frequency does not
    /// apply (DC and noise).
    pub fn max_hz(self, shape: Waveshape) -> Option<f64> {
        let (sine, square, ramp) = match self {
            Self::Dho900 => (25e6, 15e6, 150e3),
            Self::Mho900Afg100 => (100e6, 20e6, 2e6),
            Self::Mho900Afg50 => (50e6, 10e6, 1e6),
        };
        match shape {
            Waveshape::Sine => Some(sine),
            Waveshape::Square => Some(square),
            Waveshape::Ramp => Some(ramp),
            Waveshape::Dc | Waveshape::Noise => None,
        }
    }

    /// Lowest frequency for every periodic waveform.
    pub fn min_hz(self) -> f64 {
        match self {
            Self::Dho900 | Self::Mho900Afg100 => 0.002,
            Self::Mho900Afg50 => 0.02,
        }
    }

    /// Smallest amplitude, in Vpp.
    pub fn min_vpp(self) -> f64 {
        0.002
    }

    /// Largest amplitude at a frequency, in Vpp.
    pub fn max_vpp_at(self, frequency_hz: f64) -> f64 {
        match self {
            Self::Dho900 if frequency_hz > 10e6 => HARDWARE_MAX_VPP / 2.0,
            Self::Dho900 => HARDWARE_MAX_VPP,
            Self::Mho900Afg100 | Self::Mho900Afg50 if frequency_hz > 50e6 => MHO900_MAX_VPP / 2.0,
            Self::Mho900Afg100 | Self::Mho900Afg50 => MHO900_MAX_VPP,
        }
    }

    /// Largest `|offset| + amplitude/2` at a frequency, where the guide gives
    /// one. On the MHO900 the offset range is ±(max amplitude − amplitude)/2,
    /// so the excursion is at most half the largest amplitude.
    fn max_peak_at(self, frequency_hz: f64) -> Option<f64> {
        match self {
            Self::Dho900 => None,
            Self::Mho900Afg100 | Self::Mho900Afg50 => Some(self.max_vpp_at(frequency_hz) / 2.0),
        }
    }
}

/// The largest ceilings a family's generator can take, `(Vpp, peak V)`.
fn family_range(family: Family) -> Result<(f64, f64)> {
    match family {
        Family::Dho900 => Ok((HARDWARE_MAX_VPP, HARDWARE_MAX_PEAK_VOLTS)),
        Family::Mho900 => Ok((MHO900_MAX_VPP, MHO900_MAX_PEAK_VOLTS)),
        Family::Dp800 => Err(Error::NotOnThisModel {
            what: "a signal generator",
            model: "DP800".into(),
        }),
    }
}

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

    /// Whether a frequency applies to this shape.
    fn has_frequency(self) -> bool {
        !matches!(self, Self::Dc | Self::Noise)
    }
}

/// Ceilings on what the generator may put on its output.
///
/// Built only through [`GeneratorLimits::new`] or
/// [`GeneratorLimits::for_family`], which validate, or [`Default`] and
/// [`GeneratorLimits::hardware`], the hardware range. The fields are private so a struct
/// literal cannot skip the validation:
///
/// ```compile_fail
/// let unlimited = rigol::GeneratorLimits {
///     max_vpp: f64::INFINITY,
///     max_peak_volts: f64::INFINITY,
/// };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GeneratorLimits {
    max_vpp: f64,
    /// Largest `|offset| + amplitude / 2` (the signal's furthest excursion
    /// from 0 V).
    max_peak_volts: f64,
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
    /// Tighter ceilings than a DHO800/DHO900 generator's. Neither may exceed
    /// what the generator can do, nor be zero or negative.
    pub fn new(max_vpp: f64, max_peak_volts: f64) -> Result<Self> {
        Self::for_family(Family::Dho900, max_vpp, max_peak_volts)
    }

    /// The hardware range of a family's generator: 10 Vpp / 5 V on a
    /// DHO800/DHO900, 20 Vpp / 10 V open-circuit on an MHO900.
    pub fn hardware(family: Family) -> Result<Self> {
        let (max_vpp, max_peak_volts) = family_range(family)?;
        Ok(Self {
            max_vpp,
            max_peak_volts,
        })
    }

    /// Tighter ceilings than a family's hardware range. Neither may exceed
    /// what that family's generator can do, nor be zero or negative.
    pub fn for_family(family: Family, max_vpp: f64, max_peak_volts: f64) -> Result<Self> {
        let (hw_vpp, hw_peak) = family_range(family)?;
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
        check("generator amplitude ceiling (Vpp)", max_vpp, hw_vpp)?;
        check("generator peak ceiling (V)", max_peak_volts, hw_peak)?;
        Ok(Self {
            max_vpp,
            max_peak_volts,
        })
    }

    /// Largest amplitude allowed, in Vpp.
    pub fn max_vpp(&self) -> f64 {
        self.max_vpp
    }

    /// Largest `|offset| + amplitude / 2` allowed, in volts.
    pub fn max_peak_volts(&self) -> f64 {
        self.max_peak_volts
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

    /// The same signal with amplitude and offset multiplied by `factor`:
    /// 2 turns an MHO900 setting into 50 Ω into the open-circuit voltage.
    pub fn scaled(&self, factor: f64) -> Self {
        Self {
            amplitude_vpp: self.amplitude_vpp * factor,
            offset_volts: self.offset_volts * factor,
            ..*self
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
    /// The command under `source`: `:SOURce`, or `:SOURce<n>` on an MHO900.
    pub(crate) fn scpi_on(&self, source: &str) -> String {
        match self {
            Self::Shape(s) => format!("{source}:FUNCtion {}", s.scpi()),
            Self::Frequency(f) => format!("{source}:FREQuency {f}"),
            Self::Amplitude(a) => format!("{source}:VOLTage:AMPLitude {a}"),
            Self::Offset(o) => format!("{source}:VOLTage:OFFSet {o}"),
        }
    }

    /// The same change with a voltage divided by `factor`, to turn an
    /// open-circuit value back into the setting the instrument takes.
    pub(crate) fn divided(self, factor: f64) -> Self {
        match self {
            Self::Amplitude(a) => Self::Amplitude(a / factor),
            Self::Offset(o) => Self::Offset(o / factor),
            other => other,
        }
    }
}

/// Validates `target` and orders the commands that take a DHO800/DHO900
/// generator from `current` to it, so that every intermediate state is also
/// within `limits` whenever the current one is.
pub fn plan(current: &Signal, target: &Signal, limits: &GeneratorLimits) -> Result<Vec<Change>> {
    plan_on(GeneratorHardware::Dho900, current, target, limits)
}

/// [`plan`] for a given generator. Every voltage, in and out, is what the
/// output puts on an open circuit (see [`GeneratorHardware`]).
pub fn plan_on(
    hw: GeneratorHardware,
    current: &Signal,
    target: &Signal,
    limits: &GeneratorLimits,
) -> Result<Vec<Change>> {
    validate_hardware(hw, target)?;
    limits.check(target)?;

    let mut out = Vec::new();
    let shape_changes = target.shape != current.shape;

    // Leaving DC first is never safe: amplitude starts to count. Entering DC
    // first always is: only the offset counts.
    if shape_changes && target.shape == Waveshape::Dc {
        out.push(Change::Shape(target.shape));
    }

    // Raising frequency past 10 MHz (50 MHz on an MHO900) lowers the
    // amplitude ceiling, so lower the amplitude before raising the
    // frequency, and the reverse.
    let freq = (target.frequency_hz != current.frequency_hz && target.shape.has_frequency())
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
fn validate_hardware(hw: GeneratorHardware, s: &Signal) -> Result<()> {
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
    if let Some(max) = hw.max_hz(s.shape) {
        range("generator frequency (Hz)", s.frequency_hz, hw.min_hz(), max)?;
    }
    // Where no frequency applies, the lowest-frequency range does.
    let hz = if s.shape.has_frequency() {
        s.frequency_hz
    } else {
        0.0
    };
    if s.shape != Waveshape::Dc {
        let max = hw.max_vpp_at(hz);
        range(
            "generator amplitude (Vpp)",
            s.amplitude_vpp,
            hw.min_vpp(),
            max,
        )?;
    }
    if !s.offset_volts.is_finite() {
        return Err(Error::OutOfRange {
            what: "generator offset (V)",
            value: s.offset_volts,
            allowed: "a finite number".into(),
        });
    }
    if let Some(max) = hw.max_peak_at(hz) {
        range(
            "generator peak (|offset| + amplitude/2, V)",
            s.peak_volts(),
            0.0,
            max,
        )?;
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
        assert_eq!((d.max_vpp(), d.max_peak_volts()), (10.0, 5.0));
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

    fn mho_limits() -> GeneratorLimits {
        GeneratorLimits::hardware(Family::Mho900).unwrap()
    }

    #[test]
    fn mho900_ceilings_default_to_its_open_circuit_range() {
        let d = mho_limits();
        assert_eq!((d.max_vpp(), d.max_peak_volts()), (20.0, 10.0));
        assert!(GeneratorLimits::for_family(Family::Mho900, 20.0, 10.0).is_ok());
        assert!(GeneratorLimits::for_family(Family::Mho900, 20.5, 10.0).is_err());
        assert!(GeneratorLimits::for_family(Family::Mho900, 3.3, f64::NAN).is_err());
        // The DHO range stays what it was.
        assert!(GeneratorLimits::for_family(Family::Dho900, 20.0, 10.0).is_err());
        assert!(GeneratorLimits::hardware(Family::Dp800).is_err());
    }

    #[test]
    fn mho900_hardware_ranges_follow_the_option() {
        let limits = mho_limits();
        let from = sine(1.0, 0.0);
        let at = |hz: f64, amp: f64| Signal {
            frequency_hz: hz,
            ..sine(amp, 0.0)
        };
        let afg100 = GeneratorHardware::Mho900Afg100;
        let afg50 = GeneratorHardware::Mho900Afg50;
        assert!(plan_on(afg100, &from, &at(100e6, 1.0), &limits).is_ok());
        assert!(plan_on(afg100, &from, &at(101e6, 1.0), &limits).is_err());
        assert!(plan_on(afg50, &from, &at(60e6, 1.0), &limits).is_err());
        assert!(plan_on(afg50, &from, &at(0.01, 1.0), &limits).is_err());
        assert!(plan_on(afg100, &from, &at(0.01, 1.0), &limits).is_ok());
        // 20 Vpp open-circuit up to 50 MHz, 10 Vpp above.
        assert!(plan_on(afg100, &from, &at(50e6, 20.0), &limits).is_ok());
        assert!(plan_on(afg100, &from, &at(60e6, 12.0), &limits).is_err());
        let ramp = Signal {
            shape: Waveshape::Ramp,
            frequency_hz: 1.5e6,
            ..sine(1.0, 0.0)
        };
        assert!(plan_on(afg100, &from, &ramp, &limits).is_ok());
        assert!(plan_on(afg50, &from, &ramp, &limits).is_err());
    }

    /// The offset range shrinks as the amplitude grows: ±(20 − amplitude)/2
    /// open-circuit, so the excursion never passes 10 V.
    #[test]
    fn mho900_hardware_bounds_the_excursion() {
        let limits = mho_limits();
        let hw = GeneratorHardware::Mho900Afg100;
        assert!(plan_on(hw, &sine(1.0, 0.0), &sine(12.0, 4.0), &limits).is_ok());
        assert!(plan_on(hw, &sine(1.0, 0.0), &sine(12.0, 4.5), &limits).is_err());
        let dc = Signal {
            shape: Waveshape::Dc,
            ..sine(1.0, -10.5)
        };
        assert!(plan_on(hw, &sine(1.0, 0.0), &dc, &limits).is_err());
    }

    #[test]
    fn scaling_doubles_voltages_and_division_undoes_it() {
        let s = sine(3.0, -1.25).scaled(2.0);
        assert_eq!((s.amplitude_vpp, s.offset_volts), (6.0, -2.5));
        assert_eq!(s.frequency_hz, 1e3);
        assert_eq!(Change::Amplitude(6.0).divided(2.0), Change::Amplitude(3.0));
        assert_eq!(Change::Offset(-2.5).divided(2.0), Change::Offset(-1.25));
        assert_eq!(Change::Frequency(1e3).divided(2.0), Change::Frequency(1e3));
    }

    #[test]
    fn commands_go_to_the_named_source() {
        assert_eq!(
            Change::Amplitude(1.5).scpi_on(":SOURce2"),
            ":SOURce2:VOLTage:AMPLitude 1.5"
        );
        assert_eq!(
            Change::Shape(Waveshape::Square).scpi_on(":SOURce"),
            ":SOURce:FUNCtion SQUare"
        );
    }

    /// The exhaustive envelope check again, on MHO900 hardware with an
    /// output set to 50 Ω: planned in open-circuit volts, sent as halves.
    #[test]
    fn no_mho900_plan_steps_outside_the_envelope() {
        let limits = GeneratorLimits::for_family(Family::Mho900, 16.0, 8.0).unwrap();
        let hw = GeneratorHardware::Mho900Afg100;
        let shapes = [Waveshape::Sine, Waveshape::Dc, Waveshape::Square];
        let amps = [0.5, 2.0, 6.0, 8.0];
        let offs = [-3.5, -1.0, 0.0, 1.0, 3.5];
        let mut settings = Vec::new();
        for shape in shapes {
            for amp in amps {
                for off in offs {
                    let s = Signal {
                        shape,
                        ..sine(amp, off)
                    };
                    if limits.check(&s.scaled(2.0)).is_ok() {
                        settings.push(s);
                    }
                }
            }
        }
        for from in &settings {
            for to in &settings {
                let steps: Vec<Change> = plan_on(hw, &from.scaled(2.0), &to.scaled(2.0), &limits)
                    .unwrap()
                    .into_iter()
                    .map(|c| c.divided(2.0))
                    .collect();
                for p in peak_after_each_step(from, &steps) {
                    assert!(
                        p * 2.0 <= 8.0,
                        "{from:?} -> {to:?} via {steps:?} peaks at {p}"
                    );
                }
            }
        }
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
