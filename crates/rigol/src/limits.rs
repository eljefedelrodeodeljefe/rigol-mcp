//! Per-channel safety ceilings.
//!
//! This is the load-bearing part of the crate. A bad write to a router costs
//! connectivity; a bad write here costs hardware — `:VOLT 30` into a 3.3 V board
//! destroys the board and possibly what it is soldered to.
//!
//! So the check lives *here*, in the library, below the MCP layer. A tool calls
//! [`crate::Dp800::set_voltage`] and gets `Err(LimitExceeded)`. There is no
//! argument, flag, or prompt that disables it, because callers never see the
//! check at all.

use crate::dp800::Rating;
use crate::error::{Error, Result};

/// Ceilings for the three channels, indexed 1-3.
///
/// A `None` entry means "not configured" — resolved at connect time to the
/// channel's own rating as reported by the instrument. Never to unlimited.
#[derive(Debug, Clone, Default)]
pub struct Limits {
    per_channel: [Option<Rating>; 3],
}

impl Limits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets an explicit ceiling for one channel, overriding its rating.
    /// Both values must be finite and above zero.
    pub fn with_channel(mut self, channel: u8, volts: f64, amps: f64) -> Result<Self> {
        let idx = index_of(channel)?;
        self.per_channel[idx] = Some(Rating::new(volts, amps)?);
        Ok(self)
    }

    /// Fills in any unconfigured channel from the instrument's own rating.
    ///
    /// Called once at connect. This is what makes "never unlimited" true: a
    /// channel with no configured ceiling still cannot be driven past what the
    /// hardware says it can do.
    pub fn backfill_from_ratings(&mut self, ratings: &[(u8, Rating)]) {
        for (channel, rating) in ratings {
            if let Ok(idx) = index_of(*channel)
                && self.per_channel[idx].is_none()
            {
                self.per_channel[idx] = Some(rating.clone());
            }
        }
    }

    pub fn ceiling(&self, channel: u8) -> Result<&Rating> {
        let idx = index_of(channel)?;
        self.per_channel[idx]
            .as_ref()
            .ok_or(Error::NoSuchChannel(channel))
    }

    /// Refuses a voltage above the channel's ceiling, or one that is not a
    /// finite number (`NaN > ceiling` is false, so it needs its own check).
    pub fn check_voltage(&self, channel: u8, requested: f64) -> Result<()> {
        let ceiling = self.ceiling(channel)?;
        finite("voltage (V)", requested)?;
        if requested > ceiling.volts {
            return Err(Error::LimitExceeded {
                what: "voltage",
                unit: "V",
                channel,
                requested,
                ceiling: ceiling.volts,
            });
        }
        Ok(())
    }

    /// Refuses a current above the channel's ceiling, or one that is not a
    /// finite number.
    pub fn check_current(&self, channel: u8, requested: f64) -> Result<()> {
        let ceiling = self.ceiling(channel)?;
        finite("current (A)", requested)?;
        if requested > ceiling.amps {
            return Err(Error::LimitExceeded {
                what: "current",
                unit: "A",
                channel,
                requested,
                ceiling: ceiling.amps,
            });
        }
        Ok(())
    }
}

fn finite(what: &'static str, value: f64) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(Error::OutOfRange {
            what,
            value,
            allowed: "a finite number".into(),
        })
    }
}

fn index_of(channel: u8) -> Result<usize> {
    match channel {
        1..=3 => Ok((channel - 1) as usize),
        _ => Err(Error::NoSuchChannel(channel)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        let mut l = Limits::new();
        l.backfill_from_ratings(&[
            (1, Rating::new(30.0, 3.0).unwrap()),
            (2, Rating::new(30.0, 3.0).unwrap()),
            (3, Rating::new(5.0, 3.0).unwrap()),
        ]);
        l
    }

    #[test]
    fn allows_a_setpoint_at_the_ceiling() {
        assert!(limits().check_voltage(1, 30.0).is_ok());
    }

    #[test]
    fn refuses_a_setpoint_above_the_ceiling() {
        let err = limits().check_voltage(3, 5.1).unwrap_err();
        assert!(matches!(err, Error::LimitExceeded { channel: 3, .. }));
    }

    #[test]
    fn an_explicit_ceiling_wins_over_the_rating() {
        let mut l = Limits::new().with_channel(1, 3.6, 0.5).unwrap();
        l.backfill_from_ratings(&[(1, Rating::new(30.0, 3.0).unwrap())]);
        assert!(l.check_voltage(1, 5.0).is_err(), "3.6 V ceiling must hold");
        assert!(l.check_voltage(1, 3.3).is_ok());
    }

    #[test]
    fn an_unconfigured_channel_is_not_unlimited() {
        // No backfill, no explicit ceiling: refuse rather than allow.
        assert!(Limits::new().check_voltage(1, 0.1).is_err());
    }

    #[test]
    fn a_ceiling_must_be_finite_and_positive() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
            assert!(
                Limits::new().with_channel(1, bad, 0.5).is_err(),
                "{bad} V accepted as a ceiling"
            );
            assert!(
                Limits::new().with_channel(1, 3.3, bad).is_err(),
                "{bad} A accepted as a ceiling"
            );
        }
    }

    /// `NaN > ceiling` is false, so a NaN request would pass a plain
    /// comparison.
    #[test]
    fn a_non_finite_request_is_refused() {
        for bad in [f64::NAN, f64::INFINITY] {
            assert!(limits().check_voltage(1, bad).is_err(), "{bad} V passed");
            assert!(limits().check_current(1, bad).is_err(), "{bad} A passed");
        }
    }

    #[test]
    fn rejects_channels_that_do_not_exist() {
        assert!(matches!(
            limits().check_voltage(4, 1.0).unwrap_err(),
            Error::NoSuchChannel(4)
        ));
    }
}
