//! The DHO900 model against a scripted fake instrument, using replies
//! captured from a DHO924S on firmware 00.01.05.

use rigol::{Coupling, Dho900, Error, Family, MeasureItem, SessionConfig};
use rigol_fake::{Fake, Reply, Step, step};

const IDN: &str = "RIGOL TECHNOLOGIES,DHO924S,DHO9S000000001,00.01.05";

async fn connect(mut script: Vec<Step>) -> (Fake, Dho900) {
    script.insert(0, step("*IDN?", Reply::line(IDN)));
    let fake = Fake::start(script).await;
    let scope = Dho900::connect(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
    )
    .await
    .expect("connect");
    (fake, scope)
}

#[tokio::test]
async fn refuses_a_power_supply_before_sending_anything_else() {
    let fake = Fake::start(vec![step(
        "*IDN?",
        Reply::line("RIGOL TECHNOLOGIES,DP832A,DP8A000000001,00.01.19"),
    )])
    .await;

    let result = Dho900::connect(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
    )
    .await;

    match result {
        Err(Error::WrongInstrument { expected, model }) => {
            assert_eq!(expected, Family::Dho900);
            assert_eq!(model, "DP832A");
        }
        Err(e) => panic!("expected WrongInstrument, got {e:?}"),
        Ok(_) => panic!("expected WrongInstrument, got a connection"),
    }
    fake.finish();
}

#[tokio::test]
async fn knows_its_channel_count() {
    let (fake, scope) = connect(vec![]).await;
    assert_eq!(scope.channel_count(), 4);
    assert_eq!(scope.identity().model, "DHO924S");
    fake.finish();
}

#[tokio::test]
async fn reads_a_channel() {
    let (fake, scope) = connect(vec![
        step(":CHANnel1:DISPlay?", Reply::line("1")),
        step(":CHANnel1:SCALe?", Reply::line("5.000000E-02")),
        step(":CHANnel1:OFFSet?", Reply::line("-5.200000E-02")),
        step(":CHANnel1:COUPling?", Reply::line("DC")),
        step(":CHANnel1:PROBe?", Reply::line("1")),
        step(":CHANnel1:BWLimit?", Reply::line("OFF")),
        step(":CHANnel1:INVert?", Reply::line("0")),
    ])
    .await;

    let ch = scope.channel(1).await.unwrap();

    assert!(ch.displayed);
    assert_eq!(ch.volts_per_div, 0.05);
    assert_eq!(ch.offset_volts, -0.052);
    assert_eq!(ch.coupling, Some(Coupling::Dc));
    assert_eq!(ch.probe_ratio, 1.0);
    assert!(!ch.inverted);
    fake.finish();
}

#[tokio::test]
async fn refuses_a_channel_the_model_does_not_have() {
    let (fake, scope) = connect(vec![]).await;

    let err = scope.channel(5).await.unwrap_err();

    assert!(
        matches!(
            err,
            Error::NoSuchInput {
                channel: 5,
                count: 4
            }
        ),
        "{err:?}"
    );
    fake.finish();
}

#[tokio::test]
async fn an_unmeasurable_value_is_none_not_a_huge_number() {
    let (fake, scope) = connect(vec![
        step(
            ":MEASure:ITEM? FREQuency,CHANnel1",
            Reply::line("9.9000E+37"),
        ),
        step(":MEASure:ITEM? VPP,CHANnel1", Reply::line("2.0333E-03")),
    ])
    .await;

    let freq = scope.measure(MeasureItem::Frequency, 1).await.unwrap();
    let vpp = scope.measure(MeasureItem::Vpp, 1).await.unwrap();

    assert_eq!(freq.value, None);
    assert_eq!(freq.unit, "Hz");
    assert_eq!(vpp.value, Some(2.0333e-3));
    fake.finish();
}

#[tokio::test]
async fn normalises_the_mac() {
    let (fake, scope) = connect(vec![
        step(":LAN:STATus?", Reply::line("CONNECTED")),
        step(":LAN:DHCP?", Reply::line("1")),
        step(":LAN:IPADdress?", Reply::line("192.0.2.20")),
        step(":LAN:SMASk?", Reply::line("255.255.255.0")),
        step(":LAN:GATeway?", Reply::line("192.0.2.1")),
        step(":LAN:MAC?", Reply::line("00:19:af:00:00:01")),
    ])
    .await;

    let lan = scope.lan_config().await.unwrap();

    assert_eq!(lan.mac, "00:19:AF:00:00:01");
    assert!(lan.dhcp);
    fake.finish();
}

#[tokio::test]
async fn reads_a_screenshot_block() {
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0u8; 2048]].concat();
    let (fake, scope) = connect(vec![step(":DISPlay:DATA? PNG", Reply::block(&png, 512))]).await;

    let got = scope.screenshot().await.unwrap();

    assert_eq!(got, png);
    fake.finish();
}

#[tokio::test]
async fn reads_and_scales_a_waveform() {
    // 0 V on this preamble is raw 24968 (yreference + yorigin).
    let zero = 24968u16;
    let samples: Vec<u16> = (0..1000).map(|i| zero + (i % 10)).collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let (fake, scope) = connect(vec![
        step(":WAVeform:SOURce CHANnel1", Reply::None),
        step(":WAVeform:MODE NORMal", Reply::None),
        step(":WAVeform:FORMat WORD", Reply::None),
        step(
            ":WAVeform:PREamble?",
            Reply::line("1,0,1000,1,5.000000E-7,-2.500000E-4,0.000000,6.6667E-06,-7800,32768"),
        ),
        step(":WAVeform:DATA?", Reply::block(&data, 700)),
    ])
    .await;

    let w = scope.waveform(1).await.unwrap();
    let stats = w.stats().unwrap();

    assert_eq!(stats.points, 1000);
    assert!(stats.min.abs() < 1e-9, "{stats:?}");
    assert!((stats.max - 9.0 * 6.6667e-6).abs() < 1e-9, "{stats:?}");
    assert_eq!(stats.start_seconds, -2.5e-4);
    fake.finish();
}
