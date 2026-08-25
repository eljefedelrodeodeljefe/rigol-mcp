//! Read-only probe against a live instrument.
//!
//! Strictly sequential and strictly read-only: no setpoint is written, no output
//! is energised. Never turn this into a sweep — an `nmap -p-` wedged this unit
//! for ninety seconds.
//!
//!   cargo run -p rigol --example probe -- <instrument-ip>

use std::time::Instant;

use rigol::{Dp800, Limits, SessionConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("RIGOL_HOST").ok());
    let Some(host) = host else {
        eprintln!("usage: probe <instrument-ip>   (or set RIGOL_HOST)");
        std::process::exit(2);
    };

    println!("── connecting to {host} ──");
    let t0 = Instant::now();
    let psu = Dp800::connect(&host, None, SessionConfig::default(), Limits::new()).await?;
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
