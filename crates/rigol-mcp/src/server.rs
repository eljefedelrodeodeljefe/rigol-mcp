use anyhow::Context;
use base64::Engine;
use rigol::{Dho900, Dp800, Family, GeneratorLimits, Limits, SessionConfig};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ErrorCode, ErrorData, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use serde_json::Value;
use std::env::VarError;
use std::time::Duration;
use tokio::sync::OnceCell;

use crate::error::tool_error;
use crate::params::*;
use crate::tools;

pub struct RigolServer {
    /// Which instrument family this process drives. One family per process:
    /// a PSU and a scope run as two servers, each with its own write gate.
    family: Family,
    /// The tools for `family`, and only those.
    tool_router: ToolRouter<Self>,
    host: String,
    port: Option<u16>,
    cfg: SessionConfig,
    configured_limits: Limits,
    /// Ceilings for the scope's signal generator, one per generator channel
    /// (a DHO has one, an MHO900 two). The hardware range unless tightened
    /// with `RIGOL_GEN_MAX_VPP` / `RIGOL_GEN_MAX_PEAK_VOLTS`, or per MHO900
    /// channel with `RIGOL_GEN<n>_MAX_VPP` / `RIGOL_GEN<n>_MAX_PEAK_VOLTS`.
    generator_limits: [GeneratorLimits; 2],
    writes_enabled: bool,
    /// Connected on first use. Two concurrent sessions were verified to coexist
    /// on this instrument, and a session survives idle, so holding one open is
    /// safe.
    psu: OnceCell<Dp800>,
    /// The oscilloscope, for a `dho900` or `mho900` process. Connected on
    /// first use.
    scope: OnceCell<Dho900>,
}

impl RigolServer {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|key| std::env::var(key))
    }

    /// Builds the server from configuration read through `lookup`, so tests
    /// can supply it without touching the process environment.
    fn from_lookup(lookup: impl Fn(&str) -> Result<String, VarError>) -> anyhow::Result<Self> {
        let var = |key: &str| env_var(key, lookup(key));
        let host = var("RIGOL_HOST")?.context("RIGOL_HOST not set")?;
        // Defaults to dp800 so configurations written before scope support
        // keep meaning what they meant.
        let family: Family = match var("RIGOL_FAMILY")? {
            Some(f) => f.parse().map_err(anyhow::Error::msg)?,
            None => Family::Dp800,
        };
        let port = match var("RIGOL_PORT")? {
            Some(p) => Some(
                p.parse()
                    .context("RIGOL_PORT must be a valid port number")?,
            ),
            None => None,
        };
        let timeout_ms: u64 = var("RIGOL_TIMEOUT_MS")?
            .unwrap_or_else(|| "5000".into())
            .parse()
            .context("RIGOL_TIMEOUT_MS must be a whole number of milliseconds")?;
        let writes_enabled =
            parse_bool_env("RIGOL_ALLOW_WRITES", var("RIGOL_ALLOW_WRITES")?)?.unwrap_or(false);
        let f64_var = |key: &str| env_f64(key, var(key)?);

        // Explicit ceilings override the channel's rating. Anything not set here
        // is backfilled from the instrument's own rating at connect time.
        let mut configured_limits = Limits::new();
        for ch in 1..=3u8 {
            let volts = f64_var(&format!("RIGOL_CH{ch}_MAX_VOLTS"))?;
            let amps = f64_var(&format!("RIGOL_CH{ch}_MAX_AMPS"))?;
            if let (Some(v), Some(a)) = (volts, amps) {
                configured_limits = configured_limits
                    .with_channel(ch, v, a)
                    .with_context(|| format!("RIGOL_CH{ch}_MAX_VOLTS / RIGOL_CH{ch}_MAX_AMPS"))?;
            } else if volts.is_some() || amps.is_some() {
                anyhow::bail!(
                    "RIGOL_CH{ch}_MAX_VOLTS and RIGOL_CH{ch}_MAX_AMPS must be set together"
                );
            }
        }

        let generator_limits = generator_limits_from(family, &f64_var)?;

        Ok(Self {
            family,
            tool_router: Self::router_for(family),
            generator_limits,
            host,
            port,
            cfg: SessionConfig {
                timeout: Duration::from_millis(timeout_ms),
                ..SessionConfig::default()
            },
            configured_limits,
            writes_enabled,
            psu: OnceCell::new(),
            scope: OnceCell::new(),
        })
    }

    async fn psu(&self) -> Result<&Dp800, ErrorData> {
        self.psu
            .get_or_try_init(|| async {
                Dp800::connect(
                    &self.host,
                    self.port,
                    self.cfg.clone(),
                    self.configured_limits.clone(),
                )
                .await
            })
            .await
            .map_err(|e| tool_error(e.into()))
    }

    #[cfg(test)]
    fn for_test(family: Family, addr: std::net::SocketAddr) -> Self {
        Self {
            family,
            tool_router: Self::router_for(family),
            host: addr.ip().to_string(),
            port: Some(addr.port()),
            cfg: SessionConfig::default(),
            configured_limits: Limits::new(),
            generator_limits: [GeneratorLimits::default(); 2],
            writes_enabled: false,
            psu: OnceCell::new(),
            scope: OnceCell::new(),
        }
    }

    /// The tool set for a family. Announced at startup, before the instrument
    /// is reachable, which is why the family comes from configuration and the
    /// instrument's own `*IDN?` only confirms it.
    fn router_for(family: Family) -> ToolRouter<Self> {
        match family {
            Family::Dp800 => Self::psu_router(),
            Family::Dho900 => Self::scope_router() + Self::dho_scope_router(),
            Family::Mho900 => Self::scope_router() + Self::mho_scope_router(),
        }
    }

    async fn scope(&self) -> Result<&Dho900, ErrorData> {
        self.scope
            .get_or_try_init(|| async {
                let [first, second] = self.generator_limits;
                let scope =
                    Dho900::connect_as(&self.host, self.port, self.cfg.clone(), self.family)
                        .await?
                        .with_generator_limits(first);
                if scope.generator_channels() == 2 {
                    scope.with_generator_channel_limits(2, second)
                } else {
                    Ok(scope)
                }
            })
            .await
            .map_err(|e| tool_error(e.into()))
    }

    fn ok(value: &Value) -> CallToolResult {
        CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        )])
    }

    fn guard_write(&self) -> Result<(), ErrorData> {
        if self.writes_enabled {
            Ok(())
        } else {
            Err(ErrorData::new(
                ErrorCode::INVALID_REQUEST,
                "write operations are disabled — set RIGOL_ALLOW_WRITES=true to enable",
                None,
            ))
        }
    }
}

