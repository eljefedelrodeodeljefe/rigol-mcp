//! Pure tool logic for the oscilloscope — no MCP types cross this boundary.

use anyhow::Result;
use rigol::{ChannelConfig, Dho900, EdgeTriggerConfig};
use serde_json::{Value, json};

use crate::params::*;

pub async fn identity(scope: &Dho900) -> Result<Value> {
    let mut v = serde_json::to_value(scope.identity())?;
    v["channels"] = json!(scope.channel_count());
    Ok(v)
}

pub async fn lan_config(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.lan_config().await?)?)
}

pub async fn instrument_errors(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.errors().await?)?)
}

pub async fn status(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.status().await?)?)
}

pub async fn measure(scope: &Dho900, p: &MeasureParams) -> Result<Value> {
    Ok(serde_json::to_value(
        scope.measure(p.item, p.channel).await?,
    )?)
}

pub async fn screenshot(scope: &Dho900) -> Result<Vec<u8>> {
    Ok(scope.screenshot().await?)
}

/// The capture, thinned to at most `max_points` min/max buckets, with
/// statistics over every sample.
pub async fn waveform(scope: &Dho900, p: &WaveformParams) -> Result<Value> {
    let limit = p
        .max_points
        .unwrap_or(DEFAULT_WAVEFORM_POINTS)
        .clamp(1, MAX_WAVEFORM_POINTS);
    let w = scope.waveform(p.channel).await?;
    let buckets = w.decimate(limit);
    Ok(json!({
        "channel": w.channel,
        "stats": w.stats(),
        "seconds_per_sample": w.preamble.x_increment,
        "captured_points": w.volts.len(),
        "returned_buckets": buckets.len(),
        "buckets": buckets,
    }))
}

// ── Writes ───────────────────────────────────────────────────────────────────

pub async fn run(scope: &Dho900) -> Result<Value> {
    scope.run().await?;
    Ok(json!({ "sent": ":RUN" }))
}

pub async fn stop(scope: &Dho900) -> Result<Value> {
    scope.stop().await?;
    Ok(json!({ "sent": ":STOP" }))
}

pub async fn single(scope: &Dho900) -> Result<Value> {
    scope.single().await?;
    Ok(json!({ "sent": ":SINGle" }))
}

pub async fn autoset(scope: &Dho900) -> Result<Value> {
    scope.autoset().await?;
    Ok(json!({ "sent": ":AUToset" }))
}

pub async fn clear_measurements(scope: &Dho900) -> Result<Value> {
    scope.clear_measurements().await?;
    Ok(json!({ "sent": ":MEASure:CLEar" }))
}

pub async fn configure_channel(scope: &Dho900, p: &ConfigureChannelParams) -> Result<Value> {
    let cfg = ChannelConfig {
        displayed: p.displayed,
        volts_per_div: p.volts_per_div,
        offset_volts: p.offset_volts,
        coupling: p.coupling,
        probe_ratio: p.probe_ratio,
    };
    Ok(serde_json::to_value(
        scope.configure_channel(p.channel, &cfg).await?,
    )?)
}

pub async fn configure_timebase(scope: &Dho900, p: &ConfigureTimebaseParams) -> Result<Value> {
    Ok(serde_json::to_value(
        scope
            .configure_timebase(p.seconds_per_div, p.offset_seconds)
            .await?,
    )?)
}

pub async fn configure_edge_trigger(
    scope: &Dho900,
    p: &ConfigureEdgeTriggerParams,
) -> Result<Value> {
    let cfg = EdgeTriggerConfig {
        channel: p.channel,
        slope: p.slope,
        level_volts: p.level_volts,
        sweep: p.sweep,
    };
    Ok(serde_json::to_value(
        scope.configure_edge_trigger(&cfg).await?,
    )?)
}
