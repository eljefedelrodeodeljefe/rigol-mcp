//! The DHO900 model against a scripted fake instrument, using replies
//! captured from a DHO924S on firmware 00.01.05.

use rigol::{
    ChannelConfig, Coupling, Dho900, DigitalChannel, EdgeTriggerConfig, Error, Family,
    GeneratorConfig, GeneratorLimits, LogicAnalyzerConfig, MeasureItem, SessionConfig, Slope,
    Sweep, Waveshape,
};
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

// ── Writes ───────────────────────────────────────────────────────────────────

fn channel1_readback() -> Vec<Step> {
    vec![
        step(":CHANnel1:DISPlay?", Reply::line("1")),
        step(":CHANnel1:SCALe?", Reply::line("1.000000E-01")),
        step(":CHANnel1:OFFSet?", Reply::line("0.000000")),
        step(":CHANnel1:COUPling?", Reply::line("AC")),
        step(":CHANnel1:PROBe?", Reply::line("10")),
        step(":CHANnel1:BWLimit?", Reply::line("OFF")),
        step(":CHANnel1:INVert?", Reply::line("0")),
    ]
}

/// A refused setting sends nothing: the fake fails on any command at all.
#[tokio::test]
async fn an_impossible_probe_ratio_is_refused_before_sending() {
    let (fake, scope) = connect(vec![]).await;

    let err = scope
        .configure_channel(
            1,
            &ChannelConfig {
                probe_ratio: Some(3.0),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            Error::OutOfRange {
                what: "probe ratio",
                ..
            }
        ),
        "{err:?}"
    );
    fake.finish();
}

