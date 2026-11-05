//! Read-only probe against a live instrument.
//!
//! Strictly sequential and, by default, strictly read-only: no setpoint is
//! written, no output is energised, no acquisition setting is changed. Never
//! turn this into a sweep — an `nmap -p-` wedged a DP832A for ninety seconds.
//!
//! Reads `*IDN?` first and probes according to the family that answers. For
//! an oscilloscope it walks a fixed list of read-only queries from the
//! DHO800/DHO900 Programming Guide, one at a time, draining the error queue
//! after each, and prints a table of what this firmware accepts. An
//! unsupported query on these scopes is not an error reply but silence, so
//! each one is a timeout followed by a reconnect.
//!
//! With `--write`, and only for an oscilloscope, it then checks a fixed list
//! of display, trigger and logic-analyser set commands, again in Programming
//! Guide spelling: read the setting, set it, read it back, drain the error
//! queue, and finally put every setting back as it was. Nothing in that list
//! touches the signal generator.
//!
//!   cargo run -p rigol --example probe -- <instrument-ip> [--write]

use std::time::{Duration, Instant};

use tracing_subscriber::EnvFilter;

use rigol::{DEFAULT_PORT, Dp800, Family, Identity, Limits, Session, SessionConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The report is log output: `RUST_LOG=warn` keeps only refusals and failures.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .without_time()
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.iter().any(|a| a == "--write");
    let host = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .or_else(|| std::env::var("RIGOL_HOST").ok());
    let Some(host) = host else {
        tracing::error!("usage: probe <instrument-ip> [--write]   (or set RIGOL_HOST)");
        std::process::exit(2);
    };

    let session = Session::connect(format!("{host}:{DEFAULT_PORT}"), probe_cfg()).await?;
    let raw = session.query("*IDN?").await?;
    let identity =
        Identity::parse(&raw).ok_or_else(|| format!("unparsable *IDN? reply: {raw:?}"))?;
    match identity.family() {
        Some(Family::Dp800) => probe_psu(&host).await,
        Some(Family::Dho900) => {
            probe_scope(&session, &identity).await?;
            if write {
                probe_scope_writes(&session).await?;
            }
            Ok(())
        }
        None => {
            tracing::info!(
                "{raw}: no device model for `{}` — nothing else sent",
                identity.model
            );
            Ok(())
        }
    }
}

fn probe_cfg() -> SessionConfig {
    SessionConfig {
        timeout: Duration::from_secs(2),
        ..SessionConfig::default()
    }
}

/// Read-only DHO800/DHO900 queries, in Programming Guide spelling. Nothing
/// here changes instrument state: every entry is a query.
const SCOPE_QUERIES: &[&str] = &[
    ":LAN:STATus?",
    ":LAN:DHCP?",
    ":LAN:IPADdress?",
    ":LAN:SMASk?",
    ":LAN:GATeway?",
    ":LAN:MAC?",
    ":TRIGger:STATus?",
    ":TRIGger:MODE?",
    ":TRIGger:SWEep?",
    ":TRIGger:EDGE:SOURce?",
    ":TRIGger:EDGE:SLOPe?",
    ":TRIGger:EDGE:LEVel?",
    ":TIMebase:MAIN:SCALe?",
    ":TIMebase:MAIN:OFFSet?",
    ":ACQuire:SRATe?",
    ":ACQuire:MDEPth?",
    ":ACQuire:TYPE?",
    ":CHANnel1:DISPlay?",
    ":CHANnel1:SCALe?",
    ":CHANnel1:OFFSet?",
    ":CHANnel1:COUPling?",
    ":CHANnel1:PROBe?",
    ":CHANnel1:BWLimit?",
    ":CHANnel1:INVert?",
    ":MEASure:ITEM? VPP,CHANnel1",
    ":MEASure:ITEM? FREQuency,CHANnel1",
    ":MEASure:ITEM? VRMS,CHANnel1",
    ":WAVeform:SOURce?",
    ":WAVeform:MODE?",
    ":WAVeform:FORMat?",
    ":WAVeform:POINts?",
    ":WAVeform:PREamble?",
    ":SOURce:OUTPut:STATe?",
    ":SOURce:FUNCtion?",
    ":SOURce:FREQuency?",
    ":SOURce:VOLTage:AMPLitude?",
    ":SOURce:VOLTage:OFFSet?",
    ":MEASure:STATistic:DISPlay?",
    ":MEASure:STATistic:COUNt?",
    ":MEASure:STATistic:ITEM? CURRent,VPP,CHANnel1",
    ":MEASure:STATistic:ITEM? MINimum,VPP,CHANnel1",
    ":MEASure:STATistic:ITEM? MAXimum,VPP,CHANnel1",
    ":MEASure:STATistic:ITEM? AVERages,VPP,CHANnel1",
    ":MEASure:STATistic:ITEM? DEViation,VPP,CHANnel1",
    ":MEASure:STATistic:ITEM? CNT,VPP,CHANnel1",
    ":LA:ENABle?",
    ":LA:DIGital:ENABle? D0",
    ":LA:DIGital:ENABle? D15",
    ":LA:POD1:DISPlay?",
    ":LA:POD2:DISPlay?",
    ":LA:POD1:THReshold?",
    ":LA:POD2:THReshold?",
];

