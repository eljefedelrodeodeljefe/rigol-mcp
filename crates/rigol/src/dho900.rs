//! The DHO800/DHO900-series oscilloscope model.
//!
//! Command spellings follow the RIGOL DHO800/DHO900 Programming Guide and were
//! checked one by one against a DHO924S on firmware 00.01.05 (see the probe
//! example). Two things shape this module:
//!
//! - **Parameters are closed enums, never strings.** On this firmware a query
//!   it does not recognise gets no reply at all, so the only safe command is
//!   one this module already knows the spelling of.
//! - **"No reading" is a value, not an error.** A measurement the scope cannot
//!   make, such as a frequency with no signal, comes back as `9.9E37`. This
//!   module turns that into `None` rather than a number that would dwarf every
//!   real one.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::generator::{GeneratorConfig, GeneratorLimits, Signal, Waveshape, plan};
use crate::idn::{Family, Identity};
use crate::scpi::{DEFAULT_PORT, InstrumentError, Session, SessionConfig};

/// The scope reports an unmeasurable value as 9.9E37. Anything at or above
/// this threshold is "no reading".
const INVALID_MEASUREMENT: f64 = 9.0e37;

/// Bounds one waveform block read. Screen data (1000 points) takes tens of
/// milliseconds; the margin is for a loaded network.
const WAVEFORM_TIMEOUT: Duration = Duration::from_secs(30);

/// A measurement the scope can make on a channel, in Programming Guide terms
/// (`:MEASure:ITEM? <item>,<source>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MeasureItem {
    /// Maximum voltage (VMAX).
    Vmax,
    /// Minimum voltage (VMIN).
    Vmin,
    /// Peak-to-peak voltage (VPP).
    Vpp,
    /// Top level of a pulse (VTOP).
    Vtop,
    /// Base level of a pulse (VBASe).
    Vbase,
    /// Amplitude, top minus base (VAMP).
    Vamp,
    /// Average voltage (VAVG).
    Vavg,
    /// RMS voltage (VRMS).
    Vrms,
    /// Overshoot, as a ratio (OVERshoot).
    Overshoot,
    /// Preshoot, as a ratio (PREShoot).
    Preshoot,
    /// Period in seconds (PERiod).
    Period,
    /// Frequency in hertz (FREQuency).
    Frequency,
    /// Rise time in seconds (RTIMe).
    RiseTime,
    /// Fall time in seconds (FTIMe).
    FallTime,
    /// Positive pulse width in seconds (PWIDth).
    PositiveWidth,
    /// Negative pulse width in seconds (NWIDth).
    NegativeWidth,
    /// Positive duty cycle, as a ratio (PDUTy).
    PositiveDuty,
    /// Negative duty cycle, as a ratio (NDUTy).
    NegativeDuty,
}

impl MeasureItem {
    fn scpi(self) -> &'static str {
        match self {
            Self::Vmax => "VMAX",
            Self::Vmin => "VMIN",
            Self::Vpp => "VPP",
            Self::Vtop => "VTOP",
            Self::Vbase => "VBASe",
            Self::Vamp => "VAMP",
            Self::Vavg => "VAVG",
            Self::Vrms => "VRMS",
            Self::Overshoot => "OVERshoot",
            Self::Preshoot => "PREShoot",
            Self::Period => "PERiod",
            Self::Frequency => "FREQuency",
            Self::RiseTime => "RTIMe",
            Self::FallTime => "FTIMe",
            Self::PositiveWidth => "PWIDth",
            Self::NegativeWidth => "NWIDth",
            Self::PositiveDuty => "PDUTy",
            Self::NegativeDuty => "NDUTy",
        }
    }

    /// The unit the reading is in.
    pub fn unit(self) -> &'static str {
        match self {
            Self::Vmax
            | Self::Vmin
            | Self::Vpp
            | Self::Vtop
            | Self::Vbase
            | Self::Vamp
            | Self::Vavg
            | Self::Vrms => "V",
            Self::Period
            | Self::RiseTime
            | Self::FallTime
            | Self::PositiveWidth
            | Self::NegativeWidth => "s",
            Self::Frequency => "Hz",
            Self::Overshoot | Self::Preshoot | Self::PositiveDuty | Self::NegativeDuty => "ratio",
        }
    }
}

/// Input coupling (`:CHANnel<n>:COUPling`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Coupling {
    Dc,
    Ac,
    Gnd,
}

impl Coupling {
    fn scpi(self) -> &'static str {
        match self {
            Self::Dc => "DC",
            Self::Ac => "AC",
            Self::Gnd => "GND",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "DC" => Some(Self::Dc),
            "AC" => Some(Self::Ac),
            "GND" => Some(Self::Gnd),
            _ => None,
        }
    }
}

