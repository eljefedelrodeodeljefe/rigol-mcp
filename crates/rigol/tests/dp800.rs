//! The DP800 model against a scripted fake instrument.

use rigol::{Dp800, Error, Family, Limits, SessionConfig};
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