/// One set command to check with `--write`: `query` reads the setting before
/// and after, `set` changes it, and `restore` followed by the value read
/// before puts it back. Without a query the command is only sent and the
/// error queue checked. `volts_e18` marks a reply in volts times 1e18, as
/// this firmware gives for logic-analyser thresholds (1.65 V reads back as
/// `1.650000E+18`); it is scaled back before being sent as the restore.
struct WriteCheck {
    set: &'static str,
    query: Option<&'static str>,
    restore: &'static str,
    volts_e18: bool,
}

/// Display, trigger and logic-analyser settings only, in Programming Guide
/// spelling. The signal generator is deliberately absent, and so is
/// `:LA:ENABle 0`: turning the analyser back on shows D0–D7 only, so the
/// probe could not put back which channels were shown.
const SCOPE_WRITES: &[WriteCheck] = &[
    WriteCheck {
        set: ":MEASure:STATistic:RESet",
        query: None,
        restore: "",
        volts_e18: false,
    },
    WriteCheck {
        set: ":LA:DIGital:ENABle D12,0",
        query: Some(":LA:DIGital:ENABle? D12"),
        restore: ":LA:DIGital:ENABle D12,",
        volts_e18: false,
    },
    WriteCheck {
        set: ":LA:POD1:THReshold 2.5",
        query: Some(":LA:POD1:THReshold?"),
        restore: ":LA:POD1:THReshold ",
        volts_e18: true,
    },
    WriteCheck {
        set: ":LA:POD2:THReshold 2.5",
        query: Some(":LA:POD2:THReshold?"),
        restore: ":LA:POD2:THReshold ",
        volts_e18: true,
    },
    WriteCheck {
        set: ":TRIGger:EDGE:SOURce D0",
        query: Some(":TRIGger:EDGE:SOURce?"),
        restore: ":TRIGger:EDGE:SOURce ",
        volts_e18: false,
    },
];

async fn probe_scope(
    session: &Session,
    identity: &Identity,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        "── {} {} ({}) firmware {} ──",
        identity.vendor,
        identity.model,
        identity.serial,
        identity.firmware
    );
    session.clear_status().await?;

    tracing::info!("| Query | Reply | Error queue after |");
    tracing::info!("|---|---|---|");
    for q in SCOPE_QUERIES {
        let t = Instant::now();
        let reply = match session.query(q).await {
            Ok(r) => format!("`{r}` ({} ms)", t.elapsed().as_millis()),
            Err(e) => format!("**no reply**: {e}"),
        };
        let errs = describe_errors(session).await;
        tracing::info!("| `{q}` | {reply} | {errs} |");
    }

    let t = Instant::now();
    match session.query_block(":DISPlay:DATA? PNG").await {
        Ok(png) => tracing::info!(
            "| `:DISPlay:DATA? PNG` | {} bytes, PNG signature {} ({} ms) | — |",
            png.len(),
            png.starts_with(b"\x89PNG"),
            t.elapsed().as_millis()
        ),
        Err(e) => tracing::info!("| `:DISPlay:DATA? PNG` | **failed**: {e} | — |"),
    }

    tracing::info!("done — queries only; no setting was changed.");
    Ok(())
}

async fn probe_scope_writes(session: &Session) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("── --write: set, read back, restore ──");
    session.clear_status().await?;
    tracing::info!("| Command | Before | After | Error queue after |");
    tracing::info!("|---|---|---|---|");
    let mut undo = Vec::new();
    for c in SCOPE_WRITES {
        let before = match c.query {
            Some(q) => Some(session.query(q).await?),
            None => None,
        };
        session.command(c.set).await?;
        let after = match c.query {
            Some(q) => Some(session.query(q).await?),
            None => None,
        };
        let errs = describe_errors(session).await;
        tracing::info!(
            "| `{}` | {} | {} | {errs} |",
            c.set,
            show(before.as_deref()),
            show(after.as_deref())
        );
        if let Some(b) = before {
            let value = match b.trim().parse::<f64>() {
                Ok(v) if c.volts_e18 && v.abs() > 1e3 => (v / 1e18).to_string(),
                _ => b,
            };
            undo.push(format!("{}{value}", c.restore));
        }
    }

    tracing::info!("| Restore | Error queue after |");
    tracing::info!("|---|---|");
    for cmd in undo.iter().rev() {
        session.command(cmd).await?;
        let errs = describe_errors(session).await;
        tracing::info!("| `{cmd}` | {errs} |");
    }
    tracing::info!("done — every setting checked was put back.");
    Ok(())
}

