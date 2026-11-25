use rigol::{Coupling, DigitalChannel, InputImpedance, MeasureItem, Slope, Sweep, Waveshape};
use schemars::JsonSchema;
use serde::Deserialize;

/// Points returned by get_waveform when the caller does not say.
pub const DEFAULT_WAVEFORM_POINTS: usize = 1_000;
/// Upper bound on returned points, whatever the caller asks for.
pub const MAX_WAVEFORM_POINTS: usize = 10_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeasureParams {
    #[schemars(
        description = "What to measure, in DHO800/DHO900 and MHO900 Programming Guide terms"
    )]
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
pub struct SaveWaveformParams {
    #[schemars(description = "Analog input channel, from 1 to the model's channel count")]
    pub channel: u8,
    #[schemars(
        description = "Absolute path on the host running this server, ending in .npz. The JSON sidecar goes beside it with a .json extension. The directory must exist; an existing .npz or .json there is never overwritten"
    )]
    pub path: String,
    #[schemars(
        description = "RAW memory reads need the scope stopped (Programming Guide :WAVeform:MODE). If it is acquiring, stop it with :STOP for the read and restart it afterwards with :RUN (or :SINGle in single sweep). Default false: a running scope is refused. true requires RIGOL_ALLOW_WRITES"
    )]
    pub stop_if_running: Option<bool>,
    #[schemars(
        description = "Refuse unless the channel's input impedance is this (:CHANnel<n>:IMPedance? on an MHO900; a DHO input is always one_megohm). Checked before anything changes"
    )]
    pub expect_impedance: Option<InputImpedance>,
    #[schemars(
        description = "Points per :WAVeform:DATA? block (default 1000000, at most 10000000)"
    )]
    pub chunk_points: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigureChannelParams {
    #[schemars(description = "Analog input channel, from 1 to the model's channel count")]
    pub channel: u8,
    #[schemars(description = "Show or hide the channel (:CHANnel<n>:DISPlay)")]
    pub displayed: Option<bool>,
    #[schemars(
        description = "Vertical scale in volts per division (:CHANnel<n>:SCALe). At a 1x probe: 200 µV to 10 V on a DHO900, 500 µV to 10 V on a DHO800, and on an MHO900 1 mV to 10 V at 1 MΩ input or 200 µV to 1 V at 50 Ω. Multiplied by the probe ratio"
    )]
    pub volts_per_div: Option<f64>,
    #[schemars(
        description = "Vertical offset in volts (:CHANnel<n>:OFFSet). The allowed range depends on the scale; the scope decides and any objection is reported back"
    )]
    pub offset_volts: Option<f64>,
    #[schemars(description = "Input coupling (:CHANnel<n>:COUPling)")]
    pub coupling: Option<Coupling>,
    #[schemars(
        description = "Probe attenuation ratio (:CHANnel<n>:PROBe), e.g. 1 or 10. Must be one of 0.001, 0.002, 0.005 … 1, 2, 5, 10, 20, 50 … 50000; an MHO900 also takes 15, 150, 1500 and 15000. Applied before scale and offset, which it rescales"
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigureLogicAnalyzerParams {
    #[schemars(
        description = "Turn the logic analyser on or off (:LA:ENABle). Turning it on shows D0–D7 again, whatever was shown before"
    )]
    pub enabled: Option<bool>,
    #[schemars(description = "Digital channels to show (:LA:DIGital:ENABle D<n>,1)")]
    pub show: Option<Vec<DigitalChannel>>,
    #[schemars(description = "Digital channels to hide (:LA:DIGital:ENABle D<n>,0)")]
    pub hide: Option<Vec<DigitalChannel>>,
    #[schemars(
        description = "Logic threshold for D0–D7 in volts (:LA:POD1:THReshold), -15 to 15. 1.65 suits 3.3 V logic"
    )]
    pub pod1_threshold_volts: Option<f64>,
    #[schemars(
        description = "Logic threshold for D8–D15 in volts (:LA:POD2:THReshold), -15 to 15"
    )]
    pub pod2_threshold_volts: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GeneratorChannelParams {
    #[schemars(
        description = "Generator channel, 1 or 2 (:SOURce<n>). Needs option MHO900-AFG50 or MHO900-AFG100"
    )]
    pub channel: u8,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigureGeneratorChannelParams {
    #[schemars(
        description = "Generator channel, 1 or 2 (:SOURce<n>). Needs option MHO900-AFG50 or MHO900-AFG100"
    )]
    pub channel: u8,
    #[schemars(
        description = "Waveform (:SOURce<n>:FUNCtion). dc outputs the offset as a constant level"
    )]
    pub shape: Option<Waveshape>,
    #[schemars(
        description = "Frequency in hertz (:SOURce<n>:FREQuency). AFG100: sine 2 mHz–100 MHz, square 2 mHz–20 MHz, ramp 2 mHz–2 MHz. AFG50: sine 20 mHz–50 MHz, square 20 mHz–10 MHz, ramp 20 mHz–1 MHz. Not used by dc or noise"
    )]
    pub frequency_hz: Option<f64>,
    #[schemars(
        description = "Amplitude in volts peak-to-peak (:SOURce<n>:VOLTage:AMPLitude), into the load the channel's output impedance names: HighZ 2 mV–20 V, or 1 mV–10 V into 50 Ω, halved above 50 MHz. Set to 50 Ω, an open circuit sees twice this, and that is what RIGOL_GEN<n>_MAX_VPP bounds"
    )]
    pub amplitude_vpp: Option<f64>,
    #[schemars(
        description = "DC offset in volts (:SOURce<n>:VOLTage:OFFSet), into the same load as the amplitude. The open-circuit |offset| + amplitude/2 must stay within RIGOL_GEN<n>_MAX_PEAK_VOLTS"
    )]
    pub offset_volts: Option<f64>,
}