/// Which edge the trigger fires on (`:TRIGger:EDGE:SLOPe`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Slope {
    Rising,
    Falling,
    Either,
}

impl Slope {
    fn scpi(self) -> &'static str {
        match self {
            Self::Rising => "POSitive",
            Self::Falling => "NEGative",
            Self::Either => "RFALl",
        }
    }
}

/// When the scope acquires (`:TRIGger:SWEep`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Sweep {
    /// Free-runs when no trigger arrives.
    Auto,
    /// Acquires only on a trigger.
    Normal,
    /// Acquires once on the next trigger, then stops.
    Single,
}

impl Sweep {
    fn scpi(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::Normal => "NORMal",
            Self::Single => "SINGle",
        }
    }
}

/// Probe attenuation ratios the scope accepts (`:CHANnel<n>:PROBe`).
const PROBE_RATIOS: &[f64] = &[
    0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0,
    200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0, 50000.0,
];

/// Changes to one channel's vertical setup. `None` leaves a setting alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub displayed: Option<bool>,
    pub volts_per_div: Option<f64>,
    pub offset_volts: Option<f64>,
    pub coupling: Option<Coupling>,
    pub probe_ratio: Option<f64>,
}

/// Changes to the edge trigger. `None` leaves a setting alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EdgeTriggerConfig {
    /// Analog channel the trigger watches.
    pub channel: Option<u8>,
    pub slope: Option<Slope>,
    pub level_volts: Option<f64>,
    pub sweep: Option<Sweep>,
}

/// The state an instrument holds after a write, and anything it objected to.
///
/// Read back rather than echoed: where a limit depends on other settings
/// (an offset on the scale, a timebase on the bandwidth), the instrument
/// decides, and this reports what it decided.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Applied<T> {
    pub state: T,
    /// Errors the instrument raised while applying the change. Empty when it
    /// accepted everything.
    pub instrument_errors: Vec<InstrumentError>,
}

/// Timebase after a change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Timebase {
    pub seconds_per_div: f64,
    pub offset_seconds: f64,
}

/// Edge trigger after a change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EdgeTrigger {
    pub mode: String,
    pub source: String,
    pub slope: String,
    pub level_volts: f64,
    pub sweep: String,
}

/// The signal generator as the scope reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GeneratorState {
    pub output_on: bool,
    /// `None` for an arbitrary waveform, which this crate does not configure.
    pub shape: Option<Waveshape>,
    pub frequency_hz: f64,
    pub amplitude_vpp: f64,
    pub offset_volts: f64,
    /// Furthest excursion from 0 V the current settings produce.
    pub peak_volts: Option<f64>,
    /// The ceilings any change or output-on is checked against.
    pub limits: GeneratorLimits,
}

/// One analog channel's vertical setup.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelState {
    pub channel: u8,
    pub displayed: bool,
    /// Vertical scale in volts per division.
    pub volts_per_div: f64,
    /// Vertical offset in volts.
    pub offset_volts: f64,
    pub coupling: Option<Coupling>,
    /// Probe attenuation ratio (1 for 1×, 10 for 10×).
    pub probe_ratio: f64,
    pub bandwidth_limit: String,
    pub inverted: bool,
}

/// Trigger, timebase and acquisition state, plus every channel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeStatus {
    /// `:TRIGger:STATus?` — TD (triggered), WAIT, RUN, AUTO or STOP.
    pub trigger_status: String,
    pub trigger_mode: String,
    pub trigger_sweep: String,
    pub edge_source: String,
    pub edge_slope: String,
    pub edge_level_volts: f64,
    pub seconds_per_div: f64,
    pub timebase_offset_seconds: f64,
    pub sample_rate: f64,
    pub memory_depth: f64,
    pub acquisition_type: String,
    pub channels: Vec<ChannelState>,
}

/// The LAN subsystem as the scope reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeLan {
    /// CONNECTED or UNLINK.
    pub status: String,
    pub dhcp: bool,
    pub ip: String,
    pub mask: String,
    pub gateway: String,
    /// Normalised to upper-case and colon-separated; this firmware answers
    /// upper-case over LAN and lower-case over USB.
    pub mac: String,
}

/// One measurement result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reading {
    pub item: MeasureItem,
    pub channel: u8,
    /// `None` when the scope cannot make this measurement on the current
    /// signal, e.g. a frequency with no edges on screen.
    pub value: Option<f64>,
    pub unit: &'static str,
}