/// A variable's value, `None` only when it is not set. A value that is not
/// valid UTF-8 is an error naming the variable, not a silent default.
fn env_var(key: &str, value: Result<String, VarError>) -> anyhow::Result<Option<String>> {
    match value {
        Ok(v) => Ok(Some(v)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => anyhow::bail!("{key} is not valid UTF-8"),
    }
}

/// A boolean from an explicit allowlist, in any case. Anything else is an
/// error naming the variable: `RIGOL_ALLOW_WRITES=on` meaning "off" is the
/// kind of surprise a write gate must not have.
fn parse_bool_env(key: &str, value: Option<String>) -> anyhow::Result<Option<bool>> {
    let Some(v) = value else {
        return Ok(None);
    };
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(Some(true)),
        "false" | "0" | "no" => Ok(Some(false)),
        _ => anyhow::bail!("{key} must be one of true/false, 1/0, yes/no; got {v:?}"),
    }
}

/// Generator ceilings for each generator channel. `RIGOL_GEN_MAX_VPP` and
/// `RIGOL_GEN_MAX_PEAK_VOLTS` apply to every channel; on an MHO900,
/// `RIGOL_GEN<n>_MAX_VPP` and `RIGOL_GEN<n>_MAX_PEAK_VOLTS` (n = 1 or 2)
/// override one. Each pair is set together and checked against the family's
/// hardware range: 10 Vpp / 5 V on a DHO, 20 Vpp / 10 V open-circuit on an
/// MHO900. A PSU process never drives a generator, but its values are still
/// checked, against the DHO range, as they were before the MHO900.
fn generator_limits_from(
    family: Family,
    f64_var: &dyn Fn(&str) -> anyhow::Result<Option<f64>>,
) -> anyhow::Result<[GeneratorLimits; 2]> {
    let range = match family {
        Family::Mho900 => Family::Mho900,
        Family::Dho900 | Family::Dp800 => Family::Dho900,
    };
    let pair = |vpp: &str, peak: &str| -> anyhow::Result<Option<GeneratorLimits>> {
        match (f64_var(vpp)?, f64_var(peak)?) {
            (Some(v), Some(p)) => Ok(Some(
                GeneratorLimits::for_family(range, v, p)
                    .with_context(|| format!("{vpp} / {peak}"))?,
            )),
            (None, None) => Ok(None),
            _ => anyhow::bail!("{vpp} and {peak} must be set together"),
        }
    };
    let every = match pair("RIGOL_GEN_MAX_VPP", "RIGOL_GEN_MAX_PEAK_VOLTS")? {
        Some(limits) => limits,
        None => GeneratorLimits::hardware(range)?,
    };
    let mut limits = [every; 2];
    for ch in 1..=2u8 {
        let (vpp, peak) = (
            format!("RIGOL_GEN{ch}_MAX_VPP"),
            format!("RIGOL_GEN{ch}_MAX_PEAK_VOLTS"),
        );
        if let Some(l) = pair(&vpp, &peak)? {
            anyhow::ensure!(
                family == Family::Mho900,
                "{vpp} / {peak} are for an mho900, the family with two generator channels; {}",
                "use RIGOL_GEN_MAX_VPP / RIGOL_GEN_MAX_PEAK_VOLTS"
            );
            limits[usize::from(ch - 1)] = l;
        }
    }
    Ok(limits)
}

/// A ceiling from the environment. `str::parse::<f64>` accepts `NaN` and
/// `inf`, neither of which is a ceiling, so they are refused here with the
/// variable's name; the library refuses zero and negatives.
fn env_f64(key: &str, value: Option<String>) -> anyhow::Result<Option<f64>> {
    let Some(v) = value else {
        return Ok(None);
    };
    let n: f64 = v
        .trim()
        .parse()
        .with_context(|| format!("{key} must be a number"))?;
    anyhow::ensure!(n.is_finite(), "{key} must be a finite number, got {v:?}");
    Ok(Some(n))
}

