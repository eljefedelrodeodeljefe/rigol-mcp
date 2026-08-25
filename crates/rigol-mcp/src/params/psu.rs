use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChannelParams {
    #[schemars(description = "Output channel: 1, 2 or 3 (CH3 is the 5V/3A rail)")]
    pub channel: u8,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetVoltageParams {
    #[schemars(description = "Output channel: 1, 2 or 3")]
    pub channel: u8,
    #[schemars(
        description = "Voltage setpoint in volts. Refused if it exceeds the configured ceiling for this channel. Does NOT energise the output — call output_on for that."
    )]
    pub volts: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetCurrentParams {
    #[schemars(description = "Output channel: 1, 2 or 3")]
    pub channel: u8,
    #[schemars(
        description = "Current limit in amps. Refused if it exceeds the configured ceiling for this channel."
    )]
    pub amps: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetProtectionParams {
    #[schemars(description = "Output channel: 1, 2 or 3")]
    pub channel: u8,
    #[schemars(description = "Over-voltage protection trip level in volts (OVP)")]
    pub ovp_volts: Option<f64>,
    #[schemars(description = "Over-current protection trip level in amps (OCP)")]
    pub ocp_amps: Option<f64>,
}