/// `:WAVeform:PREamble?` — how raw samples map to volts and seconds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Preamble {
    /// 0 = BYTE, 1 = WORD, 2 = ASCii.
    pub format: u8,
    pub points: usize,
    pub x_increment: f64,
    pub x_origin: f64,
    pub x_reference: f64,
    pub y_increment: f64,
    pub y_origin: f64,
    pub y_reference: f64,
}

impl Preamble {
    pub fn parse(raw: &str) -> Option<Self> {
        let f: Vec<f64> = raw
            .split(',')
            .map(|s| s.trim().parse::<f64>())
            .collect::<std::result::Result<_, _>>()
            .ok()?;
        if f.len() < 10 {
            return None;
        }
        Some(Self {
            format: f[0] as u8,
            points: f[2] as usize,
            x_increment: f[4],
            x_origin: f[5],
            x_reference: f[6],
            y_increment: f[7],
            y_origin: f[8],
            y_reference: f[9],
        })
    }

    /// Volts for one raw sample. Verified on a DHO924S: a grounded input reads
    /// back as a mean within a fraction of a millivolt of zero.
    pub fn volts(&self, raw: u16) -> f64 {
        (f64::from(raw) - self.y_origin - self.y_reference) * self.y_increment
    }

    /// Seconds, relative to the trigger, for sample `i`.
    pub fn seconds(&self, i: usize) -> f64 {
        self.x_origin + (i as f64 - self.x_reference) * self.x_increment
    }
}

/// Statistics the scope keeps for one measurement item since it was added or
/// last reset (`:MEASure:STATistic:ITEM? <type>,<item>,<source>`).
///
/// A value is `None` where the scope has no reading for it, as for
/// [`Reading::value`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeasureStatistics {
    pub item: MeasureItem,
    pub channel: u8,
    pub unit: &'static str,
    /// The latest reading (CURRent).
    pub current: Option<f64>,
    /// Smallest reading so far (MINimum).
    pub minimum: Option<f64>,
    /// Largest reading so far (MAXimum).
    pub maximum: Option<f64>,
    /// Mean of the readings so far (AVERages).
    pub average: Option<f64>,
    /// Standard deviation of the readings so far (DEViation).
    pub deviation: Option<f64>,
    /// How many readings the statistics cover (CNT).
    pub count: u64,
}

/// A captured trace in volts against seconds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Waveform {
    pub channel: u8,
    pub preamble: Preamble,
    pub volts: Vec<f64>,
}

/// Statistics over every sample of a capture, however it is later thinned.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WaveformStats {
    pub points: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub rms: f64,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// One bucket of a decimated waveform: the extremes within it, so a narrow
/// spike survives thinning rather than falling between kept samples.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bucket {
    pub seconds: f64,
    pub min: f64,
    pub max: f64,
}

impl Waveform {
    /// Decodes a `:WAVeform:DATA?` payload. BYTE samples are one unsigned
    /// byte; WORD samples are two, little-endian (checked against the
    /// alternative, which reads a grounded input as tens of millivolts).
    pub fn decode(channel: u8, preamble: Preamble, data: &[u8]) -> Result<Self> {
        let raw: Vec<u16> = match preamble.format {
            0 => data.iter().map(|b| u16::from(*b)).collect(),
            1 => data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|w| u16::from_le_bytes(*w))
                .collect(),
            other => {
                return Err(Error::Protocol {
                    cmd: ":WAVeform:DATA?".into(),
                    detail: format!("unsupported waveform format {other}"),
                });
            }
        };
        let volts = raw.iter().map(|r| preamble.volts(*r)).collect();
        Ok(Self {
            channel,
            preamble,
            volts,
        })
    }

    pub fn stats(&self) -> Option<WaveformStats> {
        let n = self.volts.len();
        if n == 0 {
            return None;
        }
        let (mut min, mut max, mut sum, mut sq) = (f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.0);
        for v in &self.volts {
            min = min.min(*v);
            max = max.max(*v);
            sum += v;
            sq += v * v;
        }
        Some(WaveformStats {
            points: n,
            min,
            max,
            mean: sum / n as f64,
            rms: (sq / n as f64).sqrt(),
            start_seconds: self.preamble.seconds(0),
            end_seconds: self.preamble.seconds(n - 1),
        })
    }

    /// Thins the trace to at most `max_buckets` min/max buckets. A capture
    /// already within the limit comes back one bucket per sample.
    pub fn decimate(&self, max_buckets: usize) -> Vec<Bucket> {
        let n = self.volts.len();
        if n == 0 || max_buckets == 0 {
            return Vec::new();
        }
        let per = n.div_ceil(max_buckets);
        self.volts
            .chunks(per)
            .enumerate()
            .map(|(b, chunk)| Bucket {
                seconds: self.preamble.seconds(b * per),
                min: chunk.iter().copied().fold(f64::INFINITY, f64::min),
                max: chunk.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            })
            .collect()
    }
}

