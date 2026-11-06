//! The DP800-series device model.
//!
//! Command spellings follow the RIGOL DP800 Programming Guide, but where this
//! firmware disagrees with the guide the firmware wins — see [`LanConfig`] and
//! the firmware-quirks section of the README.

use serde::Serialize;
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::idn::Family;
pub use crate::idn::Identity;
use crate::limits::Limits;
use crate::scpi::{DEFAULT_PORT, InstrumentError, Session, SessionConfig};

/// What a channel is physically capable of, e.g. 30 V / 3 A.
///
/// Also the shape of a safety ceiling, so both values are finite and above
/// zero: a NaN ceiling compares false against every request and would allow
/// all of them. The fields are private so a struct literal cannot skip
/// [`Rating::new`]:
///
/// ```compile_fail
/// let unlimited = rigol::Rating { volts: f64::NAN, amps: f64::NAN };
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rating {
    volts: f64,
    amps: f64,
}

impl Rating {
    pub fn new(volts: f64, amps: f64) -> Result<Self> {
        positive("voltage rating or ceiling (V)", volts)?;
        positive("current rating or ceiling (A)", amps)?;
        Ok(Self { volts, amps })
    }

    pub fn volts(&self) -> f64 {
        self.volts
    }

    pub fn amps(&self) -> f64 {
        self.amps
    }
}

fn positive(what: &'static str, value: f64) -> Result<()> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(Error::OutOfRange {
            what,
            value,
            allowed: "a finite number above 0".into(),
        })
    }
}

/// What a channel is currently *set* to — not what it is delivering.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Setpoint {
    pub volts: f64,
    pub amps: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Channel {
    pub channel: u8,
    pub rating: Rating,
    pub setpoint: Setpoint,
}

impl Channel {
    /// Parses `CH1:30V/3A,0.000,3.000` — one round trip yields both the
    /// channel's rating and its setpoints.
    pub fn parse(channel: u8, raw: &str) -> Result<Self> {
        let fail = || Error::Parse {
            cmd: format!(":APPL? CH{channel}"),
            reply: raw.to_string(),
        };

        let mut parts = raw.split(',');
        let head = parts.next().ok_or_else(fail)?;
        let volts: f64 = parts
            .next()
            .ok_or_else(fail)?
            .trim()
            .parse()
            .map_err(|_| fail())?;
        let amps: f64 = parts
            .next()
            .ok_or_else(fail)?
            .trim()
            .parse()
            .map_err(|_| fail())?;

        let (_, rating) = head.split_once(':').ok_or_else(fail)?;
        let (v, a) = rating.split_once('/').ok_or_else(fail)?;

        // The rating backfills the safety ceiling, so it is validated like one.
        let rating = Rating::new(
            strip_unit(v).ok_or_else(fail)?,
            strip_unit(a).ok_or_else(fail)?,
        )
        .map_err(|_| fail())?;

        Ok(Self {
            channel,
            rating,
            setpoint: Setpoint { volts, amps },
        })
    }
}

/// A live reading: what the channel is actually delivering.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measurement {
    pub channel: u8,
    pub volts: f64,
    pub amps: f64,
    pub watts: f64,
}

