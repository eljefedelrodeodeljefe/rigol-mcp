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
    protection_report(psu, p.channel).await
}

/// The channel's protection, plus a warning for protection that is on but
/// trips above the channel's ceiling: it is armed, yet cannot fire before a
/// setpoint this server would refuse. The factory OVP level, 33 V on a 30 V
/// channel, is one.
async fn protection_report(psu: &Dp800, channel: u8) -> Result<Value> {
    let prot = psu.protection(channel).await?;
    let ceiling = psu.limits().ceiling(channel)?;
    let mut warnings = Vec::new();
    if let (Some(true), Some(v)) = (prot.ovp_enabled, prot.ovp_volts)
        && v > ceiling.volts()
    {
        warnings.push(format!(
            "OVP is on but trips at {v} V, above this channel's {} V ceiling",
            ceiling.volts()
        ));
    }
    if let (Some(true), Some(a)) = (prot.ocp_enabled, prot.ocp_amps)
        && a > ceiling.amps()
    {
        warnings.push(format!(
            "OCP is on but trips at {a} A, above this channel's {} A ceiling",
            ceiling.amps()
        ));
    }
    let mut out = serde_json::to_value(prot)?;
    out["warnings"] = json!(warnings);
    Ok(out)
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
    // Levels first: protection switched on by this call never arms at a stale
    // trip level.
    if let Some(on) = p.ovp_enabled {
        psu.set_ovp_enabled(p.channel, on).await?;
    }
    if let Some(on) = p.ocp_enabled {
        psu.set_ocp_enabled(p.channel, on).await?;
    }
    protection_report(psu, p.channel).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use rigol::{Limits, SessionConfig};
    use rigol_fake::{Fake, Reply, Step, step};

    const IDN: &str = "RIGOL TECHNOLOGIES,DP832A,DP8A000000001,00.01.21";

    /// Channel 1 has a 3.6 V / 0.5 A ceiling; the others keep their ratings.
    async fn connect(script: Vec<Step>) -> (Fake, Dp800) {
        let mut full = vec![
            step("*IDN?", Reply::line(IDN)),
            step(":APPL? CH1", Reply::line("CH1:30V/3A,0.000,0.100")),
            step(":APPL? CH2", Reply::line("CH2:30V/3A,0.000,3.000")),
            step(":APPL? CH3", Reply::line("CH3:5V/3A,0.000,3.000")),
        ];
        full.extend(script);
        let fake = Fake::start(full).await;
        let psu = Dp800::connect(
            &fake.addr.ip().to_string(),
            Some(fake.addr.port()),
            SessionConfig::default(),
            Limits::new().with_channel(1, 3.6, 0.5).unwrap(),
        )
        .await
        .expect("connect");
        (fake, psu)
    }

    /// The six reads `Dp800::protection` makes, in its order.
    fn protection_reads(ovp: (&str, &str), ocp: (&str, &str)) -> Vec<Step> {
        vec![
            step(":OUTP:OVP? CH1", Reply::line(ovp.0)),
            step(":OUTP:OVP:VAL? CH1", Reply::line(ovp.1)),
            step(":OUTP:OVP:ALAR? CH1", Reply::line("NO")),
            step(":OUTP:OCP? CH1", Reply::line(ocp.0)),
            step(":OUTP:OCP:VAL? CH1", Reply::line(ocp.1)),
            step(":OUTP:OCP:ALAR? CH1", Reply::line("NO")),
        ]
    }

    /// The trip level goes out before protection is switched on, so a call
    /// that does both never arms at the old level. The script fails the test
    /// on any other order.
    #[tokio::test]
    async fn set_protection_sets_levels_before_switching_on() {
        let mut script = vec![
            step(":OUTP:OVP:VAL CH1,3.3", Reply::None),
            step(":OUTP:OCP:VAL CH1,0.2", Reply::None),
            step(":OUTP:OVP CH1,ON", Reply::None),
            step(":OUTP:OCP CH1,ON", Reply::None),
        ];
        script.extend(protection_reads(("ON", "3.300"), ("ON", "0.200")));
        let (fake, psu) = connect(script).await;

        let out = set_protection(
            &psu,
            &SetProtectionParams {
                channel: 1,
                ovp_volts: Some(3.3),
                ocp_amps: Some(0.2),
                ovp_enabled: Some(true),
                ocp_enabled: Some(true),
            },
        )
        .await
        .expect("set_protection");

        assert_eq!(out["ovp_enabled"], json!(true));
        assert_eq!(out["ocp_enabled"], json!(true));
        assert_eq!(out["warnings"], json!([]));
        fake.finish();
    }

    /// The factory OVP level is 33 V. Armed on a channel with a 3.6 V ceiling it
    /// cannot fire before a setpoint this server would refuse; say so. A
    /// disarmed level above the ceiling is not a warning.
    #[tokio::test]
    async fn armed_protection_above_the_ceiling_is_a_warning() {
        let (fake, psu) = connect(protection_reads(("ON", "33.000"), ("OFF", "3.300"))).await;

        let out = protection(&psu, &ChannelParams { channel: 1 })
            .await
            .expect("protection");

        let warnings = out["warnings"].as_array().expect("warnings");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let w = warnings[0].as_str().unwrap();
        assert!(
            w.contains("OVP") && w.contains("33") && w.contains("3.6"),
            "{w}"
        );
        fake.finish();
    }
}
