//! The transport against a scripted fake instrument over real TCP.

use std::time::Duration;

use rigol::{Error, Session, SessionConfig};
use rigol_fake::{Fake, Reply, step};

fn cfg(timeout_ms: u64) -> SessionConfig {
    SessionConfig {
        timeout: Duration::from_millis(timeout_ms),
        ..SessionConfig::default()
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

/// A PNG-like payload: binary, and full of the newlines a line read stops at.
fn binary_payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn a_block_survives_one_byte_writes() {
    let payload = binary_payload(4096);
    let fake = Fake::start(vec![step(":DISP:DATA? PNG", Reply::block(&payload, 1))]).await;
    let s = Session::connect(fake.addr(), cfg(2000)).await.unwrap();

    let got = s.query_block(":DISP:DATA? PNG").await.unwrap();

    assert_eq!(got, payload);
    fake.finish();
}

#[tokio::test]
async fn a_query_after_a_block_gets_its_own_reply() {
    let payload = binary_payload(1000);
    let fake = Fake::start(vec![
        step(":WAV:DATA?", Reply::block(&payload, 4096)),
        step(":WAV:PRE?", Reply::line("0,0,1000,1,1e-6,0,0,0.01,0,128")),
    ])
    .await;
    let s = Session::connect(fake.addr(), cfg(1000)).await.unwrap();

    assert_eq!(s.query_block(":WAV:DATA?").await.unwrap(), payload);
    let pre = s.query(":WAV:PRE?").await.unwrap();

    assert_eq!(pre, "0,0,1000,1,1e-6,0,0,0.01,0,128");
    fake.finish();
}

/// The block's newline can arrive after the block was returned. The next
/// query must skip it rather than return it as an empty reply.
#[tokio::test]
async fn a_late_block_terminator_is_skipped() {
    let payload = b"abc\ndef".to_vec();
    let mut bare = format!("#1{}", payload.len()).into_bytes();
    bare.extend_from_slice(&payload);
    let fake = Fake::start(vec![
        step(
            ":DISP:DATA? PNG",
            Reply::Raw {
                bytes: bare,
                chunk: 64,
            },
        ),
        step(
            "*IDN?",
            Reply::Raw {
                bytes: b"\nRIGOL,DHO924S,X,1\n".to_vec(),
                chunk: 64,
            },
        ),
    ])
    .await;
    let s = Session::connect(fake.addr(), cfg(1000)).await.unwrap();

    assert_eq!(s.query_block(":DISP:DATA? PNG").await.unwrap(), payload);
    assert_eq!(s.query("*IDN?").await.unwrap(), "RIGOL,DHO924S,X,1");
    fake.finish();
}

#[tokio::test]
async fn an_oversized_block_is_refused_before_allocation() {
    let fake = Fake::start(vec![step(
        ":WAV:DATA?",
        Reply::Raw {
            bytes: b"#9999999999".to_vec(),
            chunk: 64,
        },
    )])
    .await;
    let small = SessionConfig {
        max_block_bytes: 1024,
        ..cfg(1000)
    };
    let s = Session::connect(fake.addr(), small).await.unwrap();

    let err = s.query_block(":WAV:DATA?").await.unwrap_err();

    assert!(
        matches!(
            err,
            Error::BlockTooLarge {
                len: 999_999_999,
                cap: 1024,
                ..
            }
        ),
        "{err:?}"
    );
    fake.finish();
}

#[tokio::test]
async fn an_indefinite_length_block_is_refused() {
    let fake = Fake::start(vec![step(
        ":WAV:DATA?",
        Reply::Raw {
            bytes: b"#0abc\n".to_vec(),
            chunk: 64,
        },
    )])
    .await;
    let s = Session::connect(fake.addr(), cfg(1000)).await.unwrap();

    let err = s.query_block(":WAV:DATA?").await.unwrap_err();

    assert!(matches!(err, Error::Protocol { .. }), "{err:?}");
    fake.finish();
}

#[tokio::test]
async fn a_text_reply_where_a_block_was_expected_is_a_protocol_error() {
    let fake = Fake::start(vec![step(":DISP:DATA? PNG", Reply::line("0"))]).await;
    let s = Session::connect(fake.addr(), cfg(1000)).await.unwrap();

    let err = s.query_block(":DISP:DATA? PNG").await.unwrap_err();

    assert!(matches!(err, Error::Protocol { .. }), "{err:?}");
    fake.finish();
}