impl Measurement {
    /// Parses `0.0000,0.0000,0.000`.
    pub fn parse(channel: u8, raw: &str) -> Result<Self> {
        let fail = || Error::Parse {
            cmd: format!(":MEAS:ALL? CH{channel}"),
            reply: raw.to_string(),
        };
        let mut p = raw.split(',').map(|s| s.trim().parse::<f64>());
        Ok(Self {
            channel,
            volts: p.next().ok_or_else(fail)?.map_err(|_| fail())?,
            amps: p.next().ok_or_else(fail)?.map_err(|_| fail())?,
            watts: p.next().ok_or_else(fail)?.map_err(|_| fail())?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputState {
    On,
    Off,
}

impl OutputState {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "ON" | "1" => Some(Self::On),
            "OFF" | "0" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn is_on(self) -> bool {
        self == Self::On
    }
}

/// Over-voltage and over-current protection for one channel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Protection {
    pub channel: u8,
    pub ovp_enabled: Option<bool>,
    pub ovp_volts: Option<f64>,
    pub ovp_tripped: Option<bool>,
    pub ocp_enabled: Option<bool>,
    pub ocp_amps: Option<f64>,
    pub ocp_tripped: Option<bool>,
}

/// The LAN subsystem, with every field optional.
///
/// Not defensiveness for its own sake: on firmware 00.01.19 the documented
/// `:SMAS?`, `:AIP?`, `:MANU?` and `:VISA?` spellings are rejected outright with
/// `-113 Undefined header`, and even the accepted `:MASK?` returns an empty
/// string. "Supported" and "populated" are separate facts here.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LanConfig {
    pub mac: Option<String>,
    pub ip: Option<String>,
    pub mask: Option<String>,
    pub gateway: Option<String>,
    pub dns: Option<String>,
    pub dhcp: Option<String>,
    pub auto_ip: Option<String>,
    pub manual_ip: Option<String>,
    /// Queries that this firmware answered with something non-empty.
    pub supported_queries: Vec<String>,
}

/// A DP800-series power supply. Every write goes through the ceilings in
/// [`Limits`]; the SCPI session underneath is not handed out, because a raw
/// command would go around them:
///
/// ```compile_fail
/// async fn bypass(psu: &rigol::Dp800) {
///     psu.session().command(":SOUR1:VOLT 30").await.unwrap();
/// }
/// ```
pub struct Dp800 {
    session: Session,
    limits: Limits,
    identity: Identity,
    /// Held across every write, from its check to its last command, so a
    /// check and the write it permits are never split by another caller's
    /// write. The session serialises single commands only.
    operation: Mutex<()>,
}

impl Dp800 {
    /// Connects, reads identity, and resolves any unconfigured safety ceiling
    /// from the instrument's own channel ratings.
    pub async fn connect(
        host: &str,
        port: Option<u16>,
        cfg: SessionConfig,
        mut limits: Limits,
    ) -> Result<Self> {
        let addr = format!("{host}:{}", port.unwrap_or(DEFAULT_PORT));
        let session = Session::connect(addr, cfg).await?;

        let raw = session.query("*IDN?").await?;
        let identity = Identity::parse(&raw).ok_or_else(|| Error::Parse {
            cmd: "*IDN?".into(),
            reply: raw.clone(),
        })?;
        // Before any DP800 command: another family's firmware would ignore
        // them, and an ignored query is a hung one.
        if identity.family() != Some(Family::Dp800) {
            return Err(Error::WrongInstrument {
                expected: Family::Dp800,
                model: identity.model,
            });
        }

        // Backfill ceilings from ratings before anything can be written.
        let mut ratings = Vec::new();
        for ch in 1..=3u8 {
            if let Ok(c) = Self::read_channel(&session, ch).await {
                ratings.push((ch, c.rating));
            }
        }
        limits.backfill_from_ratings(&ratings);

        tracing::info!(
            model = %identity.model,
            serial = %identity.serial,
            firmware = %identity.firmware,
            "connected"
        );

        Ok(Self {
            session,
            limits,
            identity,
            operation: Mutex::new(()),
        })
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    async fn read_channel(session: &Session, channel: u8) -> Result<Channel> {
        validate(channel)?;
        let raw = session.query(&format!(":APPL? CH{channel}")).await?;
        Channel::parse(channel, &raw)
    }

    pub async fn channel(&self, channel: u8) -> Result<Channel> {
        Self::read_channel(&self.session, channel).await
    }

    pub async fn channels(&self) -> Result<Vec<Channel>> {
        let mut out = Vec::with_capacity(3);
        for ch in 1..=3u8 {
            out.push(self.channel(ch).await?);
        }
        Ok(out)
    }

    pub async fn measure(&self, channel: u8) -> Result<Measurement> {
        validate(channel)?;
        let raw = self
            .session
            .query(&format!(":MEAS:ALL? CH{channel}"))
            .await?;
        Measurement::parse(channel, &raw)
    }

    pub async fn output_state(&self, channel: u8) -> Result<OutputState> {
        validate(channel)?;
        let raw = self.session.query(&format!(":OUTP? CH{channel}")).await?;
        OutputState::parse(&raw).ok_or(Error::Parse {
            cmd: format!(":OUTP? CH{channel}"),
            reply: raw,
        })
    }

    // ── Writes ────────────────────────────────────────────────────────────────
    //
    // Every one of these checks the ceiling first. Setting and energising are
    // deliberately separate operations: nothing here turns an output on as a
    // side effect of changing a setpoint.

    pub async fn set_voltage(&self, channel: u8, volts: f64) -> Result<()> {
        validate(channel)?;
        let _operation = self.operation.lock().await;
        self.limits.check_voltage(channel, volts)?;
        tracing::info!(op = "set_voltage", channel, volts, "setting voltage");
        self.session
            .command(&format!(":SOUR{channel}:VOLT {volts}"))
            .await
    }

    pub async fn set_current(&self, channel: u8, amps: f64) -> Result<()> {
        validate(channel)?;
        let _operation = self.operation.lock().await;
        self.limits.check_current(channel, amps)?;
        tracing::info!(op = "set_current", channel, amps, "setting current limit");
        self.session
            .command(&format!(":SOUR{channel}:CURR {amps}"))
            .await
    }

    /// Energises a channel. Logged at info level — every energisation leaves a
    /// trace, because this one moves real power into real hardware.
    ///
    /// The setpoints are checked against the ceilings first. They need not
    /// have come through [`Dp800::set_voltage`]: the front panel, or anything
    /// that ran before this process, can leave one above the ceiling.
    pub async fn output_on(&self, channel: u8) -> Result<()> {
        validate(channel)?;
        let _operation = self.operation.lock().await;
        let before = self.channel(channel).await?;
        self.limits.check_voltage(channel, before.setpoint.volts)?;
        self.limits.check_current(channel, before.setpoint.amps)?;
        tracing::info!(
            op = "output_on",
            channel,
            volts = before.setpoint.volts,
            amps = before.setpoint.amps,
            "ENERGISING output"
        );
        self.session.command(&format!(":OUTP CH{channel},ON")).await
    }

    pub async fn output_off(&self, channel: u8) -> Result<()> {
        validate(channel)?;
        let _operation = self.operation.lock().await;
        tracing::info!(op = "output_off", channel, "de-energising output");
        self.session
            .command(&format!(":OUTP CH{channel},OFF"))
            .await
    }

    pub async fn set_ovp(&self, channel: u8, volts: f64) -> Result<()> {
        validate(channel)?;
        let _operation = self.operation.lock().await;
        self.limits.check_voltage(channel, volts)?;
        tracing::info!(
            op = "set_ovp",
            channel,
            volts,
            "setting over-voltage protection"
        );
        self.session
            .command(&format!(":OUTP:OVP:VAL CH{channel},{volts}"))
            .await
    }

    pub async fn set_ocp(&self, channel: u8, amps: f64) -> Result<()> {
        validate(channel)?;
        let _operation = self.operation.lock().await;
        self.limits.check_current(channel, amps)?;
        tracing::info!(
            op = "set_ocp",
            channel,
            amps,
            "setting over-current protection"
        );
        self.session
            .command(&format!(":OUTP:OCP:VAL CH{channel},{amps}"))
            .await
    }

    pub async fn protection(&self, channel: u8) -> Result<Protection> {
        validate(channel)?;
        Ok(Protection {
            channel,
            ovp_enabled: self.opt_bool(&format!(":OUTP:OVP? CH{channel}")).await,
            ovp_volts: self.opt_f64(&format!(":OUTP:OVP:VAL? CH{channel}")).await,
            ovp_tripped: self.opt_trip(&format!(":OUTP:OVP:ALAR? CH{channel}")).await,
            ocp_enabled: self.opt_bool(&format!(":OUTP:OCP? CH{channel}")).await,
            ocp_amps: self.opt_f64(&format!(":OUTP:OCP:VAL? CH{channel}")).await,
            ocp_tripped: self.opt_trip(&format!(":OUTP:OCP:ALAR? CH{channel}")).await,
        })
    }

    /// Reads the LAN subsystem, probing rather than assuming.
    pub async fn lan_config(&self) -> Result<LanConfig> {
        let mut supported = Vec::new();

        let mac = self.probe(":SYST:COMM:LAN:MAC?", &mut supported).await;
        let ip = self.probe(":SYST:COMM:LAN:IPAD?", &mut supported).await;
        let mask = self.probe(":SYST:COMM:LAN:MASK?", &mut supported).await;
        let gateway = self.probe(":SYST:COMM:LAN:GATE?", &mut supported).await;
        let dns = self.probe(":SYST:COMM:LAN:DNS?", &mut supported).await;
        let dhcp = self.probe(":SYST:COMM:LAN:DHCP?", &mut supported).await;
        let auto_ip = self.probe(":SYST:COMM:LAN:AUTO?", &mut supported).await;
        let manual_ip = self.probe(":SYST:COMM:LAN:MAN?", &mut supported).await;

        // Undocumented headers land in the error queue; do not leave ours there.
        if let Err(e) = self.session.clear_status().await {
            tracing::warn!(error = %e, "could not clear the error queue after the LAN probe");
        }

        Ok(LanConfig {
            mac: mac.as_deref().map(normalise_mac),
            ip,
            mask,
            gateway,
            dns,
            dhcp,
            auto_ip,
            manual_ip,
            supported_queries: supported,
        })
    }

    /// One capability probe: returns the trimmed reply only if the firmware
    /// answered with something, and records the command as supported if so.
    async fn probe(&self, cmd: &str, supported: &mut Vec<String>) -> Option<String> {
        match self.session.query(cmd).await {
            Ok(v) if !v.trim().is_empty() => {
                supported.push(cmd.to_string());
                Some(v.trim().to_string())
            }
            Ok(_) => {
                tracing::debug!(%cmd, "empty reply — unsupported or unpopulated");
                None
            }
            Err(e) => {
                tracing::debug!(%cmd, error = %e, "query failed");
                None
            }
        }
    }

    pub async fn errors(&self) -> Result<Vec<InstrumentError>> {
        self.session.errors().await
    }

    async fn opt_f64(&self, cmd: &str) -> Option<f64> {
        self.session.query(cmd).await.ok()?.trim().parse().ok()
    }

    async fn opt_bool(&self, cmd: &str) -> Option<bool> {
        OutputState::parse(&self.session.query(cmd).await.ok()?).map(OutputState::is_on)
    }

    /// The alarm queries answer `YES`/`NO`, not `ON`/`OFF`.
    ///
    /// Note the spelling: `:OUTP:OVP:ALAR?` is accepted, while the guide's
    /// long-form `:OUTP:OVP:ALARm?` is rejected with `-113`. Abbreviated wins.
    async fn opt_trip(&self, cmd: &str) -> Option<bool> {
        parse_yes_no(&self.session.query(cmd).await.ok()?)
    }
}

fn validate(channel: u8) -> Result<()> {
    match channel {
        1..=3 => Ok(()),
        _ => Err(Error::NoSuchChannel(channel)),
    }
}

/// The protection-alarm queries answer `YES`/`NO`.
fn parse_yes_no(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "YES" | "1" => Some(true),
        "NO" | "0" => Some(false),
        _ => None,
    }
}

/// `30V` -> `30.0`, `3A` -> `3.0`.
fn strip_unit(s: &str) -> Option<f64> {
    s.trim().trim_end_matches(char::is_alphabetic).parse().ok()
}

/// The instrument reports its MAC hyphen-separated (`00-19-AF-00-00-01`);
/// everything else speaks colons.
fn normalise_mac(raw: &str) -> String {
    raw.trim().replace('-', ":").to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_live_identity_string() {
        let id = Identity::parse("RIGOL TECHNOLOGIES,DP832A,DP8A000000000,00.01.19").unwrap();
        assert_eq!(id.vendor, "RIGOL TECHNOLOGIES");
        assert_eq!(id.model, "DP832A");
        assert_eq!(id.serial, "DP8A000000000");
        assert_eq!(id.firmware, "00.01.19");
    }

    #[test]
    fn parses_rating_and_setpoint_from_one_reply() {
        let c = Channel::parse(1, "CH1:30V/3A,0.000,3.000").unwrap();
        assert_eq!(c.rating, Rating::new(30.0, 3.0).unwrap());
        assert_eq!(c.setpoint.volts, 0.0);
        assert_eq!(c.setpoint.amps, 3.0);
    }

    #[test]
    fn parses_the_five_volt_channel() {
        let c = Channel::parse(3, "CH3:5V/3A,0.000,3.000").unwrap();
        assert_eq!(c.rating, Rating::new(5.0, 3.0).unwrap());
    }

    #[test]
    fn parses_a_measurement_triple() {
        let m = Measurement::parse(2, "0.0000,0.0000,0.000").unwrap();
        assert_eq!((m.volts, m.amps, m.watts), (0.0, 0.0, 0.0));
    }

    /// A rating backfills the ceiling, so one that cannot be a ceiling is a
    /// parse failure, and the channel stays without one.
    #[test]
    fn a_zero_or_negative_rating_is_refused() {
        assert!(Channel::parse(1, "CH1:0V/3A,0.000,3.000").is_err());
        assert!(Channel::parse(1, "CH1:30V/-3A,0.000,3.000").is_err());
    }

    #[test]
    fn a_malformed_channel_reply_is_an_error_not_a_panic() {
        assert!(Channel::parse(1, "nonsense").is_err());
    }

    #[test]
    fn normalises_the_hyphenated_mac() {
        assert_eq!(normalise_mac("00-19-af-00-00-01"), "00:19:AF:00:00:01");
    }

    #[test]
    fn reads_the_alarm_queries_as_yes_no() {
        // Verified against firmware 00.01.19: ALAR? answers YES/NO, not ON/OFF.
        assert_eq!(parse_yes_no("NO"), Some(false));
        assert_eq!(parse_yes_no("YES"), Some(true));
        assert_eq!(parse_yes_no("ON"), None);
    }

    #[test]
    fn reads_output_state_both_ways() {
        assert_eq!(OutputState::parse("ON"), Some(OutputState::On));
        assert_eq!(OutputState::parse("off"), Some(OutputState::Off));
        assert_eq!(OutputState::parse("maybe"), None);
    }
}
