//! The DP800 model against a scripted fake instrument.

use std::time::{Duration, Instant};

use rigol::{DP800_COMMAND_SETTLE, Dp800, Error, Family, Limits, Session, SessionConfig};
use rigol_fake::{Fake, Reply, step};

/// Pointed at a scope by mistake, the PSU model must stop at `*IDN?`. The fake
/// fails the test if any DP800 command (`:APPL? CH1` …) follows.
#[tokio::test]
async fn refuses_another_family_before_sending_anything_else() {
    let fake = Fake::start(vec![step(
        "*IDN?",
        Reply::line("RIGOL TECHNOLOGIES,DHO924S,DHO9S000000001,00.01.05"),
    )])
    .await;

    let host = fake.addr.ip().to_string();
    let result = Dp800::connect(
        &host,
        Some(fake.addr.port()),
        SessionConfig::default(),
        Limits::new(),
    )
    .await;

    match result {
        Err(Error::WrongInstrument { expected, model }) => {
            assert_eq!(expected, Family::Dp800);
            assert_eq!(model, "DHO924S");
        }
        Err(e) => panic!("expected WrongInstrument, got {e:?}"),
        Ok(_) => panic!("expected WrongInstrument, got a connection"),
    }
    fake.finish();
}

const IDN: &str = "RIGOL TECHNOLOGIES,DP832A,DP8A000000001,00.01.19";

/// Connects with a 3.6 V / 0.5 A ceiling on channel 1; the other channels
/// keep their ratings.
async fn connect(script: Vec<rigol_fake::Step>) -> (Fake, Dp800) {
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

/// A setpoint can be above the ceiling without this server having set it:
/// from the front panel, or before the server started. Output-on must
/// refuse it rather than energise it. The fake fails the test if
/// `:OUTP CH1,ON` is sent; the trailing `:OUTP?` makes sure it would have
/// been seen.
#[tokio::test]
async fn output_on_refuses_setpoints_over_the_ceiling() {
    for setpoint in ["CH1:30V/3A,12.000,0.100", "CH1:30V/3A,3.300,3.000"] {
        let (fake, psu) = connect(vec![
            step(":APPL? CH1", Reply::line(setpoint)),
            step(":OUTP? CH1", Reply::line("OFF")),
        ])
        .await;

        let err = psu.output_on(1).await.unwrap_err();
        assert!(
            matches!(err, Error::LimitExceeded { channel: 1, .. }),
            "{setpoint}: {err:?}"
        );
        assert!(!psu.output_state(1).await.unwrap().is_on());
        fake.finish();
    }
}

#[tokio::test]
async fn output_on_within_the_ceiling_energises() {
    let (fake, psu) = connect(vec![
        step(":APPL? CH1", Reply::line("CH1:30V/3A,3.300,0.100")),
        step(":OUTP CH1,ON", Reply::None),
        step(":OUTP? CH1", Reply::line("ON")),
    ])
    .await;

    psu.output_on(1).await.unwrap();

    assert!(psu.output_state(1).await.unwrap().is_on());
    fake.finish();
}

/// A DP832A on firmware `00.01.21`, reduced to what the command-settle fix is
/// about: a query that arrives within 30 ms of a command that changed a
/// setpoint gets a bare `Command error` with no newline, as on the real unit.
/// Only channel 1's voltage is modelled; the other channels answer with their
/// ratings.
fn busy_after_change() -> impl FnMut(&str) -> Option<Reply> + Send + 'static {
    const BUSY: Duration = Duration::from_millis(30);
    let mut volts = 0.0_f64;
    let mut changed_at: Option<Instant> = None;
    move |cmd| {
        let busy = changed_at.is_some_and(|t| t.elapsed() < BUSY);
        if cmd.contains('?') && busy {
            return Some(Reply::Raw {
                bytes: b"Command error".to_vec(),
                chunk: 64,
            });
        }
        match cmd {
            "*IDN?" => Some(Reply::line(IDN)),
            ":APPL? CH1" => Some(Reply::Line(format!("CH1:30V/3A,{volts:.3},3.000"))),
            ":APPL? CH2" => Some(Reply::line("CH2:30V/3A,0.000,3.000")),
            ":APPL? CH3" => Some(Reply::line("CH3:5V/3A,0.000,3.000")),
            _ => {
                let value: f64 = cmd.strip_prefix(":SOUR1:VOLT ")?.parse().ok()?;
                if value != volts {
                    volts = value;
                    changed_at = Some(Instant::now());
                }
                Some(Reply::None)
            }
        }
    }
}