fn show(reply: Option<&str>) -> String {
    reply.map_or_else(|| "—".to_string(), |r| format!("`{r}`"))
}

async fn describe_errors(session: &Session) -> String {
    match session.errors().await {
        Ok(v) if v.is_empty() => "clean".to_string(),
        Ok(v) => v
            .iter()
            .map(|e| format!("{} {}", e.code, e.message))
            .collect::<Vec<_>>()
            .join("; "),
        Err(e) => format!("queue unreadable: {e}"),
    }
}

async fn probe_psu(host: &str) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("── connecting to {host} ──");
    let t0 = Instant::now();
    let psu = Dp800::connect(host, None, SessionConfig::default(), Limits::new()).await?;
    tracing::info!("connected in {:?}", t0.elapsed());

    let id = psu.identity();
    tracing::info!("── identity ──");
    tracing::info!("  vendor   {}", id.vendor);
    tracing::info!("  model    {}", id.model);
    tracing::info!("  serial   {}", id.serial);
    tracing::info!("  firmware {}", id.firmware);

    tracing::info!("── channels (rating, setpoint, measured, output) ──");
    for ch in 1..=3u8 {
        let c = psu.channel(ch).await?;
        let m = psu.measure(ch).await?;
        let o = psu.output_state(ch).await?;
        tracing::info!(
            "  CH{ch}  rated {:>5.1}V/{:.1}A   set {:>6.3}V/{:.3}A   meas {:>6.4}V/{:.4}A/{:.3}W   output {:?}",
            c.rating.volts,
            c.rating.amps,
            c.setpoint.volts,
            c.setpoint.amps,
            m.volts,
            m.amps,
            m.watts,
            o
        );
    }

    tracing::info!("── resolved safety ceilings (backfilled from ratings) ──");
    for ch in 1..=3u8 {
        let r = psu.limits().ceiling(ch)?;
        tracing::info!("  CH{ch}  max {:.1}V / {:.1}A", r.volts, r.amps);
    }

    tracing::info!("── open question 2: protection subsystem ──");
    for ch in 1..=3u8 {
        let p = psu.protection(ch).await?;
        tracing::info!(
            "  CH{ch}  ovp={:?} @ {:?}V tripped={:?}   ocp={:?} @ {:?}A tripped={:?}",
            p.ovp_enabled,
            p.ovp_volts,
            p.ovp_tripped,
            p.ocp_enabled,
            p.ocp_amps,
            p.ocp_tripped
        );
    }

    tracing::info!("── LAN subsystem (capability-probed) ──");
    let lan = psu.lan_config().await?;
    tracing::info!("  mac       {:?}", lan.mac);
    tracing::info!("  ip        {:?}", lan.ip);
    tracing::info!("  mask      {:?}", lan.mask);
    tracing::info!("  gateway   {:?}", lan.gateway);
    tracing::info!("  dns       {:?}", lan.dns);
    tracing::info!("  dhcp      {:?}", lan.dhcp);
    tracing::info!("  auto-ip   {:?}", lan.auto_ip);
    tracing::info!("  manual-ip {:?}", lan.manual_ip);
    tracing::info!("  answered  {} of 8 queries", lan.supported_queries.len());

    tracing::info!("── latency of a single query (open question: timeout budget) ──");
    for i in 1..=3 {
        let t = Instant::now();
        let _ = psu.session().query("*IDN?").await?;
        tracing::info!("  *IDN? #{i}  {:?}", t.elapsed());
    }

    tracing::info!("── error queue ──");
    let errs = psu.errors().await?;
    if errs.is_empty() {
        tracing::info!("  clean");
    } else {
        for e in errs {
            tracing::info!("  {} {}", e.code, e.message);
        }
    }

    tracing::info!("── safety check (no write is sent — the library refuses first) ──");
    match psu.set_voltage(3, 30.0).await {
        Err(e) => tracing::info!("  30V on CH3 (rated 5V): refused — {e}"),
        Ok(()) => tracing::error!("  CH3 accepted 30V — the ceiling is not working"),
    }

    tracing::info!("done — nothing was written, no output was energised.");
    Ok(())
}