pub struct Dho900 {
    session: Session,
    identity: Identity,
    channels: u8,
    generator_limits: GeneratorLimits,
}

impl Dho900 {
    /// Connects and reads identity. Nothing DHO-specific is sent until the
    /// instrument has identified itself as one.
    pub async fn connect(host: &str, port: Option<u16>, cfg: SessionConfig) -> Result<Self> {
        let addr = format!("{host}:{}", port.unwrap_or(DEFAULT_PORT));
        let session = Session::connect(addr, cfg).await?;

        let raw = session.query("*IDN?").await?;
        let identity = Identity::parse(&raw).ok_or_else(|| Error::Parse {
            cmd: "*IDN?".into(),
            reply: raw.clone(),
        })?;
        if identity.family() != Some(Family::Dho900) {
            return Err(Error::WrongInstrument {
                expected: Family::Dho900,
                model: identity.model,
            });
        }
        let channels = channel_count(&identity.model);

        tracing::info!(
            model = %identity.model,
            serial = %identity.serial,
            firmware = %identity.firmware,
            channels,
            "connected"
        );
        Ok(Self {
            session,
            identity,
            channels,
            generator_limits: GeneratorLimits::default(),
        })
    }

    /// Tightens the signal generator's ceilings below the hardware range.
    pub fn with_generator_limits(mut self, limits: GeneratorLimits) -> Self {
        self.generator_limits = limits;
        self
    }

    pub fn generator_limits(&self) -> GeneratorLimits {
        self.generator_limits
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Analog channels on this model.
    pub fn channel_count(&self) -> u8 {
        self.channels
    }

    /// The raw session, for diagnostics.
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub async fn errors(&self) -> Result<Vec<InstrumentError>> {
        self.session.errors().await
    }

    pub async fn lan_config(&self) -> Result<ScopeLan> {
        Ok(ScopeLan {
            status: self.text(":LAN:STATus?").await?,
            dhcp: self.flag(":LAN:DHCP?").await?,
            ip: self.text(":LAN:IPADdress?").await?,
            mask: self.text(":LAN:SMASk?").await?,
            gateway: self.text(":LAN:GATeway?").await?,
            mac: self
                .text(":LAN:MAC?")
                .await?
                .to_ascii_uppercase()
                .replace('-', ":"),
        })
    }

    pub async fn channel(&self, channel: u8) -> Result<ChannelState> {
        self.check_channel(channel)?;
        let c = format!(":CHANnel{channel}");
        Ok(ChannelState {
            channel,
            displayed: self.flag(&format!("{c}:DISPlay?")).await?,
            volts_per_div: self.number(&format!("{c}:SCALe?")).await?,
            offset_volts: self.number(&format!("{c}:OFFSet?")).await?,
            coupling: Coupling::parse(&self.text(&format!("{c}:COUPling?")).await?),
            probe_ratio: self.number(&format!("{c}:PROBe?")).await?,
            bandwidth_limit: self.text(&format!("{c}:BWLimit?")).await?,
            inverted: self.flag(&format!("{c}:INVert?")).await?,
        })
    }

    pub async fn status(&self) -> Result<ScopeStatus> {
        let mut channels = Vec::new();
        for ch in 1..=self.channels {
            channels.push(self.channel(ch).await?);
        }
        Ok(ScopeStatus {
            trigger_status: self.text(":TRIGger:STATus?").await?,
            trigger_mode: self.text(":TRIGger:MODE?").await?,
            trigger_sweep: self.text(":TRIGger:SWEep?").await?,
            edge_source: self.text(":TRIGger:EDGE:SOURce?").await?,
            edge_slope: self.text(":TRIGger:EDGE:SLOPe?").await?,
            edge_level_volts: self.number(":TRIGger:EDGE:LEVel?").await?,
            seconds_per_div: self.number(":TIMebase:MAIN:SCALe?").await?,
            timebase_offset_seconds: self.number(":TIMebase:MAIN:OFFSet?").await?,
            sample_rate: self.number(":ACQuire:SRATe?").await?,
            memory_depth: self.number(":ACQuire:MDEPth?").await?,
            acquisition_type: self.text(":ACQuire:TYPE?").await?,
            channels,
        })
    }

    /// One automatic measurement.
    ///
    /// Not a pure read on this firmware: like choosing the item on the front
    /// panel, the query also adds it to the scope's on-screen measurement
    /// list. The Programming Guide does not say so; a screenshot does.
    pub async fn measure(&self, item: MeasureItem, channel: u8) -> Result<Reading> {
        self.check_channel(channel)?;
        let cmd = format!(":MEASure:ITEM? {},CHANnel{channel}", item.scpi());
        let value = parse_number(&cmd, &self.session.query(&cmd).await?)?;
        Ok(Reading {
            item,
            channel,
            value: (value.abs() < INVALID_MEASUREMENT).then_some(value),
            unit: item.unit(),
        })
    }

    /// The statistics the scope holds for one measurement item.
    ///
    /// Like [`Dho900::measure`], not a pure read: an item that is not yet in
    /// the on-screen list is added to it, and its statistics start from that
    /// reading. Verified on a DHO924S (00.01.05): the first query of a new
    /// item reports a count of 1.
    pub async fn measure_statistics(
        &self,
        item: MeasureItem,
        channel: u8,
    ) -> Result<MeasureStatistics> {
        self.check_channel(channel)?;
        let stat = |kind: &'static str| {
            let cmd = format!(
                ":MEASure:STATistic:ITEM? {kind},{},CHANnel{channel}",
                item.scpi()
            );
            async move {
                let value = parse_number(&cmd, &self.session.query(&cmd).await?)?;
                Ok::<_, Error>((value.abs() < INVALID_MEASUREMENT).then_some(value))
            }
        };
        let current = stat("CURRent").await?;
        let minimum = stat("MINimum").await?;
        let maximum = stat("MAXimum").await?;
        let average = stat("AVERages").await?;
        let deviation = stat("DEViation").await?;
        // The count comes back in scientific notation too: 5.4800E+02.
        let count = stat("CNT").await?.map_or(0, |n| n.max(0.0).round() as u64);
        Ok(MeasureStatistics {
            item,
            channel,
            unit: item.unit(),
            current,
            minimum,
            maximum,
            average,
            deviation,
            count,
        })
    }

