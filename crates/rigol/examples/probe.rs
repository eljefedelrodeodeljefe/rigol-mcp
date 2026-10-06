//! Read-only probe against a live instrument.
//!
//! Strictly sequential and strictly read-only: no setpoint is written, no output
//! is energised, no acquisition setting is changed. Never turn this into a
//! sweep — an `nmap -p-` wedged a DP832A for ninety seconds.
//!
//! Reads `*IDN?` first and probes according to the family that answers. For
//! an oscilloscope it walks a fixed list of read-only queries from the
//! DHO800/DHO900 Programming Guide, one at a time, draining the error queue
//! after each, and prints a table of what this firmware accepts. An
//! unsupported query on these scopes is not an error reply but silence, so
//! each one is a timeout followed by a reconnect.
//!
//!   cargo run -p rigol --example probe -- <instrument-ip>

use std::time::{Duration, Instant};

use rigol::{DEFAULT_PORT, Dp800, Family, Identity, Limits, Session, SessionConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("RIGOL_HOST").ok());
    let Some(host) = host else {
        eprintln!("usage: probe <instrument-ip>   (or set RIGOL_HOST)");
        std::process::exit(2);
    };

    let session = Session::connect(format!("{host}:{DEFAULT_PORT}"), probe_cfg()).await?;
    let raw = session.query("*IDN?").await?;
    let identity =
        Identity::parse(&raw).ok_or_else(|| format!("unparsable *IDN? reply: {raw:?}"))?;
    match identity.family() {
        Some(Family::Dp800) => probe_psu(&host).await,
        Some(Family::Dho900) => probe_scope(&session, &identity).await,
        None => {
            println!(
                "{raw}\nno device model for `{}` — nothing else sent",
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
];

async fn probe_scope(
    session: &Session,
    identity: &Identity,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "── {} {} ({}) firmware {} ──",
        identity.vendor, identity.model, identity.serial, identity.firmware
    );
    session.clear_status().await?;

    println!("\n| Query | Reply | Error queue after |");
    println!("|---|---|---|");
    for q in SCOPE_QUERIES {
        let t = Instant::now();
        let reply = match session.query(q).await {
            Ok(r) => format!("`{r}` ({} ms)", t.elapsed().as_millis()),
            Err(e) => format!("**no reply**: {e}"),
        };
        let errs = match session.errors().await {
            Ok(v) if v.is_empty() => "clean".to_string(),
            Ok(v) => v
                .iter()
                .map(|e| format!("{} {}", e.code, e.message))
                .collect::<Vec<_>>()
                .join("; "),
            Err(e) => format!("queue unreadable: {e}"),
        };
        println!("| `{q}` | {reply} | {errs} |");
    }

    let t = Instant::now();
    match session.query_block(":DISPlay:DATA? PNG").await {
        Ok(png) => println!(
            "| `:DISPlay:DATA? PNG` | {} bytes, PNG signature {} ({} ms) | — |",
            png.len(),
            png.starts_with(b"\x89PNG"),
            t.elapsed().as_millis()
        ),
        Err(e) => println!("| `:DISPlay:DATA? PNG` | **failed**: {e} | — |"),
    }

    println!("\ndone — queries only; no setting was changed.");
    Ok(())
}

async fn probe_psu(host: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("── connecting to {host} ──");
    let t0 = Instant::now();
    let psu = Dp800::connect(host, None, SessionConfig::default(), Limits::new()).await?;
    println!("connected in {:?}", t0.elapsed());

    let id = psu.identity();
    println!("\n── identity ──");
    println!("  vendor   {}", id.vendor);
    println!("  model    {}", id.model);
    println!("  serial   {}", id.serial);
    println!("  firmware {}", id.firmware);

    println!("\n── channels (rating, setpoint, measured, output) ──");
    for ch in 1..=3u8 {
        let c = psu.channel(ch).await?;
        let m = psu.measure(ch).await?;
        let o = psu.output_state(ch).await?;
        println!(
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

    println!("\n── resolved safety ceilings (backfilled from ratings) ──");
    for ch in 1..=3u8 {
        let r = psu.limits().ceiling(ch)?;
        println!("  CH{ch}  max {:.1}V / {:.1}A", r.volts, r.amps);
    }

    println!("\n── open question 2: protection subsystem ──");
    for ch in 1..=3u8 {
        let p = psu.protection(ch).await?;
        println!(
            "  CH{ch}  ovp={:?} @ {:?}V tripped={:?}   ocp={:?} @ {:?}A tripped={:?}",
            p.ovp_enabled, p.ovp_volts, p.ovp_tripped, p.ocp_enabled, p.ocp_amps, p.ocp_tripped
        );
    }

    println!("\n── LAN subsystem (capability-probed) ──");
    let lan = psu.lan_config().await?;
    println!("  mac       {:?}", lan.mac);
    println!("  ip        {:?}", lan.ip);
    println!("  mask      {:?}", lan.mask);
    println!("  gateway   {:?}", lan.gateway);
    println!("  dns       {:?}", lan.dns);
    println!("  dhcp      {:?}", lan.dhcp);
    println!("  auto-ip   {:?}", lan.auto_ip);
    println!("  manual-ip {:?}", lan.manual_ip);
    println!("  answered  {} of 8 queries", lan.supported_queries.len());

    println!("\n── latency of a single query (open question: timeout budget) ──");
    for i in 1..=3 {
        let t = Instant::now();
        let _ = psu.session().query("*IDN?").await?;
        println!("  *IDN? #{i}  {:?}", t.elapsed());
    }

    println!("\n── error queue ──");
    let errs = psu.errors().await?;
    if errs.is_empty() {
        println!("  clean");
    } else {
        for e in errs {
            println!("  {} {}", e.code, e.message);
        }
    }

    println!("\n── safety check (no write is sent — the library refuses first) ──");
    match psu.set_voltage(3, 30.0).await {
        Err(e) => println!("  30V on CH3 (rated 5V): refused — {e}"),
        Ok(()) => println!("  !! CH3 accepted 30V — the ceiling is not working"),
    }

    println!("\ndone — nothing was written, no output was energised.");
    Ok(())
}
