use rigol::{Coupling, DigitalChannel, MeasureItem, Slope, Sweep, Waveshape};
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigureChannelParams {
    #[schemars(description = "Analog input channel, from 1 to the model's channel count")]
    pub channel: u8,
    #[schemars(description = "Show or hide the channel (:CHANnel<n>:DISPlay)")]
    pub displayed: Option<bool>,
    #[schemars(
        description = "Vertical scale in volts per division (:CHANnel<n>:SCALe). 200 µV to 10 V at a 1x probe on a DHO900 (500 µV on a DHO800), multiplied by the probe ratio"
    )]
    pub volts_per_div: Option<f64>,
    #[schemars(
        description = "Vertical offset in volts (:CHANnel<n>:OFFSet). The allowed range depends on the scale; the scope decides and any objection is reported back"
    )]
    pub offset_volts: Option<f64>,
    #[schemars(description = "Input coupling (:CHANnel<n>:COUPling)")]
    pub coupling: Option<Coupling>,
    #[schemars(
        description = "Probe attenuation ratio (:CHANnel<n>:PROBe), e.g. 1 or 10. Must be one of 0.001, 0.002, 0.005 … 1, 2, 5, 10, 20, 50 … 50000. Applied before scale and offset, which it rescales"
    )]
    pub probe_ratio: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigureTimebaseParams {
    #[schemars(description = "Horizontal scale in seconds per division (:TIMebase:MAIN:SCALe)")]
    pub seconds_per_div: Option<f64>,
    #[schemars(
        description = "Horizontal offset in seconds relative to the trigger (:TIMebase:MAIN:OFFSet)"
    )]
    pub offset_seconds: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigureEdgeTriggerParams {
    #[schemars(
        description = "Analog channel to trigger on (:TRIGger:EDGE:SOURce CHANnel<n>). Give this or digital, not both"
    )]
    pub channel: Option<u8>,
    #[schemars(
        description = "Digital channel D0–D15 to trigger on (:TRIGger:EDGE:SOURce D<n>), DHO900 series only. The trigger level then sets the switching threshold for that edge, -20 V to 20 V. Give this or channel, not both"
    )]
    pub digital: Option<DigitalChannel>,
    #[schemars(description = "Edge to trigger on (:TRIGger:EDGE:SLOPe)")]
    pub slope: Option<Slope>,
    #[schemars(description = "Trigger level in volts (:TRIGger:EDGE:LEVel)")]
    pub level_volts: Option<f64>,
    #[schemars(
        description = "auto free-runs without a trigger, normal waits for one, single acquires once and stops (:TRIGger:SWEep)"
    )]
    pub sweep: Option<Sweep>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigureGeneratorParams {
    #[schemars(
        description = "Waveform (:SOURce:FUNCtion). dc outputs the offset as a constant level"
    )]
    pub shape: Option<Waveshape>,
    #[schemars(
        description = "Frequency in hertz (:SOURce:FREQuency): sine 2 mHz–25 MHz, square 2 mHz–15 MHz, ramp 2 mHz–150 kHz; not used by dc or noise"
    )]
    pub frequency_hz: Option<f64>,
    #[schemars(
        description = "Amplitude in volts peak-to-peak (:SOURce:VOLTage:AMPLitude): 2 mV–10 V, 5 V above 10 MHz, and never above RIGOL_GEN_MAX_VPP"
    )]
    pub amplitude_vpp: Option<f64>,
    #[schemars(
        description = "DC offset in volts (:SOURce:VOLTage:OFFSet). |offset| + amplitude/2 must stay within RIGOL_GEN_MAX_PEAK_VOLTS"
    )]
    pub offset_volts: Option<f64>,
}
