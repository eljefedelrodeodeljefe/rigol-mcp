use anyhow::Context;
use base64::Engine;
use rigol::{Dho900, Dp800, Family, Limits, SessionConfig};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ErrorCode, ErrorData, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use serde_json::Value;
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
    writes_enabled: bool,
    /// Connected on first use. Two concurrent sessions were verified to coexist
    /// on this instrument, and a session survives idle, so holding one open is
    /// safe.
    psu: OnceCell<Dp800>,
    /// The oscilloscope, for a `dho900` process. Connected on first use.
    scope: OnceCell<Dho900>,
}

impl RigolServer {
    pub fn from_env() -> anyhow::Result<Self> {
        let host = std::env::var("RIGOL_HOST").context("RIGOL_HOST not set")?;
        // Defaults to dp800 so configurations written before scope support
        // keep meaning what they meant.
        let family: Family = match std::env::var("RIGOL_FAMILY") {
            Ok(f) => f.parse().map_err(anyhow::Error::msg)?,
            Err(_) => Family::Dp800,
        };
        let port = match std::env::var("RIGOL_PORT") {
            Ok(p) => Some(
                p.parse()
                    .context("RIGOL_PORT must be a valid port number")?,
            ),
            Err(_) => None,
        };
        let timeout_ms: u64 = std::env::var("RIGOL_TIMEOUT_MS")
            .unwrap_or_else(|_| "5000".into())
            .parse()
            .context("RIGOL_TIMEOUT_MS must be a whole number of milliseconds")?;
        let writes_enabled = std::env::var("RIGOL_ALLOW_WRITES")
            .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
            .unwrap_or(false);

        // Explicit ceilings override the channel's rating. Anything not set here
        // is backfilled from the instrument's own rating at connect time.
        let mut configured_limits = Limits::new();
        for ch in 1..=3u8 {
            let volts = env_f64(&format!("RIGOL_CH{ch}_MAX_VOLTS"))?;
            let amps = env_f64(&format!("RIGOL_CH{ch}_MAX_AMPS"))?;
            if let (Some(v), Some(a)) = (volts, amps) {
                configured_limits = configured_limits.with_channel(ch, v, a)?;
            } else if volts.is_some() || amps.is_some() {
                anyhow::bail!(
                    "RIGOL_CH{ch}_MAX_VOLTS and RIGOL_CH{ch}_MAX_AMPS must be set together"
                );
            }
        }

        Ok(Self {
            family,
            tool_router: Self::router_for(family),
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
            Family::Dho900 => Self::scope_router(),
        }
    }

    async fn scope(&self) -> Result<&Dho900, ErrorData> {
        self.scope
            .get_or_try_init(|| async {
                Dho900::connect(&self.host, self.port, self.cfg.clone()).await
            })
            .await
            .map_err(|e| tool_error(e.into()))
    }

    fn ok(value: &Value) -> CallToolResult {
        CallToolResult::success(vec![Content::text(
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

fn env_f64(key: &str) -> anyhow::Result<Option<f64>> {
    match std::env::var(key) {
        Ok(v) => Ok(Some(
            v.parse()
                .with_context(|| format!("{key} must be a number"))?,
        )),
        Err(_) => Ok(None),
    }
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
        description = "Set over-voltage (OVP) and/or over-current (OCP) protection trip levels for a channel"
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
        description = "ENERGISE a channel — turns the output on at its current setpoints (:OUTPut CHn,ON). Check get_channel first: this delivers real power to whatever is connected"
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
        description = "Get trigger status and edge setup, timebase, sample rate, memory depth, acquisition type, and every channel's display/scale/offset/coupling/probe (:TRIGger:STATus? :TIMebase:MAIN:SCALe? :ACQuire:SRATe? :CHANnel<n>:…). trigger_status is TD (triggered), WAIT, RUN, AUTO or STOP"
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
        description = "Capture the oscilloscope screen as a PNG image (:DISPlay:DATA? PNG). The quickest way to see the instrument's whole state at once"
    )]
    async fn get_screenshot(&self) -> Result<CallToolResult, ErrorData> {
        let png = tools::scope::screenshot(self.scope().await?)
            .await
            .map_err(tool_error)?;
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        Ok(CallToolResult::success(vec![Content::image(
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
        description = "Remove every measurement from the scope's on-screen list (:MEASure:CLEar), including those added by measure. Requires RIGOL_ALLOW_WRITES"
    )]
    async fn clear_measurements(&self) -> Result<CallToolResult, ErrorData> {
        self.guard_write()?;
        let data = tools::scope::clear_measurements(self.scope().await?)
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
        description = "Switch to an edge trigger and set its source, slope, level and sweep (:TRIGger:MODE EDGE, :TRIGger:EDGE:…, :TRIGger:SWEep). Omitted fields are left alone. Returns what the scope holds afterwards plus any errors it raised. Requires RIGOL_ALLOW_WRITES"
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
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RigolServer {
    fn get_info(&self) -> ServerInfo {
        let instructions = match self.family {
            Family::Dp800 => PSU_INSTRUCTIONS,
            Family::Dho900 => SCOPE_INSTRUCTIONS,
        };
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
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
    "RIGOL_TIMEOUT_MS (default 5000), RIGOL_ALLOW_WRITES (default false). ",
    "Start with get_screenshot to see the instrument's state, then get_status for exact values. ",
    "Measurements that the current signal does not support return a null value.",
);

#[cfg(test)]
mod tests {
    use super::*;

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
                "configure_timebase",
                "get_identity",
                "get_instrument_errors",
                "get_lan_config",
                "get_screenshot",
                "get_status",
                "get_waveform",
                "measure",
                "run",
                "single",
                "stop",
            ]
        );
    }

    /// Nothing that sets a PSU output can be reachable from a scope process.
    #[test]
    fn no_psu_write_is_reachable_from_a_scope_server() {
        let scope = RigolServer::router_for(Family::Dho900);
        for tool in [
            "set_voltage",
            "set_current",
            "set_protection",
            "output_on",
            "output_off",
        ] {
            assert!(
                !scope.has_route(tool),
                "{tool} is reachable from the scope router"
            );
        }
    }
}