    /// The screen as a PNG (`:DISPlay:DATA? PNG`).
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.session.query_block(":DISPlay:DATA? PNG").await
    }

    /// The on-screen trace of one channel.
    ///
    /// Selects the channel as the waveform source and 16-bit samples first.
    /// Those settings only steer what `:WAVeform:DATA?` returns; they do not
    /// touch acquisition, triggering or anything on screen.
    pub async fn waveform(&self, channel: u8) -> Result<Waveform> {
        self.check_channel(channel)?;
        self.session
            .command(&format!(":WAVeform:SOURce CHANnel{channel}"))
            .await?;
        self.session.command(":WAVeform:MODE NORMal").await?;
        self.session.command(":WAVeform:FORMat WORD").await?;

        let cmd = ":WAVeform:PREamble?";
        let raw = self.session.query(cmd).await?;
        let preamble = Preamble::parse(&raw).ok_or_else(|| Error::Parse {
            cmd: cmd.into(),
            reply: raw.clone(),
        })?;
        let data = self
            .session
            .query_block_with_timeout(":WAVeform:DATA?", WAVEFORM_TIMEOUT)
            .await?;
        Waveform::decode(channel, preamble, &data)
    }

    // ── Writes ───────────────────────────────────────────────────────────────
    //
    // None of these reach the device under test: they change what the scope
    // shows and when it acquires. The signal generator, which does drive the
    // device under test, is gated separately.

    /// Starts acquiring (`:RUN`).
    pub async fn run(&self) -> Result<()> {
        self.session.command(":RUN").await
    }

    /// Stops acquiring (`:STOP`).
    pub async fn stop(&self) -> Result<()> {
        self.session.command(":STOP").await
    }

    /// Arms a single acquisition on the next trigger (`:SINGle`).
    pub async fn single(&self) -> Result<()> {
        self.session.command(":SINGle").await
    }

    /// Lets the scope pick scale, timebase and trigger for the signal
    /// (`:AUToset`).
    pub async fn autoset(&self) -> Result<()> {
        self.session.command(":AUToset").await
    }

    /// Removes every measurement from the on-screen list
    /// (`:MEASure:CLEar`), including ones added by [`Dho900::measure`].
    pub async fn clear_measurements(&self) -> Result<()> {
        self.session.command(":MEASure:CLEar").await
    }

    /// Clears the statistics history of every measurement item and starts
    /// counting again (`:MEASure:STATistic:RESet`). The items stay in the
    /// on-screen list. Returns anything the scope objected to.
    pub async fn reset_measurement_statistics(&self) -> Result<Vec<InstrumentError>> {
        self.session.clear_status().await?;
        self.session.command(":MEASure:STATistic:RESet").await?;
        self.errors().await
    }

    pub async fn configure_channel(
        &self,
        channel: u8,
        cfg: &ChannelConfig,
    ) -> Result<Applied<ChannelState>> {
        self.check_channel(channel)?;
        if let Some(p) = cfg.probe_ratio
            && !PROBE_RATIOS.contains(&p)
        {
            return Err(Error::OutOfRange {
                what: "probe ratio",
                value: p,
                allowed: "one of 0.001, 0.002, 0.005 … 1, 2, 5, 10 … 50000".into(),
            });
        }
        if let Some(scale) = cfg.volts_per_div {
            let probe = match cfg.probe_ratio {
                Some(p) => p,
                None => self.number(&format!(":CHANnel{channel}:PROBe?")).await?,
            };
            let (min, max) = (self.min_volts_per_div() * probe, 10.0 * probe);
            if !(min..=max).contains(&scale) {
                return Err(Error::OutOfRange {
                    what: "vertical scale (V/div)",
                    value: scale,
                    allowed: format!("{min} to {max} at a {probe}x probe ratio"),
                });
            }
        }
        check_finite("offset (V)", cfg.offset_volts)?;

        let c = format!(":CHANnel{channel}");
        self.session.clear_status().await?;
        // Probe first: it changes the meaning of scale and offset.
        if let Some(p) = cfg.probe_ratio {
            self.session.command(&format!("{c}:PROBe {p}")).await?;
        }
        if let Some(on) = cfg.displayed {
            self.session
                .command(&format!("{c}:DISPlay {}", u8::from(on)))
                .await?;
        }
        if let Some(cp) = cfg.coupling {
            self.session
                .command(&format!("{c}:COUPling {}", cp.scpi()))
                .await?;
        }
        if let Some(v) = cfg.volts_per_div {
            self.session.command(&format!("{c}:SCALe {v}")).await?;
        }
        if let Some(v) = cfg.offset_volts {
            self.session.command(&format!("{c}:OFFSet {v}")).await?;
        }
        Ok(Applied {
            state: self.channel(channel).await?,
            instrument_errors: self.errors().await?,
        })
    }

    pub async fn configure_timebase(
        &self,
        seconds_per_div: Option<f64>,
        offset_seconds: Option<f64>,
    ) -> Result<Applied<Timebase>> {
        if let Some(s) = seconds_per_div
            && !(s.is_finite() && s > 0.0)
        {
            return Err(Error::OutOfRange {
                what: "timebase scale (s/div)",
                value: s,
                allowed: "a positive number".into(),
            });
        }
        check_finite("timebase offset (s)", offset_seconds)?;

        self.session.clear_status().await?;
        if let Some(s) = seconds_per_div {
            self.session
                .command(&format!(":TIMebase:MAIN:SCALe {s}"))
                .await?;
        }
        if let Some(o) = offset_seconds {
            self.session
                .command(&format!(":TIMebase:MAIN:OFFSet {o}"))
                .await?;
        }
        Ok(Applied {
            state: Timebase {
                seconds_per_div: self.number(":TIMebase:MAIN:SCALe?").await?,
                offset_seconds: self.number(":TIMebase:MAIN:OFFSet?").await?,
            },
            instrument_errors: self.errors().await?,
        })
    }

    /// Switches the trigger to edge mode and applies the given settings.
    pub async fn configure_edge_trigger(
        &self,
        cfg: &EdgeTriggerConfig,
    ) -> Result<Applied<EdgeTrigger>> {
        if let Some(ch) = cfg.channel {
            self.check_channel(ch)?;
        }
        check_finite("trigger level (V)", cfg.level_volts)?;

        self.session.clear_status().await?;
        self.session.command(":TRIGger:MODE EDGE").await?;
        if let Some(ch) = cfg.channel {
            self.session
                .command(&format!(":TRIGger:EDGE:SOURce CHANnel{ch}"))
                .await?;
        }
        if let Some(s) = cfg.slope {
            self.session
                .command(&format!(":TRIGger:EDGE:SLOPe {}", s.scpi()))
                .await?;
        }
        if let Some(l) = cfg.level_volts {
            self.session
                .command(&format!(":TRIGger:EDGE:LEVel {l}"))
                .await?;
        }
        if let Some(s) = cfg.sweep {
            self.session
                .command(&format!(":TRIGger:SWEep {}", s.scpi()))
                .await?;
        }
        Ok(Applied {
            state: EdgeTrigger {
                mode: self.text(":TRIGger:MODE?").await?,
                source: self.text(":TRIGger:EDGE:SOURce?").await?,
                slope: self.text(":TRIGger:EDGE:SLOPe?").await?,
                level_volts: self.number(":TRIGger:EDGE:LEVel?").await?,
                sweep: self.text(":TRIGger:SWEep?").await?,
            },
            instrument_errors: self.errors().await?,
        })
    }

    // ── Signal generator ─────────────────────────────────────────────────────
    //
    // Unlike everything above, this reaches the device under test. See the
    // generator module for the rules.

    pub async fn generator(&self) -> Result<GeneratorState> {
        let shape = Waveshape::parse(&self.text(":SOURce:FUNCtion?").await?);
        let frequency_hz = self.number(":SOURce:FREQuency?").await?;
        let amplitude_vpp = self.number(":SOURce:VOLTage:AMPLitude?").await?;
        let offset_volts = self.number(":SOURce:VOLTage:OFFSet?").await?;
        let peak_volts = shape.map(|shape| {
            Signal {
                shape,
                frequency_hz,
                amplitude_vpp,
                offset_volts,
            }
            .peak_volts()
        });
        Ok(GeneratorState {
            output_on: self.flag(":SOURce:OUTPut:STATe?").await?,
            shape,
            frequency_hz,
            amplitude_vpp,
            offset_volts,
            peak_volts,
            limits: self.generator_limits,
        })
    }

    /// Changes waveform, frequency, amplitude and/or offset. Never turns the
    /// output on. The resulting signal is checked against the ceilings
    /// before anything is sent, and the commands go out in an order that
    /// keeps every intermediate state inside them too.
    pub async fn configure_generator(
        &self,
        cfg: &GeneratorConfig,
    ) -> Result<Applied<GeneratorState>> {
        let current = self.current_signal().await?;
        let target = cfg.apply_to(&current);
        let changes = plan(&current, &target, &self.generator_limits)?;

        self.session.clear_status().await?;
        for change in &changes {
            self.session.command(&change.scpi()).await?;
        }
        Ok(Applied {
            state: self.generator().await?,
            instrument_errors: self.errors().await?,
        })
    }

    /// Turns the output on, after checking the settings it would put out.
    /// They may have changed on the front panel since they were configured.
    pub async fn generator_output_on(&self) -> Result<GeneratorState> {
        let current = self.current_signal().await?;
        self.generator_limits.check(&current)?;
        self.session.command(":SOURce:OUTPut:STATe ON").await?;
        let state = self.generator().await?;
        tracing::info!(
            shape = ?state.shape,
            frequency_hz = state.frequency_hz,
            amplitude_vpp = state.amplitude_vpp,
            offset_volts = state.offset_volts,
            peak_volts = ?state.peak_volts,
            "signal generator output ON"
        );
        Ok(state)
    }

    pub async fn generator_output_off(&self) -> Result<GeneratorState> {
        self.session.command(":SOURce:OUTPut:STATe OFF").await?;
        tracing::info!("signal generator output off");
        self.generator().await
    }

    /// The generator's signal now. An arbitrary waveform cannot be checked
    /// against the ceilings, so it is refused rather than guessed at.
    async fn current_signal(&self) -> Result<Signal> {
        let cmd = ":SOURce:FUNCtion?";
        let raw = self.text(cmd).await?;
        let shape = Waveshape::parse(&raw).ok_or_else(|| Error::Parse {
            cmd: cmd.into(),
            reply: raw,
        })?;
        Ok(Signal {
            shape,
            frequency_hz: self.number(":SOURce:FREQuency?").await?,
            amplitude_vpp: self.number(":SOURce:VOLTage:AMPLitude?").await?,
            offset_volts: self.number(":SOURce:VOLTage:OFFSet?").await?,
        })
    }

    /// Smallest vertical scale at a 1x probe: 200 µV/div on the DHO900
    /// series, 500 µV/div on the DHO800 (Programming Guide, `:CHANnel<n>:SCALe`).
    fn min_volts_per_div(&self) -> f64 {
        if self.identity.model.to_ascii_uppercase().starts_with("DHO9") {
            200e-6
        } else {
            500e-6
        }
    }

    fn check_channel(&self, channel: u8) -> Result<()> {
        if (1..=self.channels).contains(&channel) {
            Ok(())
        } else {
            Err(Error::NoSuchInput {
                channel,
                count: self.channels,
            })
        }
    }

    async fn text(&self, cmd: &str) -> Result<String> {
        self.session.query(cmd).await
    }

    async fn number(&self, cmd: &str) -> Result<f64> {
        let raw = self.session.query(cmd).await?;
        parse_number(cmd, &raw)
    }

    async fn flag(&self, cmd: &str) -> Result<bool> {
        let raw = self.session.query(cmd).await?;
        match raw.trim().to_ascii_uppercase().as_str() {
            "1" | "ON" => Ok(true),
            "0" | "OFF" => Ok(false),
            _ => Err(Error::Parse {
                cmd: cmd.into(),
                reply: raw,
            }),
        }
    }
}

