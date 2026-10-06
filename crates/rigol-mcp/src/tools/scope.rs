//! Pure tool logic for the oscilloscope — no MCP types cross this boundary.

use anyhow::Result;
use rigol::Dho900;
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
