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
