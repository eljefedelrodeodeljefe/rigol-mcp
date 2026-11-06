//! Pure tool logic — no MCP types cross this boundary.

use anyhow::Result;
use rigol::Dp800;
use serde_json::{Value, json};

use crate::params::*;

pub async fn identity(psu: &Dp800) -> Result<Value> {
    Ok(serde_json::to_value(psu.identity())?)
}

pub async fn get_channel(psu: &Dp800, p: &ChannelParams) -> Result<Value> {
    Ok(serde_json::to_value(psu.channel(p.channel).await?)?)
}

pub async fn list_channels(psu: &Dp800) -> Result<Value> {
    Ok(serde_json::to_value(psu.channels().await?)?)
}

pub async fn measure(psu: &Dp800, p: &ChannelParams) -> Result<Value> {
    Ok(serde_json::to_value(psu.measure(p.channel).await?)?)
}

pub async fn output_state(psu: &Dp800, p: &ChannelParams) -> Result<Value> {
    Ok(json!({
        "channel": p.channel,
        "output": psu.output_state(p.channel).await?,
    }))
}

pub async fn protection(psu: &Dp800, p: &ChannelParams) -> Result<Value> {
    Ok(serde_json::to_value(psu.protection(p.channel).await?)?)
}

pub async fn lan_config(psu: &Dp800) -> Result<Value> {
    Ok(serde_json::to_value(psu.lan_config().await?)?)
}

pub async fn instrument_errors(psu: &Dp800) -> Result<Value> {
    Ok(serde_json::to_value(psu.errors().await?)?)
}

/// Reports the ceiling in force for every channel, so a caller can see what it
/// is allowed to do before being refused.
pub async fn limits(psu: &Dp800) -> Result<Value> {
    let mut out = Vec::new();
    for ch in 1..=3u8 {
        let r = psu.limits().ceiling(ch)?;
        out.push(json!({ "channel": ch, "max_volts": r.volts(), "max_amps": r.amps() }));
    }
    Ok(Value::Array(out))
}

// ── Writes ───────────────────────────────────────────────────────────────────

pub async fn set_voltage(psu: &Dp800, p: &SetVoltageParams) -> Result<Value> {
    psu.set_voltage(p.channel, p.volts).await?;
    Ok(serde_json::to_value(psu.channel(p.channel).await?)?)
}

pub async fn set_current(psu: &Dp800, p: &SetCurrentParams) -> Result<Value> {
    psu.set_current(p.channel, p.amps).await?;
    Ok(serde_json::to_value(psu.channel(p.channel).await?)?)
}

pub async fn set_protection(psu: &Dp800, p: &SetProtectionParams) -> Result<Value> {
    if let Some(v) = p.ovp_volts {
        psu.set_ovp(p.channel, v).await?;
    }
    if let Some(a) = p.ocp_amps {
        psu.set_ocp(p.channel, a).await?;
    }
    Ok(serde_json::to_value(psu.protection(p.channel).await?)?)
}

pub async fn output_on(psu: &Dp800, p: &ChannelParams) -> Result<Value> {
    psu.output_on(p.channel).await?;
    Ok(json!({
        "channel": p.channel,
        "output": psu.output_state(p.channel).await?,
        "measured": psu.measure(p.channel).await?,
    }))
}

pub async fn output_off(psu: &Dp800, p: &ChannelParams) -> Result<Value> {
    psu.output_off(p.channel).await?;
    Ok(json!({
        "channel": p.channel,
        "output": psu.output_state(p.channel).await?,
    }))
}
