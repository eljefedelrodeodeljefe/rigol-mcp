//! The transport against a scripted fake instrument over real TCP.

use std::time::Duration;

use rigol::{Error, Session, SessionConfig};
use rigol_fake::{Fake, Reply, step};

fn cfg(timeout_ms: u64) -> SessionConfig {
    SessionConfig {
        timeout: Duration::from_millis(timeout_ms),
    }
}

#[tokio::test]
async fn query_reads_one_line() {
    let fake = Fake::start(vec![step(
        "*IDN?",
        Reply::line("RIGOL TECHNOLOGIES,DP832A,DP8A000000001,00.01.19"),
    )])
    .await;
    let s = Session::connect(fake.addr(), cfg(1000)).await.unwrap();

    let idn = s.query("*IDN?").await.unwrap();

    assert_eq!(idn, "RIGOL TECHNOLOGIES,DP832A,DP8A000000001,00.01.19");
    fake.finish();
}

/// The reply to a timed-out query can still arrive. Read on the same socket,
/// it would be taken as the answer to the *next* query, and every reply after
/// it would be off by one. The session must reconnect instead.
#[tokio::test]
async fn a_late_reply_is_never_paired_with_the_next_query() {
    let fake = Fake::start(vec![
        step(
            ":MEAS:VOLT? CH1",
            Reply::LateLine(Duration::from_millis(300), "LATE".into()),
        ),
        step(":MEAS:CURR? CH1", Reply::line("0.250")),
    ])
    .await;
    let s = Session::connect(fake.addr(), cfg(100)).await.unwrap();

    let first = s.query(":MEAS:VOLT? CH1").await;
    assert!(matches!(first, Err(Error::Timeout { .. })), "{first:?}");

    // Let the late reply land in the old socket's buffer before asking again.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let second = s.query(":MEAS:CURR? CH1").await.unwrap();

    assert_eq!(second, "0.250");
    assert_eq!(
        fake.connections(),
        2,
        "the poisoned socket should be replaced"
    );
    fake.finish();
}

#[tokio::test]
async fn a_healthy_session_keeps_its_connection() {
    let fake = Fake::start(vec![
        step(":OUTP? CH1", Reply::line("OFF")),
        step(":OUTP? CH2", Reply::line("ON")),
        step("*CLS", Reply::None),
        step(":OUTP? CH3", Reply::line("OFF")),
    ])
    .await;
    let s = Session::connect(fake.addr(), cfg(1000)).await.unwrap();

    assert_eq!(s.query(":OUTP? CH1").await.unwrap(), "OFF");
    assert_eq!(s.query(":OUTP? CH2").await.unwrap(), "ON");
    s.clear_status().await.unwrap();
    assert_eq!(s.query(":OUTP? CH3").await.unwrap(), "OFF");

    assert_eq!(fake.connections(), 1);
    fake.finish();
}

#[tokio::test]
async fn the_error_queue_drains_until_no_error() {
    let fake = Fake::start(vec![
        step(":SYST:ERR?", Reply::line(r#"-113,"Undefined header""#)),
        step(":SYST:ERR?", Reply::line(r#"-100,"Command err""#)),
        step(":SYST:ERR?", Reply::line(r#"0,"No error""#)),
    ])
    .await;
    let s = Session::connect(fake.addr(), cfg(1000)).await.unwrap();

    let errors = s.errors().await.unwrap();

    let codes: Vec<i32> = errors.iter().map(|e| e.code).collect();
    assert_eq!(codes, [-113, -100]);
    fake.finish();
}
