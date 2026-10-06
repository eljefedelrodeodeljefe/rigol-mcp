use rigol::MeasureItem;
use schemars::JsonSchema;
use serde::Deserialize;

/// Points returned by get_waveform when the caller does not say.
pub const DEFAULT_WAVEFORM_POINTS: usize = 1_000;
/// Upper bound on returned points, whatever the caller asks for.
pub const MAX_WAVEFORM_POINTS: usize = 10_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeasureParams {
    #[schemars(description = "What to measure, in DHO800/DHO900 Programming Guide terms")]
    pub item: MeasureItem,
    #[schemars(description = "Analog input channel, from 1 to the model's channel count")]
    pub channel: u8,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaveformParams {
    #[schemars(description = "Analog input channel, from 1 to the model's channel count")]
    pub channel: u8,
    #[schemars(
        description = "Most points to return (default 1000, capped at 10000). Longer captures are thinned to min/max pairs per bucket, so peaks survive; the statistics always cover every sample"
    )]
    pub max_points: Option<usize>,
}