fn short_timeout() -> SessionConfig {
    SessionConfig {
        timeout: Duration::from_millis(400),
        ..SessionConfig::default()
    }
}

/// The failure the settle exists for: without it, the read-back right after
/// a setpoint change times out, although the change was applied. With
/// [`DP800_COMMAND_SETTLE`] the same exchange is answered.
#[tokio::test]
async fn a_query_right_after_a_change_needs_the_settle() {
    let fake = Fake::start_with(busy_after_change()).await;
    let bare = Session::connect(fake.addr(), short_timeout())
        .await
        .expect("connect");
    bare.command(":SOUR1:VOLT 1").await.expect("command");
    match bare.query(":APPL? CH1").await {
        Err(Error::Timeout { cmd, .. }) => assert_eq!(cmd, ":APPL? CH1"),
        other => panic!("expected a timeout, got {other:?}"),
    }

    let settled = Session::connect(fake.addr(), short_timeout())
        .await
        .expect("connect")
        .with_command_settle(DP800_COMMAND_SETTLE);
    settled.command(":SOUR1:VOLT 2").await.expect("command");
    let reply = settled.query(":APPL? CH1").await.expect("read-back");
    assert_eq!(reply, "CH1:30V/3A,2.000,3.000");
    fake.finish();
}

/// `Dp800` waits [`DP800_COMMAND_SETTLE`] after each command, so a write and
/// its read-back both succeed, which is what every PSU write tool does.
#[tokio::test]
async fn dp800_reads_back_a_changed_setpoint() {
    let fake = Fake::start_with(busy_after_change()).await;
    let psu = Dp800::connect(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        short_timeout(),
        Limits::new(),
    )
    .await
    .expect("connect");

    for volts in [1.0, 2.5, 0.0] {
        psu.set_voltage(1, volts).await.expect("set_voltage");
        let channel = psu.channel(1).await.expect("read-back after a change");
        assert!((channel.setpoint.volts - volts).abs() < 1e-9);
    }
    fake.finish();
}

/// Protection on/off is the bare `:OUTP:OVP` / `:OUTP:OCP` state command, the
/// abbreviated form this firmware accepts. The trailing query proves nothing
/// else was sent in between.
#[tokio::test]
async fn protection_on_off_sends_the_state_commands() {
    let (fake, psu) = connect(vec![
        step(":OUTP:OVP CH1,ON", Reply::None),
        step(":OUTP:OCP CH1,ON", Reply::None),
        step(":OUTP:OVP CH2,OFF", Reply::None),
        step(":OUTP:OCP CH3,OFF", Reply::None),
        step(":OUTP? CH1", Reply::line("OFF")),
    ])
    .await;

    psu.set_ovp_enabled(1, true).await.expect("ovp on");
    psu.set_ocp_enabled(1, true).await.expect("ocp on");
    psu.set_ovp_enabled(2, false).await.expect("ovp off");
    psu.set_ocp_enabled(3, false).await.expect("ocp off");
    psu.output_state(1).await.expect("output state");
    fake.finish();
}

/// A channel that does not exist is refused before anything is sent.
#[tokio::test]
async fn protection_on_off_refuses_a_channel_that_does_not_exist() {
    let (fake, psu) = connect(vec![step(":OUTP? CH1", Reply::line("OFF"))]).await;

    assert!(psu.set_ovp_enabled(4, true).await.is_err());
    assert!(psu.set_ocp_enabled(0, true).await.is_err());
    psu.output_state(1).await.expect("output state");
    fake.finish();
}
