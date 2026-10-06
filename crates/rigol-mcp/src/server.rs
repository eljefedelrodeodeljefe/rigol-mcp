use anyhow::Context;
use rigol::{Dp800, Family, Limits, SessionConfig};
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

/// Oscilloscope tools. Empty until the DHO900 device model lands.
#[tool_router(router = scope_router)]
impl RigolServer {}

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
    "RIGOL_TIMEOUT_MS (default 5000), RIGOL_ALLOW_WRITES (default false).",
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

    #[test]
    fn a_scope_server_exposes_no_psu_tools() {
        let scope = RigolServer::router_for(Family::Dho900);
        for tool in sorted_tools(&RigolServer::router_for(Family::Dp800)) {
            assert!(
                !scope.has_route(&tool.name),
                "{} leaked into the scope router",
                tool.name
            );
        }
    }
}