#[tokio::test]
async fn a_scale_beyond_the_probe_adjusted_range_is_refused() {
    // 10x probe: 2 mV/div to 100 V/div on a DHO900.
    let (fake, scope) = connect(vec![step(":CHANnel1:PROBe?", Reply::line("10"))]).await;

    let too_fine = scope
        .configure_channel(
            1,
            &ChannelConfig {
                volts_per_div: Some(0.001),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(too_fine, Error::OutOfRange { .. }), "{too_fine:?}");
    fake.finish();
}

#[tokio::test]
async fn a_channel_change_reports_what_the_scope_holds() {
    let mut script = vec![
        step("*CLS", Reply::None),
        step(":CHANnel1:PROBe 10", Reply::None),
        step(":CHANnel1:COUPling AC", Reply::None),
        step(":CHANnel1:SCALe 0.1", Reply::None),
    ];
    script.extend(channel1_readback());
    script.push(step(":SYST:ERR?", Reply::line(r#"0,"No error""#)));
    let (fake, scope) = connect(script).await;

    let applied = scope
        .configure_channel(
            1,
            &ChannelConfig {
                probe_ratio: Some(10.0),
                coupling: Some(Coupling::Ac),
                volts_per_div: Some(0.1),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(applied.state.probe_ratio, 10.0);
    assert_eq!(applied.state.coupling, Some(Coupling::Ac));
    assert!(applied.instrument_errors.is_empty());
    fake.finish();
}

/// Where the limit depends on other settings, the scope decides; its
/// objection comes back alongside the state it kept.
#[tokio::test]
async fn an_offset_the_scope_rejects_comes_back_as_an_instrument_error() {
    let mut script = vec![
        step("*CLS", Reply::None),
        step(":CHANnel1:OFFSet 50", Reply::None),
    ];
    script.extend(channel1_readback());
    script.push(step(
        ":SYST:ERR?",
        Reply::line(r#"-222,"Data out of range""#),
    ));
    script.push(step(":SYST:ERR?", Reply::line(r#"0,"No error""#)));
    let (fake, scope) = connect(script).await;

    let applied = scope
        .configure_channel(
            1,
            &ChannelConfig {
                offset_volts: Some(50.0),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(applied.state.offset_volts, 0.0);
    assert_eq!(applied.instrument_errors.len(), 1);
    assert_eq!(applied.instrument_errors[0].code, -222);
    fake.finish();
}

#[tokio::test]
async fn an_edge_trigger_change_uses_guide_spellings() {
    let (fake, scope) = connect(vec![
        step("*CLS", Reply::None),
        step(":TRIGger:MODE EDGE", Reply::None),
        step(":TRIGger:EDGE:SOURce CHANnel2", Reply::None),
        step(":TRIGger:EDGE:SLOPe NEGative", Reply::None),
        step(":TRIGger:EDGE:LEVel 1.5", Reply::None),
        step(":TRIGger:SWEep SINGle", Reply::None),
        step(":TRIGger:MODE?", Reply::line("EDGE")),
        step(":TRIGger:EDGE:SOURce?", Reply::line("CHAN2")),
        step(":TRIGger:EDGE:SLOPe?", Reply::line("NEG")),
        step(":TRIGger:EDGE:LEVel?", Reply::line("1.500000E+00")),
        step(":TRIGger:SWEep?", Reply::line("SING")),
        step(":SYST:ERR?", Reply::line(r#"0,"No error""#)),
    ])
    .await;

    let applied = scope
        .configure_edge_trigger(&EdgeTriggerConfig {
            channel: Some(2),
            digital: None,
            slope: Some(Slope::Falling),
            level_volts: Some(1.5),
            sweep: Some(Sweep::Single),
        })
        .await
        .unwrap();

    assert_eq!(applied.state.source, "CHAN2");
    assert_eq!(applied.state.level_volts, 1.5);
    fake.finish();
}

#[tokio::test]
async fn an_edge_trigger_can_watch_a_digital_channel() {
    // Captured from a DHO924S (00.01.05) with a PLA2216 probe on D0; the
    // level reply format is the one :TRIGger:EDGE:LEVel? gives for 0.4 V,
    // 4.000000E-1.
    let (fake, scope) = connect(vec![
        step("*CLS", Reply::None),
        step(":TRIGger:MODE EDGE", Reply::None),
        step(":TRIGger:EDGE:SOURce D0", Reply::None),
        step(":TRIGger:EDGE:SLOPe NEGative", Reply::None),
        step(":TRIGger:EDGE:LEVel 1.65", Reply::None),
        step(":TRIGger:MODE?", Reply::line("EDGE")),
        step(":TRIGger:EDGE:SOURce?", Reply::line("D0")),
        step(":TRIGger:EDGE:SLOPe?", Reply::line("NEG")),
        step(":TRIGger:EDGE:LEVel?", Reply::line("1.650000E0")),
        step(":TRIGger:SWEep?", Reply::line("NORM")),
        step(":SYST:ERR?", Reply::line(r#"0,"No error""#)),
    ])
    .await;

    let applied = scope
        .configure_edge_trigger(&EdgeTriggerConfig {
            digital: Some(DigitalChannel::D0),
            slope: Some(Slope::Falling),
            level_volts: Some(1.65),
            ..EdgeTriggerConfig::default()
        })
        .await
        .unwrap();

    assert_eq!(applied.state.source, "D0");
    assert!(applied.instrument_errors.is_empty());
    fake.finish();
}

#[tokio::test]
async fn two_trigger_sources_are_refused_before_sending() {
    let (fake, scope) = connect(vec![]).await;
    let result = scope
        .configure_edge_trigger(&EdgeTriggerConfig {
            channel: Some(1),
            digital: Some(DigitalChannel::D3),
            ..EdgeTriggerConfig::default()
        })
        .await;
    assert!(matches!(result, Err(Error::Conflicting(_))), "{result:?}");
    fake.finish();
}

#[tokio::test]
async fn a_dho800_refuses_a_digital_trigger_before_sending() {
    let fake = Fake::start(vec![step(
        "*IDN?",
        Reply::line("RIGOL TECHNOLOGIES,DHO804,DHO8A000000001,00.01.05"),
    )])
    .await;
    let scope = Dho900::connect(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
    )
    .await
    .unwrap();

    let result = scope
        .configure_edge_trigger(&EdgeTriggerConfig {
            digital: Some(DigitalChannel::D0),
            ..EdgeTriggerConfig::default()
        })
        .await;
    assert!(
        matches!(result, Err(Error::NotOnThisModel { .. })),
        "{result:?}"
    );
    fake.finish();
}

#[test]
fn digital_channels_map_to_guide_names_and_pods() {
    assert_eq!(DigitalChannel::ALL.len(), 16);
    for (n, d) in DigitalChannel::ALL.iter().enumerate() {
        assert_eq!(usize::from(d.number()), n);
        assert_eq!(serde_json::to_value(d).unwrap(), format!("D{n}"));
    }
    assert_eq!(DigitalChannel::D7.pod(), 1);
    assert_eq!(DigitalChannel::D8.pod(), 2);
}

// ── Logic analyser ───────────────────────────────────────────────────────────

/// The logic-analyser readback, as a DHO924S (00.01.05) answers it: D0–D7
/// shown, thresholds in volts times 1e18.
fn la_readback(shown: &[u8], pod1: &str, pod2: &str) -> Vec<Step> {
    let mut v: Vec<Step> = (0..16u8)
        .map(|n| {
            step(
                &format!(":LA:DIGital:ENABle? D{n}"),
                Reply::line(if shown.contains(&n) { "1" } else { "0" }),
            )
        })
        .collect();
    v.push(step(":LA:ENABle?", Reply::line("1")));
    v.push(step(":LA:POD1:THReshold?", Reply::line(pod1)));
    v.push(step(":LA:POD2:THReshold?", Reply::line(pod2)));
    v
}

#[tokio::test]
async fn reads_the_logic_analyser_with_thresholds_in_volts() {
    let (fake, scope) = connect(la_readback(
        &[0, 1, 2, 3, 4, 5, 6, 7, 12],
        "1.650000E+18",
        "0.000000",
    ))
    .await;

    let la = scope.logic_analyzer().await.unwrap();

    assert!(la.enabled);
    assert!((la.pod1_threshold_volts - 1.65).abs() < 1e-12);
    assert_eq!(la.pod2_threshold_volts, 0.0);
    let shown: Vec<_> = la
        .channels
        .iter()
        .filter(|c| c.displayed)
        .map(|c| c.channel)
        .collect();
    assert_eq!(shown.len(), 9);
    assert_eq!(shown.last(), Some(&DigitalChannel::D12));
    fake.finish();
}

#[tokio::test]
async fn configures_the_logic_analyser_on_first_in_guide_spelling() {
    let mut script = vec![
        step("*CLS", Reply::None),
        step(":LA:ENABle 1", Reply::None),
        step(":LA:POD1:THReshold 1.65", Reply::None),
        step(":LA:POD2:THReshold 2.5", Reply::None),
        step(":LA:DIGital:ENABle D8,1", Reply::None),
        step(":LA:DIGital:ENABle D3,0", Reply::None),
    ];
    script.extend(la_readback(
        &[0, 1, 2, 4, 5, 6, 7, 8],
        "1.650000E+18",
        "2.500000E+18",
    ));
    script.push(step(":SYST:ERR?", Reply::line(r#"0,"No error""#)));
    let (fake, scope) = connect(script).await;

    let applied = scope
        .configure_logic_analyzer(&LogicAnalyzerConfig {
            enabled: Some(true),
            show: vec![DigitalChannel::D8],
            hide: vec![DigitalChannel::D3],
            pod1_threshold_volts: Some(1.65),
            pod2_threshold_volts: Some(2.5),
        })
        .await
        .unwrap();

    assert!((applied.state.pod2_threshold_volts - 2.5).abs() < 1e-12);
    assert!(applied.state.channels[8].displayed);
    assert!(!applied.state.channels[3].displayed);
    assert!(applied.instrument_errors.is_empty());
    fake.finish();
}

#[tokio::test]
async fn turning_the_logic_analyser_off_comes_last() {
    let mut script = vec![
        step("*CLS", Reply::None),
        step(":LA:DIGital:ENABle D12,0", Reply::None),
        step(":LA:ENABle 0", Reply::None),
    ];
    // Off, every channel reads as hidden.
    script.extend((0..16u8).map(|n| step(&format!(":LA:DIGital:ENABle? D{n}"), Reply::line("0"))));
    script.push(step(":LA:ENABle?", Reply::line("0")));
    script.push(step(":LA:POD1:THReshold?", Reply::line("1.650000E+18")));
    script.push(step(":LA:POD2:THReshold?", Reply::line("1.650000E+18")));
    script.push(step(":SYST:ERR?", Reply::line(r#"0,"No error""#)));
    let (fake, scope) = connect(script).await;

    let applied = scope
        .configure_logic_analyzer(&LogicAnalyzerConfig {
            enabled: Some(false),
            hide: vec![DigitalChannel::D12],
            ..LogicAnalyzerConfig::default()
        })
        .await
        .unwrap();

    assert!(!applied.state.enabled);
    fake.finish();
}

#[tokio::test]
async fn a_threshold_the_scope_would_clamp_is_refused_before_sending() {
    let (fake, scope) = connect(vec![]).await;
    for v in [15.5, -20.0, f64::NAN] {
        let result = scope
            .configure_logic_analyzer(&LogicAnalyzerConfig {
                pod2_threshold_volts: Some(v),
                ..LogicAnalyzerConfig::default()
            })
            .await;
        assert!(
            matches!(result, Err(Error::OutOfRange { .. })),
            "{v}: {result:?}"
        );
    }
    fake.finish();
}

#[tokio::test]
async fn showing_and_hiding_one_channel_is_refused() {
    let (fake, scope) = connect(vec![]).await;
    let result = scope
        .configure_logic_analyzer(&LogicAnalyzerConfig {
            show: vec![DigitalChannel::D1],
            hide: vec![DigitalChannel::D1],
            ..LogicAnalyzerConfig::default()
        })
        .await;
    assert!(matches!(result, Err(Error::Conflicting(_))), "{result:?}");
    fake.finish();
}

#[tokio::test]
async fn acquisition_control_uses_guide_spellings() {
    let (fake, scope) = connect(vec![
        step(":RUN", Reply::None),
        step(":STOP", Reply::None),
        step(":SINGle", Reply::None),
        step(":AUToset", Reply::None),
        step(":MEASure:CLEar", Reply::None),
        step(":SYST:ERR?", Reply::line(r#"0,"No error""#)),
    ])
    .await;

    scope.run().await.unwrap();
    scope.stop().await.unwrap();
    scope.single().await.unwrap();
    scope.autoset().await.unwrap();
    scope.clear_measurements().await.unwrap();
    // A query, so the fake has seen every command before finish().
    scope.errors().await.unwrap();

    fake.finish();
}

#[tokio::test]
async fn reads_measurement_statistics_in_guide_spelling() {
    // Captured from a DHO924S (00.01.05), VPP on channel 1 right after the
    // item was added: one reading, so no spread yet.
    let (fake, scope) = connect(vec![
        step(
            ":MEASure:STATistic:ITEM? CURRent,VPP,CHANnel1",
            Reply::line("5.6600E-01"),
        ),
        step(
            ":MEASure:STATistic:ITEM? MINimum,VPP,CHANnel1",
            Reply::line("5.6600E-01"),
        ),
        step(
            ":MEASure:STATistic:ITEM? MAXimum,VPP,CHANnel1",
            Reply::line("5.6600E-01"),
        ),
        step(
            ":MEASure:STATistic:ITEM? AVERages,VPP,CHANnel1",
            Reply::line("5.6600E-01"),
        ),
        step(
            ":MEASure:STATistic:ITEM? DEViation,VPP,CHANnel1",
            Reply::line("0.0000E+00"),
        ),
        step(
            ":MEASure:STATistic:ITEM? CNT,VPP,CHANnel1",
            Reply::line("1.0000E+00"),
        ),
    ])
    .await;

    let s = scope.measure_statistics(MeasureItem::Vpp, 1).await.unwrap();

    assert_eq!(s.current, Some(0.566));
    assert_eq!(s.minimum, Some(0.566));
    assert_eq!(s.maximum, Some(0.566));
    assert_eq!(s.average, Some(0.566));
    assert_eq!(s.deviation, Some(0.0));
    assert_eq!(s.count, 1);
    assert_eq!(s.unit, "V");
    fake.finish();
}

#[tokio::test]
async fn unmeasurable_statistics_are_none() {
    // 9.9E37 is the scope's "no reading", as for :MEASure:ITEM?.
    let none = "9.9000E+37";
    let (fake, scope) = connect(
        [
            "CURRent",
            "MINimum",
            "MAXimum",
            "AVERages",
            "DEViation",
            "CNT",
        ]
        .iter()
        .map(|t| {
            step(
                &format!(":MEASure:STATistic:ITEM? {t},FREQuency,CHANnel2"),
                Reply::line(none),
            )
        })
        .collect(),
    )
    .await;

    let s = scope
        .measure_statistics(MeasureItem::Frequency, 2)
        .await
        .unwrap();

    assert_eq!((s.current, s.minimum, s.maximum), (None, None, None));
    assert_eq!((s.average, s.deviation, s.count), (None, None, 0));
    fake.finish();
}

#[tokio::test]
async fn statistics_on_a_missing_channel_send_nothing() {
    let (fake, scope) = connect(vec![]).await;
    assert!(matches!(
        scope.measure_statistics(MeasureItem::Period, 5).await,
        Err(Error::NoSuchInput { channel: 5, .. })
    ));
    fake.finish();
}

#[tokio::test]
async fn resetting_statistics_reports_the_error_queue() {
    let (fake, scope) = connect(vec![
        step("*CLS", Reply::None),
        step(":MEASure:STATistic:RESet", Reply::None),
        step(":SYST:ERR?", Reply::line(r#"0,"No error""#)),
    ])
    .await;

    assert!(
        scope
            .reset_measurement_statistics()
            .await
            .unwrap()
            .is_empty()
    );
    fake.finish();
}

// ── Signal generator ─────────────────────────────────────────────────────────

fn generator_now(shape: &str, hz: &str, amp: &str, off: &str) -> Vec<Step> {
    vec![
        step(":SOURce:FUNCtion?", Reply::line(shape)),
        step(":SOURce:FREQuency?", Reply::line(hz)),
        step(":SOURce:VOLTage:AMPLitude?", Reply::line(amp)),
        step(":SOURce:VOLTage:OFFSet?", Reply::line(off)),
    ]
}

fn generator_readback(output: &str, shape: &str, hz: &str, amp: &str, off: &str) -> Vec<Step> {
    let mut v = generator_now(shape, hz, amp, off);
    v.push(step(":SOURce:OUTPut:STATe?", Reply::line(output)));
    v
}

/// Configuring the generator must never energise it. The fake fails the
/// test on any command not in the script, so an `:SOURce:OUTPut:STATe ON`
/// anywhere would be caught.
#[tokio::test]
async fn configuring_the_generator_leaves_the_output_off() {
    let mut script = generator_now("SIN", "1.000000E+3", "5.000000E0", "0.000000");
    script.push(step("*CLS", Reply::None));
    script.push(step(":SOURce:VOLTage:AMPLitude 1", Reply::None));
    script.push(step(":SOURce:FREQuency 10000", Reply::None));
    script.push(step(":SOURce:FUNCtion SQUare", Reply::None));
    script.extend(generator_readback(
        "0",
        "SQU",
        "1.000000E+4",
        "1.000000E0",
        "0.000000",
    ));
    script.push(step(":SYST:ERR?", Reply::line(r#"0,"No error""#)));
    let (fake, scope) = connect(script).await;

    let applied = scope
        .configure_generator(&GeneratorConfig {
            shape: Some(Waveshape::Square),
            frequency_hz: Some(10_000.0),
            amplitude_vpp: Some(1.0),
            offset_volts: None,
        })
        .await
        .unwrap();

    assert!(!applied.state.output_on);
    assert_eq!(applied.state.peak_volts, Some(0.5));
    fake.finish();
}

#[tokio::test]
async fn a_generator_setting_over_the_ceiling_sends_nothing() {
    let (fake, scope) = connect(generator_now(
        "SIN",
        "1.000000E+3",
        "1.000000E0",
        "0.000000",
    ))
    .await;
    let scope = scope.with_generator_limits(GeneratorLimits::new(4.0, 3.3).unwrap());

    let err = scope
        .configure_generator(&GeneratorConfig {
            amplitude_vpp: Some(3.0),
            offset_volts: Some(2.0),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert!(matches!(err, Error::GeneratorLimit { .. }), "{err:?}");
    fake.finish();
}

/// Settings changed on the front panel since configuration are checked
/// again before the output goes on.
#[tokio::test]
async fn output_on_refuses_front_panel_settings_over_the_ceiling() {
    let (fake, scope) = connect(generator_now(
        "SIN",
        "1.000000E+3",
        "5.000000E0",
        "0.000000",
    ))
    .await;
    let scope = scope.with_generator_limits(GeneratorLimits::new(3.3, 1.65).unwrap());

    let err = scope.generator_output_on().await.unwrap_err();

    assert!(matches!(err, Error::GeneratorLimit { .. }), "{err:?}");
    fake.finish();
}

#[tokio::test]
async fn output_on_within_the_ceiling_switches_on() {
    let mut script = generator_now("SIN", "1.000000E+3", "1.000000E0", "0.000000");
    script.push(step(":SOURce:OUTPut:STATe ON", Reply::None));
    script.extend(generator_readback(
        "1",
        "SIN",
        "1.000000E+3",
        "1.000000E0",
        "0.000000",
    ));
    let (fake, scope) = connect(script).await;

    let state = scope.generator_output_on().await.unwrap();

    assert!(state.output_on);
    fake.finish();
}

/// The generator as a stateful fake: it applies every command it is sent
/// and records the largest peak `|offset| + amplitude/2` the output passed
/// through on the way.
#[derive(Debug)]
struct GeneratorModel {
    amplitude_vpp: f64,
    offset_volts: f64,
    worst_peak: f64,
}

async fn generator_fake(model: std::sync::Arc<std::sync::Mutex<GeneratorModel>>) -> Fake {
    let answer = move |cmd: &str| -> Option<Reply> {
        let mut g = model.lock().unwrap();
        let reply = match cmd {
            "*IDN?" => Reply::line(IDN),
            ":SOURce:FUNCtion?" => Reply::line("SIN"),
            ":SOURce:FREQuency?" => Reply::line("1.000000E+3"),
            ":SOURce:VOLTage:AMPLitude?" => Reply::Line(format!("{:E}", g.amplitude_vpp)),
            ":SOURce:VOLTage:OFFSet?" => Reply::Line(format!("{:E}", g.offset_volts)),
            ":SOURce:OUTPut:STATe?" => Reply::line("1"),
            ":SYST:ERR?" => Reply::line(r#"0,"No error""#),
            "*CLS" => Reply::None,
            _ => {
                if let Some(v) = cmd.strip_prefix(":SOURce:VOLTage:AMPLitude ") {
                    g.amplitude_vpp = v.parse().ok()?;
                } else {
                    let v = cmd.strip_prefix(":SOURce:VOLTage:OFFSet ")?;
                    g.offset_volts = v.parse().ok()?;
                }
                let peak = g.offset_volts.abs() + g.amplitude_vpp / 2.0;
                g.worst_peak = g.worst_peak.max(peak);
                Reply::None
            }
        };
        Some(reply)
    };
    Fake::start_with(answer).await
}

/// rmcp runs each request as its own task, so two configure calls can be
/// in flight at once. Each is inside the 3.3 V peak on its own, measured
/// against the state it read; together they would reach 4.95 V. With the
/// operation lock the second one reads the first one's result and is
/// refused, and the output never leaves the envelope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_generator_changes_never_exceed_the_ceiling() {
    use std::sync::{Arc, Mutex};
    for _ in 0..20 {
        let model = Arc::new(Mutex::new(GeneratorModel {
            amplitude_vpp: 1.0,
            offset_volts: 0.0,
            worst_peak: 0.5,
        }));
        let fake = generator_fake(model.clone()).await;
        let scope = Dho900::connect(
            &fake.addr.ip().to_string(),
            Some(fake.addr.port()),
            SessionConfig::default(),
        )
        .await
        .expect("connect")
        .with_generator_limits(GeneratorLimits::new(8.0, 3.3).unwrap());

        let raise_amplitude = GeneratorConfig {
            amplitude_vpp: Some(6.6),
            ..Default::default()
        };
        let raise_offset = GeneratorConfig {
            offset_volts: Some(1.65),
            ..Default::default()
        };
        let (a, b) = tokio::join!(
            scope.configure_generator(&raise_amplitude),
            scope.configure_generator(&raise_offset),
        );

        let g = model.lock().unwrap();
        assert!(
            g.worst_peak <= 3.3 + 1e-9,
            "output reached {} V peak: {g:?}",
            g.worst_peak
        );
        assert!(
            a.is_ok() != b.is_ok(),
            "exactly one change fits: {a:?} / {b:?}"
        );
        drop(g);
        fake.finish();
    }
}