fn check_finite(what: &'static str, value: Option<f64>) -> Result<()> {
    match value {
        Some(v) if !v.is_finite() => Err(Error::OutOfRange {
            what,
            value: v,
            allowed: "a finite number".into(),
        }),
        _ => Ok(()),
    }
}

fn parse_number(cmd: &str, raw: &str) -> Result<f64> {
    raw.trim().parse().map_err(|_| Error::Parse {
        cmd: cmd.into(),
        reply: raw.into(),
    })
}

/// Analog channels from the model name: the last digit of the series number
/// (DHO802 → 2, DHO924S → 4). Falls back to 2, the smallest in the series.
fn channel_count(model: &str) -> u8 {
    model
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .chars()
        .last()
        .and_then(|c| c.to_digit(10))
        .and_then(|d| u8::try_from(d).ok())
        .filter(|d| (1..=4).contains(d))
        .unwrap_or(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a DHO924S (00.01.05), channel 1, grounded input.
    const PREAMBLE: &str = "1,0,1000,1,5.000000E-7,-2.500000E-4,0.000000,6.6667E-06,-7800,32768";

    #[test]
    fn counts_channels_from_the_model() {
        assert_eq!(channel_count("DHO924S"), 4);
        assert_eq!(channel_count("DHO914"), 4);
        assert_eq!(channel_count("DHO802"), 2);
        assert_eq!(channel_count("DHO812"), 2);
    }

    #[test]
    fn parses_a_real_preamble() {
        let p = Preamble::parse(PREAMBLE).unwrap();
        assert_eq!(p.format, 1);
        assert_eq!(p.points, 1000);
        assert_eq!(p.x_increment, 5.0e-7);
        assert_eq!(p.y_origin, -7800.0);
        assert_eq!(p.y_reference, 32768.0);
    }

    #[test]
    fn scales_samples_to_volts_and_seconds() {
        let p = Preamble::parse(PREAMBLE).unwrap();
        // The raw code for 0 V on this setup: yreference + yorigin.
        assert!(p.volts(24968).abs() < 1e-9);
        assert!((p.volts(24968 + 150) - 150.0 * 6.6667e-6).abs() < 1e-9);
        assert_eq!(p.seconds(0), -2.5e-4);
        assert!((p.seconds(1000) - 2.5e-4).abs() < 1e-12);
    }

    #[test]
    fn decodes_word_samples_little_endian() {
        let p = Preamble::parse(PREAMBLE).unwrap();
        let data = [0x88, 0x61, 0x88, 0x61]; // 24968 = 0x6188, little-endian
        let w = Waveform::decode(1, p, &data).unwrap();
        assert_eq!(w.volts.len(), 2);
        assert!(w.volts.iter().all(|v| v.abs() < 1e-9), "{:?}", w.volts);
    }

    #[test]
    fn refuses_ascii_waveforms() {
        let mut p = Preamble::parse(PREAMBLE).unwrap();
        p.format = 2;
        assert!(Waveform::decode(1, p, b"1,2,3").is_err());
    }

    fn ramp(n: usize) -> Waveform {
        let p = Preamble::parse(PREAMBLE).unwrap();
        Waveform {
            channel: 1,
            preamble: p,
            volts: (0..n).map(|i| i as f64).collect(),
        }
    }

    #[test]
    fn stats_cover_every_sample() {
        let s = ramp(5).stats().unwrap();
        assert_eq!((s.points, s.min, s.max, s.mean), (5, 0.0, 4.0, 2.0));
        assert!((s.rms - 6.0f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn decimation_keeps_a_one_sample_spike() {
        let mut w = ramp(1000);
        w.volts.iter_mut().for_each(|v| *v = 0.0);
        w.volts[537] = 5.0;
        let buckets = w.decimate(10);
        assert_eq!(buckets.len(), 10);
        assert_eq!(buckets.iter().map(|b| b.max).fold(0.0, f64::max), 5.0);
    }

    #[test]
    fn decimation_below_the_limit_is_one_bucket_per_sample() {
        assert_eq!(ramp(7).decimate(100).len(), 7);
        assert!(ramp(7).decimate(0).is_empty());
    }

    #[test]
    fn maps_every_measure_item_to_a_unit() {
        assert_eq!(MeasureItem::Vpp.unit(), "V");
        assert_eq!(MeasureItem::Frequency.unit(), "Hz");
        assert_eq!(MeasureItem::RiseTime.unit(), "s");
        assert_eq!(MeasureItem::PositiveDuty.unit(), "ratio");
    }
}