#[tool_router(router = psu_router)]
impl RigolServer {
    // ── Reads ─────────────────────────────────────────────────────────────────

    #[tool(
        description = "Get instrument identity: vendor, model, serial number and firmware (*IDN?)"
    )]
    async fn get_identity(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::identity(self.psu().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Get one channel's rating and voltage/current setpoints (:APPLy? CHn). Setpoints are what the channel is set to, not what it is delivering — use measure_channel for that"
    )]
    async fn get_channel(
        &self,
        Parameters(p): Parameters<ChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::get_channel(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(description = "List all three channels with their ratings and setpoints")]
    async fn list_channels(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::list_channels(self.psu().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Measure what a channel is actually delivering: volts, amps and watts (:MEASure:ALL? CHn)"
    )]
    async fn measure_channel(
        &self,
        Parameters(p): Parameters<ChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::measure(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(description = "Get whether a channel's output is on or off (:OUTPut? CHn)")]
    async fn get_output_state(
        &self,
        Parameters(p): Parameters<ChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::output_state(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Get over-voltage and over-current protection for a channel: enabled state, trip levels, and whether protection has tripped (:OUTPut:OVP/OCP)"
    )]
    async fn get_protection(
        &self,
        Parameters(p): Parameters<ChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::protection(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Get the safety ceilings currently in force for each channel. Any setpoint above these is refused before it reaches the instrument"
    )]
    async fn get_safety_limits(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::limits(self.psu().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Get the instrument's LAN configuration (:SYSTem:COMMunicate:LAN). Fields this firmware does not support come back null"
    )]
    async fn get_lan_config(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::lan_config(self.psu().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(description = "Read and drain the instrument's error queue (:SYSTem:ERRor?)")]
    async fn get_instrument_errors(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::psu::instrument_errors(self.psu().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    // ── Writes ────────────────────────────────────────────────────────────────

    #[tool(
        description = "Set a channel's voltage setpoint (:SOURce<n>:VOLTage). Does NOT turn the output on. Refused if above the channel's safety ceiling"
    )]
    async fn set_voltage(
        &self,
        Parameters(p): Parameters<SetVoltageParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::psu::set_voltage(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Set a channel's current limit (:SOURce<n>:CURRent). Does NOT turn the output on. Refused if above the channel's safety ceiling"
    )]
    async fn set_current(
        &self,
        Parameters(p): Parameters<SetCurrentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::psu::set_current(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Set over-voltage (OVP) and over-current (OCP) protection for a channel: trip levels (:OUTPut:OVP:VALue, :OUTPut:OCP:VALue, refused above the channel's ceiling) and on/off (:OUTPut:OVP, :OUTPut:OCP). Levels are applied before on/off. With protection on, the instrument itself turns the output off above the trip level, whoever set the setpoint. Returns the protection read back, with warnings for protection that is on but trips above the ceiling"
    )]
    async fn set_protection(
        &self,
        Parameters(p): Parameters<SetProtectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::psu::set_protection(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "ENERGISE a channel — turns the output on at its current setpoints (:OUTPut CHn,ON). Refused if either setpoint is above the channel's safety ceiling, wherever it was set. Check get_channel first: this delivers real power to whatever is connected"
    )]
    async fn output_on(
        &self,
        Parameters(p): Parameters<ChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::psu::output_on(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(description = "Turn a channel's output off (:OUTPut CHn,OFF)")]
    async fn output_off(
        &self,
        Parameters(p): Parameters<ChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::psu::output_off(self.psu().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }
}

/// Oscilloscope tools. Names mirror the PSU's where the meaning is the same
/// (`get_identity`, `get_lan_config`, `get_instrument_errors`); the two sets
/// never share a process.
#[tool_router(router = scope_router)]
impl RigolServer {
    // ── Reads ─────────────────────────────────────────────────────────────────

    #[tool(
        name = "get_identity",
        description = "Get oscilloscope identity: vendor, model, serial number, firmware (*IDN?) and analog channel count"
    )]
    async fn scope_get_identity(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::identity(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        name = "get_lan_config",
        description = "Get the oscilloscope's LAN state: link status, DHCP, IP, mask, gateway, MAC (:LAN:STATus? :DHCP? :IPADdress? :SMASk? :GATeway? :MAC?)"
    )]
    async fn scope_get_lan_config(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::lan_config(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        name = "get_instrument_errors",
        description = "Drain the oscilloscope's SCPI error queue (:SYSTem:ERRor?)"
    )]
    async fn scope_get_instrument_errors(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::instrument_errors(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Get trigger status and edge setup, timebase, sample rate, memory depth, acquisition type, and every channel's display/scale/offset/coupling/probe, plus input impedance on an MHO900 (:TRIGger:STATus? :TIMebase:MAIN:SCALe? :ACQuire:SRATe? :CHANnel<n>:…). trigger_status is TD (triggered), WAIT, RUN, AUTO or STOP"
    )]
    async fn get_status(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::status(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Make one automatic measurement on a channel (:MEASure:ITEM? <item>,CHANnel<n>). value is null when the scope cannot measure it on the current signal — e.g. a frequency with no edges on screen. Like choosing it on the front panel, this also adds the item to the scope's on-screen measurement list"
    )]
    async fn measure(
        &self,
        Parameters(p): Parameters<MeasureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::measure(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Get the statistics the scope keeps for one measurement on a channel: current, minimum, maximum, average, standard deviation and count of readings (:MEASure:STATistic:ITEM? CURRent|MINimum|MAXimum|AVERages|DEViation|CNT,<item>,CHANnel<n>). They cover every reading since the item was added or since reset_measurement_statistics. A value is null when the scope has no reading for it. Like measure, this adds the item to the on-screen measurement list if it is not there yet, and its statistics start from that reading"
    )]
    async fn measure_statistics(
        &self,
        Parameters(p): Parameters<MeasureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::measure_statistics(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Capture the oscilloscope screen as a PNG image (:DISPlay:DATA? PNG). The quickest way to see the instrument's whole state at once"
    )]
    async fn get_screenshot(&self) -> Result<CallToolResult, ErrorData> {
        let png = tools::scope::screenshot(self.scope().await?)
            .await
            .map_err(tool_error)?;
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        Ok(CallToolResult::success(vec![ContentBlock::image(
            data,
            "image/png",
        )]))
    }

    #[tool(
        description = "Read one channel's on-screen trace in volts against seconds (:WAVeform:PREamble? :WAVeform:DATA?), thinned to min/max buckets, with min/max/mean/RMS over every captured sample. Sets the waveform transfer source and format only — no acquisition or display setting changes"
    )]
    async fn get_waveform(
        &self,
        Parameters(p): Parameters<WaveformParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::waveform(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Save one channel's whole acquisition record, not the thinned trace, to a file on the host running this server. Reads the scope's internal memory in blocks (:WAVeform:MODE RAW, :WAVeform:FORMat WORD, :WAVeform:STARt/STOP, :WAVeform:DATA?) with :WAVeform:PREamble?, the channel setup and input impedance (:CHANnel<n>:IMPedance? on an MHO900), :ACQuire:SRATe? and :TRIGger:POSition?. Writes <path>.npz (codes, volts and time arrays plus the scaling as scalars, readable with numpy.load) and a .json sidecar with the settings read back and the npz's SHA-256; never overwrites. RAW reads need the scope stopped: a running scope is refused unless stop_if_running, which sends :STOP and restarts the acquisition afterwards and requires RIGOL_ALLOW_WRITES"
    )]
    async fn save_waveform(
        &self,
        Parameters(p): Parameters<SaveWaveformParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if p.stop_if_running.unwrap_or(false) {
            self.guard_write()?;
        }
        let data = tools::scope::save_waveform(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    // ── Writes ────────────────────────────────────────────────────────────────
    //
    // Display and acquisition only — nothing here reaches the device under
    // test. Still behind RIGOL_ALLOW_WRITES, like every mutating tool.

    #[tool(description = "Start acquiring continuously (:RUN). Requires RIGOL_ALLOW_WRITES")]
    async fn run(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::run(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Stop acquiring and freeze the display (:STOP). Requires RIGOL_ALLOW_WRITES"
    )]
    async fn stop(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::stop(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Arm a single acquisition: capture once on the next trigger, then stop (:SINGle). Arm this before causing the event, e.g. before turning a supply on. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn single(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::single(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Let the scope choose vertical scale, timebase and trigger for the current signal (:AUToset). Overwrites the current setup. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn autoset(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::autoset(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Clear the statistics history of every measurement item and start counting again (:MEASure:STATistic:RESet). The items stay in the on-screen list. Returns any errors the scope raised. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn reset_measurement_statistics(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::reset_measurement_statistics(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Change a channel's vertical setup: display, scale, offset, coupling, probe ratio (:CHANnel<n>:…). Omitted fields are left alone. Returns the setup the scope actually holds afterwards plus any errors it raised. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn configure_channel(
        &self,
        Parameters(p): Parameters<ConfigureChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::configure_channel(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Change the horizontal scale and/or offset (:TIMebase:MAIN:SCALe / :OFFSet). Returns what the scope holds afterwards plus any errors it raised. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn configure_timebase(
        &self,
        Parameters(p): Parameters<ConfigureTimebaseParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::configure_timebase(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Switch to an edge trigger and set its source, slope, level and sweep (:TRIGger:MODE EDGE, :TRIGger:EDGE:SOURce CHANnel<n>|D<n>, :TRIGger:EDGE:SLOPe, :TRIGger:EDGE:LEVel, :TRIGger:SWEep). The source is an analog channel or, on the DHO900 and MHO900 series, a digital channel D0–D15. Omitted fields are left alone. Returns what the scope holds afterwards plus any errors it raised. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn configure_edge_trigger(
        &self,
        Parameters(p): Parameters<ConfigureEdgeTriggerParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::configure_edge_trigger(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    // ── Logic analyser (DHO900 and MHO900 series) ─────────────────────────────

    #[tool(
        description = "Get the logic analyser's state: on or off, the POD1 (D0–D7) and POD2 (D8–D15) logic thresholds in volts, and which of D0–D15 are shown (:LA:ENABle? :LA:POD<n>:THReshold? :LA:DIGital:ENABle? D<n>). DHO900 and MHO900 series, not the DHO800"
    )]
    async fn get_logic_analyzer(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::logic_analyzer(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Turn the logic analyser on or off, show or hide digital channels D0–D15, and set the pod logic thresholds (:LA:ENABle, :LA:DIGital:ENABle D<n>,<bool>, :LA:POD<n>:THReshold). Omitted fields are left alone. Turning the analyser on shows D0–D7 again, so list any other channels in show in the same call. Returns what the scope holds afterwards plus any errors it raised. DHO900 and MHO900 series, not the DHO800. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn configure_logic_analyzer(
        &self,
        Parameters(p): Parameters<ConfigureLogicAnalyzerParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::configure_logic_analyzer(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }
}

/// DHO800/DHO900-only scope tools: the measurement-clear spelling and the
/// one-channel signal generator.
#[tool_router(router = dho_scope_router)]
impl RigolServer {
    #[tool(
        description = "Remove every measurement from the scope's on-screen list (:MEASure:CLEar), including those added by measure. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn clear_measurements(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::clear_measurements(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    // ── Signal generator ──────────────────────────────────────────────────────
    //
    // Unlike the rest of the scope, this drives a signal into the device under
    // test. It follows the PSU's rules: configuring never energises, ceilings
    // are enforced in the library, output-on is explicit and logged.

    #[tool(
        description = "Get the built-in signal generator's state: output on/off, waveform, frequency, amplitude, offset, peak excursion, and the ceilings in force (:SOURce:OUTPut:STATe? :FUNCtion? :FREQuency? :VOLTage:AMPLitude? :VOLTage:OFFSet?)"
    )]
    async fn get_generator(&self) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::generator(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Set the signal generator's waveform, frequency, amplitude and/or offset (:SOURce:FUNCtion / :FREQuency / :VOLTage:AMPLitude / :VOLTage:OFFSet). Does NOT turn the output on. Refused if amplitude or the peak |offset| + amplitude/2 would exceed the ceilings. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn configure_generator(
        &self,
        Parameters(p): Parameters<ConfigureGeneratorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::configure_generator(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Turn the signal generator's output ON (:SOURce:OUTPut:STATe ON). This drives a signal into whatever is connected. The current settings are checked against the ceilings first. Confirm with the user before calling. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn generator_output_on(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::generator_output_on(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        description = "Turn the signal generator's output OFF (:SOURce:OUTPut:STATe OFF). Requires RIGOL_ALLOW_WRITES"
    )]
    async fn generator_output_off(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::generator_output_off(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }
}

/// MHO900-only scope tools: `:MEASure:DELete`, and the optional
/// two-channel generator (MHO900-AFG50 or MHO900-AFG100), whose tools take
/// the generator channel.
#[tool_router(router = mho_scope_router)]
impl RigolServer {
    #[tool(
        name = "clear_measurements",
        description = "Remove every measurement from the scope's on-screen list (:MEASure:DELete), including those added by measure. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn mho_clear_measurements(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::clear_measurements(self.scope().await?)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    // ── Signal generator ──────────────────────────────────────────────────────
    //
    // The DHO rules, per channel. Ceilings bound the open-circuit voltage: a
    // channel set to a 50 Ω load (:SOURce<n>:IMPedance FIFTy) counts twice
    // its settings, because that is what an unterminated input sees.

    #[tool(
        name = "get_generator",
        description = "Get one generator channel's state: output on/off, waveform, frequency, amplitude, offset, output load (high_z or fifty_ohm), open-circuit peak excursion, and the ceilings in force (:SOURce<n>:OUTPut:STATe? :FUNCtion? :FREQuency? :VOLTage:AMPLitude? :VOLTage:OFFSet? :IMPedance?). Refused when option MHO900-AFG50 or MHO900-AFG100 is not installed (:SYSTem:OPTion:STATus?)"
    )]
    async fn mho_get_generator(
        &self,
        Parameters(p): Parameters<GeneratorChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = tools::scope::generator_channel(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        name = "configure_generator",
        description = "Set one generator channel's waveform, frequency, amplitude and/or offset (:SOURce<n>:FUNCtion / :FREQuency / :VOLTage:AMPLitude / :VOLTage:OFFSet). Does NOT turn the output on. Refused if the open-circuit amplitude or peak |offset| + amplitude/2 would exceed that channel's ceilings; with the channel set to a 50 Ω load the open-circuit values are twice the settings. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn mho_configure_generator(
        &self,
        Parameters(p): Parameters<ConfigureGeneratorChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::configure_generator_channel(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        name = "generator_output_on",
        description = "Turn one generator channel's output ON (:SOURce<n>:OUTPut:STATe ON). This drives a signal into whatever is connected. The channel's settings and load setting are read and checked against its ceilings first. Confirm with the user before calling. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn mho_generator_output_on(
        &self,
        Parameters(p): Parameters<GeneratorChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::generator_channel_output_on(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }

    #[tool(
        name = "generator_output_off",
        description = "Turn one generator channel's output OFF (:SOURce<n>:OUTPut:STATe OFF). Requires RIGOL_ALLOW_WRITES"
    )]
    async fn mho_generator_output_off(
        &self,
        Parameters(p): Parameters<GeneratorChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::generator_channel_output_off(self.scope().await?, &p)
            .await
            .map_err(tool_error)?;
        Ok(Self::ok(&data))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RigolServer {
    fn get_info(&self) -> ServerConfig {
        let instructions = match self.family {
            Family::Dp800 => PSU_INSTRUCTIONS,
            Family::Dho900 => SCOPE_INSTRUCTIONS,
            Family::Mho900 => MHO_SCOPE_INSTRUCTIONS,
        };
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(instructions)
    }
}

const PSU_INSTRUCTIONS: &str = concat!(
    "RIGOL DP800-series bench power supply control over SCPI (raw TCP socket). ",
    "Configure with RIGOL_HOST. Optional: RIGOL_PORT (default 5555 — note this is ",
    "not the LXI-conventional 5025), RIGOL_TIMEOUT_MS (default 5000), ",
    "RIGOL_ALLOW_WRITES (default false — must be 'true' to enable mutating tools), ",
    "and per-channel safety ceilings RIGOL_CH<n>_MAX_VOLTS / RIGOL_CH<n>_MAX_AMPS ",
    "(default: the channel's own rating). ",
    "This instrument sources real power into real hardware: setting a voltage and ",
    "energising an output are separate operations, and setpoints above a channel's ",
    "ceiling are refused before they reach the instrument.",
);

const SCOPE_INSTRUCTIONS: &str = concat!(
    "RIGOL DHO800/DHO900-series oscilloscope control over SCPI (raw TCP socket). ",
    "Configure with RIGOL_HOST and RIGOL_FAMILY=dho900. Optional: RIGOL_PORT (default 5555), ",
    "RIGOL_TIMEOUT_MS (default 5000), RIGOL_ALLOW_WRITES (default false), and signal-generator ",
    "ceilings RIGOL_GEN_MAX_VPP / RIGOL_GEN_MAX_PEAK_VOLTS (default: the generator's own 10 Vpp / 5 V). ",
    "The built-in signal generator drives real hardware: configuring it and turning its output on ",
    "are separate operations, and settings over the ceilings are refused before they reach the scope. ",
    "Start with get_screenshot to see the instrument's state, then get_status for exact values. ",
    "Measurements that the current signal does not support return a null value.",
);

const MHO_SCOPE_INSTRUCTIONS: &str = concat!(
    "RIGOL MHO900-series oscilloscope control over SCPI (raw TCP socket). ",
    "Configure with RIGOL_HOST and RIGOL_FAMILY=mho900. Optional: RIGOL_PORT (default 5555), ",
    "RIGOL_TIMEOUT_MS (default 5000), RIGOL_ALLOW_WRITES (default false), and signal-generator ",
    "ceilings RIGOL_GEN_MAX_VPP / RIGOL_GEN_MAX_PEAK_VOLTS for both generator channels or ",
    "RIGOL_GEN<n>_MAX_VPP / RIGOL_GEN<n>_MAX_PEAK_VOLTS for one (default: the generator's own ",
    "20 Vpp / 10 V open-circuit). The generator is an option (MHO900-AFG50 or MHO900-AFG100); ",
    "without it the generator tools are refused. It drives real hardware: configuring a channel ",
    "and turning its output on are separate operations, and settings whose open-circuit voltage ",
    "is over the ceilings are refused before they reach the scope. ",
    "Start with get_screenshot to see the instrument's state, then get_status for exact values. ",
    "Measurements that the current signal does not support return a null value.",
);

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(
        vars: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Result<String, VarError> {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
                .ok_or(VarError::NotPresent)
        }
    }

    fn config(vars: &'static [(&'static str, &'static str)]) -> anyhow::Result<RigolServer> {
        RigolServer::from_lookup(lookup(vars))
    }

    #[test]
    fn a_ceiling_that_is_not_a_positive_number_stops_startup() {
        for bad in ["NaN", "inf", "-inf", "0", "-3.3"] {
            let vars: &'static [_] = Box::leak(Box::new([
                ("RIGOL_HOST", "127.0.0.1"),
                ("RIGOL_CH1_MAX_VOLTS", bad),
                ("RIGOL_CH1_MAX_AMPS", "0.5"),
            ]));
            assert!(config(vars).is_err(), "RIGOL_CH1_MAX_VOLTS={bad} accepted");
        }
        let good = config(&[
            ("RIGOL_HOST", "127.0.0.1"),
            ("RIGOL_CH1_MAX_VOLTS", "3.3"),
            ("RIGOL_CH1_MAX_AMPS", "0.5"),
        ]);
        assert!(good.is_ok(), "{:?}", good.err());
    }

    fn writes(value: &'static str) -> anyhow::Result<bool> {
        let vars: &'static [_] = Box::leak(Box::new([
            ("RIGOL_HOST", "127.0.0.1"),
            ("RIGOL_ALLOW_WRITES", value),
        ]));
        config(vars).map(|s| s.writes_enabled)
    }

    #[test]
    fn allow_writes_accepts_an_explicit_list_in_any_case() {
        for on in ["true", "TRUE", "True", "1", "yes", "YES"] {
            assert!(writes(on).unwrap(), "{on}");
        }
        for off in ["false", "FALSE", "0", "no", "No"] {
            assert!(!writes(off).unwrap(), "{off}");
        }
        assert!(
            !config(&[("RIGOL_HOST", "127.0.0.1")])
                .unwrap()
                .writes_enabled
        );
    }

    /// `on` or a typo must not quietly mean off: the operator meant
    /// something, and a startup error naming the variable says what.
    #[test]
    fn allow_writes_refuses_anything_else_at_startup() {
        for bad in ["on", "off", "enabled", "t", ""] {
            let err = writes(bad).expect_err(bad);
            assert!(err.to_string().contains("RIGOL_ALLOW_WRITES"), "{err}");
        }
    }

    /// Only an unset variable means unset; one that is not UTF-8 is an error.
    #[test]
    fn a_variable_that_is_not_utf8_stops_startup() {
        let lookup = |key: &str| match key {
            "RIGOL_HOST" => Ok("127.0.0.1".to_string()),
            "RIGOL_ALLOW_WRITES" => Err(VarError::NotUnicode(std::ffi::OsString::from("x"))),
            _ => Err(VarError::NotPresent),
        };
        let err = RigolServer::from_lookup(lookup)
            .err()
            .expect("a non-UTF-8 value was treated as unset");
        assert!(err.to_string().contains("RIGOL_ALLOW_WRITES"), "{err}");
    }

    fn sorted_tools(router: &ToolRouter<RigolServer>) -> Vec<rmcp::model::Tool> {
        let mut tools = router.list_all();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }

    /// The PSU tool surface is a published contract: same names, same
    /// schemas, same descriptions as before the family split.
    #[test]
    fn the_psu_tools_are_unchanged() {
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/dp800-tools.json")).unwrap();
        let actual =
            serde_json::to_value(sorted_tools(&RigolServer::router_for(Family::Dp800))).unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn a_psu_server_refuses_a_scope_and_names_both() {
        use rigol_fake::{Fake, Reply, step};
        let fake = Fake::start(vec![step(
            "*IDN?",
            Reply::line("RIGOL TECHNOLOGIES,DHO924S,DHO9S000000001,00.01.05"),
        )])
        .await;
        let server = RigolServer::for_test(Family::Dp800, fake.addr);

        let err = server
            .psu()
            .await
            .err()
            .expect("a scope must not pass as a PSU");

        assert_eq!(err.code, ErrorCode::INVALID_REQUEST);
        assert!(err.message.contains("dp800"), "{}", err.message);
        assert!(err.message.contains("DHO924S"), "{}", err.message);
        fake.finish();
    }

    fn names(router: &ToolRouter<RigolServer>) -> Vec<String> {
        sorted_tools(router)
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    #[test]
    fn a_scope_server_exposes_exactly_the_scope_tools() {
        assert_eq!(
            names(&RigolServer::router_for(Family::Dho900)),
            [
                "autoset",
                "clear_measurements",
                "configure_channel",
                "configure_edge_trigger",
                "configure_generator",
                "configure_logic_analyzer",
                "configure_timebase",
                "generator_output_off",
                "generator_output_on",
                "get_generator",
                "get_identity",
                "get_instrument_errors",
                "get_lan_config",
                "get_logic_analyzer",
                "get_screenshot",
                "get_status",
                "get_waveform",
                "measure",
                "measure_statistics",
                "reset_measurement_statistics",
                "run",
                "save_waveform",
                "single",
                "stop",
            ]
        );
    }

    /// An MHO900 process offers the same tool names as a DHO one; only the
    /// generator tools take a channel.
    #[test]
    fn an_mho900_server_exposes_the_scope_tools_with_generator_channels() {
        let mho = RigolServer::router_for(Family::Mho900);
        assert_eq!(names(&mho), names(&RigolServer::router_for(Family::Dho900)));
        for tool in sorted_tools(&mho) {
            let schema = serde_json::to_value(&tool.input_schema).unwrap();
            let takes_channel = schema["required"]
                .as_array()
                .is_some_and(|r| r.iter().any(|f| f == "channel"));
            let generator = tool.name.contains("generator");
            if generator {
                assert!(takes_channel, "{} takes no channel", tool.name);
            }
        }
        let clear = mho
            .list_all()
            .into_iter()
            .find(|t| t.name == "clear_measurements")
            .unwrap();
        assert!(
            clear
                .description
                .is_some_and(|d| d.contains(":MEASure:DELete"))
        );
    }

    /// The reported failure: `dho900` pointed at an MHO984.
    #[tokio::test]
    async fn a_dho900_server_refuses_an_mho900_and_names_both() {
        use rigol_fake::{Fake, Reply, step};
        let fake = Fake::start(vec![step(
            "*IDN?",
            Reply::line("RIGOL TECHNOLOGIES,MHO984,MHO9XXXXXXXXXX,00.01.00"),
        )])
        .await;
        let server = RigolServer::for_test(Family::Dho900, fake.addr);

        let err = server
            .scope()
            .await
            .err()
            .expect("an MHO984 passed as a DHO900");

        assert_eq!(err.code, ErrorCode::INVALID_REQUEST);
        assert!(err.message.contains("dho900"), "{}", err.message);
        assert!(err.message.contains("MHO984"), "{}", err.message);
        fake.finish();
    }

    #[tokio::test]
    async fn an_mho900_server_connects_to_an_mho984() {
        use rigol_fake::{Fake, Reply, step};
        let fake = Fake::start(vec![step(
            "*IDN?",
            Reply::line("RIGOL TECHNOLOGIES,MHO984,MHO9XXXXXXXXXX,00.01.00"),
        )])
        .await;
        let server = RigolServer::for_test(Family::Mho900, fake.addr);

        let scope = server.scope().await.expect("connect");

        assert_eq!(scope.family(), Family::Mho900);
        assert_eq!(scope.generator_channels(), 2);
        fake.finish();
    }

    fn generator_config(
        vars: &'static [(&'static str, &'static str)],
    ) -> anyhow::Result<[GeneratorLimits; 2]> {
        config(vars).map(|s| s.generator_limits)
    }

    #[test]
    fn mho900_generator_ceilings_default_to_its_range_and_split_by_channel() {
        let d =
            generator_config(&[("RIGOL_HOST", "127.0.0.1"), ("RIGOL_FAMILY", "mho900")]).unwrap();
        assert_eq!((d[1].max_vpp(), d[1].max_peak_volts()), (20.0, 10.0));

        let l = generator_config(&[
            ("RIGOL_HOST", "127.0.0.1"),
            ("RIGOL_FAMILY", "mho900"),
            ("RIGOL_GEN_MAX_VPP", "12"),
            ("RIGOL_GEN_MAX_PEAK_VOLTS", "6"),
            ("RIGOL_GEN2_MAX_VPP", "3.3"),
            ("RIGOL_GEN2_MAX_PEAK_VOLTS", "1.65"),
        ])
        .unwrap();
        assert_eq!((l[0].max_vpp(), l[0].max_peak_volts()), (12.0, 6.0));
        assert_eq!((l[1].max_vpp(), l[1].max_peak_volts()), (3.3, 1.65));

        assert!(
            generator_config(&[
                ("RIGOL_HOST", "127.0.0.1"),
                ("RIGOL_FAMILY", "mho900"),
                ("RIGOL_GEN1_MAX_VPP", "21"),
                ("RIGOL_GEN1_MAX_PEAK_VOLTS", "5"),
            ])
            .is_err()
        );
        assert!(
            generator_config(&[
                ("RIGOL_HOST", "127.0.0.1"),
                ("RIGOL_FAMILY", "mho900"),
                ("RIGOL_GEN2_MAX_VPP", "3.3"),
            ])
            .is_err()
        );
    }

    /// The DHO keeps its range, and a second-channel ceiling it cannot
    /// apply stops startup rather than being ignored.
    #[test]
    fn dho900_generator_ceilings_keep_their_range() {
        let d =
            generator_config(&[("RIGOL_HOST", "127.0.0.1"), ("RIGOL_FAMILY", "dho900")]).unwrap();
        assert_eq!((d[0].max_vpp(), d[0].max_peak_volts()), (10.0, 5.0));
        assert!(
            generator_config(&[
                ("RIGOL_HOST", "127.0.0.1"),
                ("RIGOL_FAMILY", "dho900"),
                ("RIGOL_GEN_MAX_VPP", "20"),
                ("RIGOL_GEN_MAX_PEAK_VOLTS", "10"),
            ])
            .is_err()
        );
        let err = generator_config(&[
            ("RIGOL_HOST", "127.0.0.1"),
            ("RIGOL_FAMILY", "dho900"),
            ("RIGOL_GEN2_MAX_VPP", "3.3"),
            ("RIGOL_GEN2_MAX_PEAK_VOLTS", "1.65"),
        ])
        .unwrap_err();
        assert!(err.to_string().contains("mho900"), "{err}");
    }

    /// Stopping the scope for a memory read is a write: with writes off,
    /// `stop_if_running` is refused before the scope is even connected to.
    #[tokio::test]
    async fn save_waveform_stopping_the_scope_needs_writes() {
        use rigol_fake::Fake;
        let fake = Fake::start(vec![]).await;
        let server = RigolServer::for_test(Family::Mho900, fake.addr);

        let Err(err) = server
            .save_waveform(Parameters(SaveWaveformParams {
                channel: 1,
                path: "/nonexistent/record.npz".into(),
                stop_if_running: Some(true),
                expect_impedance: None,
                chunk_points: None,
            }))
            .await
        else {
            panic!("stopping the scope with writes off");
        };

        assert_eq!(err.code, ErrorCode::INVALID_REQUEST);
        assert!(
            err.message.contains("RIGOL_ALLOW_WRITES"),
            "{}",
            err.message
        );
        assert_eq!(fake.connections(), 0);
        fake.finish();
    }

    /// Nothing that sets a PSU output can be reachable from a scope process.
    #[test]
    fn no_psu_write_is_reachable_from_a_scope_server() {
        for family in [Family::Dho900, Family::Mho900] {
            let scope = RigolServer::router_for(family);
            for tool in [
                "set_voltage",
                "set_current",
                "set_protection",
                "output_on",
                "output_off",
            ] {
                assert!(
                    !scope.has_route(tool),
                    "{tool} is reachable from the {family} router"
                );
            }
        }
    }
}
