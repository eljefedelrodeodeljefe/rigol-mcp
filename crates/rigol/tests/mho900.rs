//! The scope model driving an MHO900 against a scripted fake instrument.
//! Read replies are captured from an MHO984 on firmware 00.01.00, with the
//! serial number and addresses replaced. That unit has no generator option,
//! so the generator replies follow the MHO900 Programming Guide instead.

use rigol::{
    ChannelConfig, Dho900, DigitalChannel, EdgeTriggerConfig, Error, Family, GeneratorConfig,
    GeneratorLimits, GeneratorLoad, InputImpedance, MeasureItem, SessionConfig,
};
use rigol_fake::{Fake, Reply, Step, step};

const IDN: &str = "RIGOL TECHNOLOGIES,MHO984,MHO9XXXXXXXXXX,00.01.00";
const NO_ERROR: &str = r#"0,"No error""#;

async fn connect(mut script: Vec<Step>) -> (Fake, Dho900) {
    script.insert(0, step("*IDN?", Reply::line(IDN)));
    let fake = Fake::start(script).await;
    let scope = Dho900::connect_as(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
        Family::Mho900,
    )
    .await
    .expect("connect");
    (fake, scope)
}

#[tokio::test]
async fn connects_as_an_mho900_with_four_channels() {
    let (fake, scope) = connect(vec![]).await;
    assert_eq!(scope.family(), Family::Mho900);
    assert_eq!(scope.channel_count(), 4);
    assert_eq!(scope.generator_channels(), 2);
    assert_eq!(scope.identity().firmware, "00.01.00");
    fake.finish();
}

/// `RIGOL_FAMILY=dho900` pointed at an MHO984 is refused, naming the model,
/// as is `mho900` pointed at a DHO924S.
#[tokio::test]
async fn each_scope_family_refuses_the_other() {
    let fake = Fake::start(vec![step("*IDN?", Reply::line(IDN))]).await;
    let result = Dho900::connect(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
    )
    .await;
    match result {
        Err(Error::WrongInstrument { expected, model }) => {
            assert_eq!(expected, Family::Dho900);
            assert_eq!(model, "MHO984");
        }
        Err(e) => panic!("expected WrongInstrument, got {e:?}"),
        Ok(_) => panic!("an MHO984 passed as a DHO900"),
    }
    fake.finish();

    let fake = Fake::start(vec![step(
        "*IDN?",
        Reply::line("RIGOL TECHNOLOGIES,DHO924S,DHO9S000000001,00.01.05"),
    )])
    .await;
    let result = Dho900::connect_as(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
        Family::Mho900,
    )
    .await;
    assert!(
        matches!(
            result,
            Err(Error::WrongInstrument {
                expected: Family::Mho900,
                ..
            })
        ),
        "a DHO924S passed as an MHO900"
    );
    fake.finish();
}

#[tokio::test]
async fn a_power_supply_family_is_refused_before_connecting() {
    let result = Dho900::connect_as(
        "192.0.2.1",
        Some(9),
        SessionConfig::default(),
        Family::Dp800,
    )
    .await;
    assert!(matches!(result, Err(Error::Conflicting(_))));
}

fn channel1(impedance: &str) -> Vec<Step> {
    vec![
        step(":CHANnel1:DISPlay?", Reply::line("1")),
        step(":CHANnel1:SCALe?", Reply::line("5.000000E-02")),
        step(":CHANnel1:OFFSet?", Reply::line("-1.556530E-01")),
        step(":CHANnel1:COUPling?", Reply::line("DC")),
        step(":CHANnel1:PROBe?", Reply::line("1.000000E0")),
        step(":CHANnel1:BWLimit?", Reply::line("OFF")),
        step(":CHANnel1:INVert?", Reply::line("0")),
        step(":CHANnel1:IMPedance?", Reply::line(impedance)),
    ]
}

#[tokio::test]
async fn reads_a_channel_with_its_input_impedance() {
    let (fake, scope) = connect(channel1("OMEG")).await;

    let ch = scope.channel(1).await.unwrap();

    assert!(ch.displayed);
    assert_eq!(ch.volts_per_div, 0.05);
    assert_eq!(ch.offset_volts, -0.155653);
    assert_eq!(ch.probe_ratio, 1.0);
    assert_eq!(ch.input_impedance, Some(InputImpedance::OneMegohm));
    let json = serde_json::to_value(&ch).unwrap();
    assert_eq!(json["input_impedance"], "one_megohm");
    fake.finish();
}

#[tokio::test]
async fn reads_the_lan_subsystem() {
    let (fake, scope) = connect(vec![
        step(":LAN:STATus?", Reply::line("CONNECTED")),
        step(":LAN:DHCP?", Reply::line("1")),
        step(":LAN:IPADdress?", Reply::line("192.0.2.20")),
        step(":LAN:SMASk?", Reply::line("255.255.255.0")),
        step(":LAN:GATeway?", Reply::line("192.0.2.1")),
        step(":LAN:MAC?", Reply::line("00:19:AF:00:00:01")),
    ])
    .await;

    let lan = scope.lan_config().await.unwrap();

    assert_eq!(lan.status, "CONNECTED");
    assert!(lan.dhcp);
    assert_eq!(lan.mac, "00:19:AF:00:00:01");
    fake.finish();
}

#[tokio::test]
async fn measures_and_reads_statistics() {
    let (fake, scope) = connect(vec![
        step(":MEASure:ITEM? VPP,CHANnel1", Reply::line("3.1580E-01")),
        step(
            ":MEASure:STATistic:ITEM? CURRent,VPP,CHANnel1",
            Reply::line("3.1513E-01"),
        ),
        step(
            ":MEASure:STATistic:ITEM? MINimum,VPP,CHANnel1",
            Reply::line("3.1469E-01"),
        ),
        step(
            ":MEASure:STATistic:ITEM? MAXimum,VPP,CHANnel1",
            Reply::line("3.1580E-01"),
        ),
        step(
            ":MEASure:STATistic:ITEM? AVERages,VPP,CHANnel1",
            Reply::line("3.1511E-01"),
        ),
        step(
            ":MEASure:STATistic:ITEM? DEViation,VPP,CHANnel1",
            Reply::line("4.0818E-04"),
        ),
        step(
            ":MEASure:STATistic:ITEM? CNT,VPP,CHANnel1",
            Reply::line("1.3000E+01"),
        ),
    ])
    .await;

    let r = scope.measure(MeasureItem::Vpp, 1).await.unwrap();
    assert_eq!(r.value, Some(0.3158));
    let s = scope.measure_statistics(MeasureItem::Vpp, 1).await.unwrap();
    assert_eq!(s.count, 13);
    assert_eq!(s.minimum, Some(0.31469));
    fake.finish();
}

#[tokio::test]
async fn reads_a_word_waveform() {
    // 0 V is raw 9421 (32768 - 23347); one division of 50 mV is 7500 codes.
    let mut data = Vec::new();
    for raw in [9421u16, 9421 + 7500, 9421] {
        data.extend_from_slice(&raw.to_le_bytes());
    }
    let (fake, scope) = connect(vec![
        step(":WAVeform:SOURce CHANnel1", Reply::None),
        step(":WAVeform:MODE NORMal", Reply::None),
        step(":WAVeform:FORMat WORD", Reply::None),
        step(
            ":WAVeform:PREamble?",
            Reply::line("1,0,3,1,2.000000E-6,-1.000000E-3,0.000000,6.6667E-06,-23347,32768"),
        ),
        step(":WAVeform:DATA?", Reply::block(&data, 64)),
    ])
    .await;

    let w = scope.waveform(1).await.unwrap();

    assert_eq!(w.volts.len(), 3);
    assert!(w.volts[0].abs() < 1e-9);
    assert!((w.volts[1] - 0.05).abs() < 1e-5);
    fake.finish();
}

#[tokio::test]
async fn clears_measurements_with_measure_delete() {
    let (fake, scope) = connect(vec![
        step(":MEASure:DELete", Reply::None),
        step(":SYST:ERR?", Reply::line(NO_ERROR)),
    ])
    .await;
    assert_eq!(scope.clear_measurements_command(), ":MEASure:DELete");
    scope.clear_measurements().await.unwrap();
    // A query, so the fake has seen the command before finish().
    scope.errors().await.unwrap();
    fake.finish();
}

/// 1 mV/div is the floor at 1 MΩ; 200 µV/div is allowed at 50 Ω, where the
/// ceiling is 1 V/div.
#[tokio::test]
async fn the_vertical_scale_range_follows_the_input_impedance() {
    let (fake, scope) = connect(vec![
        step(":CHANnel1:PROBe?", Reply::line("1.000000E0")),
        step(":CHANnel1:IMPedance?", Reply::line("OMEG")),
        step(":CHANnel1:PROBe?", Reply::line("1.000000E0")),
        step(":CHANnel1:IMPedance?", Reply::line("FIFT")),
    ])
    .await;
    let small = ChannelConfig {
        volts_per_div: Some(500e-6),
        ..Default::default()
    };
    let large = ChannelConfig {
        volts_per_div: Some(2.0),
        ..Default::default()
    };

    let err = scope.configure_channel(1, &small).await.unwrap_err();
    assert!(matches!(err, Error::OutOfRange { .. }), "{err:?}");
    let err = scope.configure_channel(1, &large).await.unwrap_err();
    assert!(matches!(err, Error::OutOfRange { .. }), "{err:?}");
    fake.finish();
}

#[tokio::test]
async fn accepts_the_mho900_only_probe_ratios() {
    let mut script = vec![
        step("*CLS", Reply::None),
        step(":CHANnel1:PROBe 15", Reply::None),
    ];
    script.extend(channel1("OMEG"));
    script.push(step(":SYST:ERR?", Reply::line(NO_ERROR)));
    let (fake, scope) = connect(script).await;

    let cfg = ChannelConfig {
        probe_ratio: Some(15.0),
        ..Default::default()
    };
    scope.configure_channel(1, &cfg).await.unwrap();
    fake.finish();
}

#[tokio::test]
async fn digital_channels_are_available() {
    let (fake, scope) = connect(vec![
        step("*CLS", Reply::None),
        step(":TRIGger:MODE EDGE", Reply::None),
        step(":TRIGger:EDGE:SOURce D0", Reply::None),
        step(":TRIGger:MODE?", Reply::line("EDGE")),
        step(":TRIGger:EDGE:SOURce?", Reply::line("D0")),
        step(":TRIGger:EDGE:SLOPe?", Reply::line("POS")),
        step(":TRIGger:EDGE:LEVel?", Reply::line("1.516530E-1")),
        step(":TRIGger:SWEep?", Reply::line("AUTO")),
        step(":SYST:ERR?", Reply::line(NO_ERROR)),
    ])
    .await;

    let cfg = EdgeTriggerConfig {
        digital: Some(DigitalChannel::D0),
        ..Default::default()
    };
    let applied = scope.configure_edge_trigger(&cfg).await.unwrap();
    assert_eq!(applied.state.source, "D0");
    fake.finish();
}

// ── Signal generator ─────────────────────────────────────────────────────────

fn no_generator_option() -> Vec<Step> {
    vec![
        step(":SYSTem:OPTion:STATus? AFG100", Reply::line("0")),
        step(":SYSTem:OPTion:STATus? BND", Reply::line("0")),
        step(":SYSTem:OPTion:STATus? AFG50", Reply::line("0")),
    ]
}

/// Captured: the MHO984 without the option answers `0` to all three. No
/// `:SOURce<n>` command is sent.
#[tokio::test]
async fn without_the_option_the_generator_is_refused_and_nothing_is_sent() {
    let mut script = no_generator_option();
    script.extend(no_generator_option());
    let (fake, scope) = connect(script).await;

    let err = scope.generator_channel(1).await.unwrap_err();
    assert!(matches!(err, Error::NotOnThisModel { .. }), "{err:?}");
    let err = scope
        .generator_channel_output_on(2)
        .await
        .expect_err("output on without a generator");
    assert!(err.to_string().contains("AFG"), "{err}");
    fake.finish();
}

#[tokio::test]
async fn a_third_generator_channel_does_not_exist() {
    let (fake, scope) = connect(vec![]).await;
    let err = scope.generator_channel(3).await.unwrap_err();
    assert!(
        matches!(
            err,
            Error::NoSuchInput {
                channel: 3,
                count: 2
            }
        ),
        "{err:?}"
    );
    fake.finish();
}

fn afg100() -> Step {
    step(":SYSTem:OPTion:STATus? AFG100", Reply::line("1"))
}

fn source2_now(load: &str, amp: &str, off: &str) -> Vec<Step> {
    vec![
        step(":SOURce2:IMPedance?", Reply::line(load)),
        step(":SOURce2:FUNCtion?", Reply::line("SIN")),
        step(":SOURce2:FREQuency?", Reply::line("1.000000E+3")),
        step(":SOURce2:VOLTage:AMPLitude?", Reply::line(amp)),
        step(":SOURce2:VOLTage:OFFSet?", Reply::line(off)),
    ]
}

fn source2_readback(output: &str, load: &str, amp: &str, off: &str) -> Vec<Step> {
    vec![
        step(":SOURce2:FUNCtion?", Reply::line("SIN")),
        step(":SOURce2:FREQuency?", Reply::line("1.000000E+3")),
        step(":SOURce2:VOLTage:AMPLitude?", Reply::line(amp)),
        step(":SOURce2:VOLTage:OFFSet?", Reply::line(off)),
        step(":SOURce2:IMPedance?", Reply::line(load)),
        step(":SOURce2:OUTPut:STATe?", Reply::line(output)),
    ]
}

/// Set to a 50 Ω load, 6 Vpp is 12 Vpp on an open circuit: over a 10 Vpp
/// ceiling, so nothing is sent, although 6 Vpp alone would pass.
#[tokio::test]
async fn a_fifty_ohm_setting_counts_double_against_the_ceiling() {
    let mut script = vec![afg100()];
    script.extend(source2_now("FIFT", "1.000000E0", "0.000000"));
    let (fake, scope) = connect(script).await;
    let limits = GeneratorLimits::for_family(Family::Mho900, 10.0, 5.0).unwrap();
    let scope = scope.with_generator_channel_limits(2, limits).unwrap();

    let err = scope
        .configure_generator_channel(
            2,
            &GeneratorConfig {
                amplitude_vpp: Some(6.0),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(err, Error::GeneratorLimit { .. }), "{err:?}");
    fake.finish();
}

/// Planned in open-circuit volts, sent as the setting: 4 Vpp into 50 Ω
/// goes out as `4` and reports an 4 V open-circuit peak.
#[tokio::test]
async fn configuring_a_channel_sends_the_setting_and_leaves_the_output_off() {
    let mut script = vec![afg100()];
    script.extend(source2_now("FIFT", "1.000000E0", "0.000000"));
    script.push(step("*CLS", Reply::None));
    script.push(step(":SOURce2:VOLTage:AMPLitude 4", Reply::None));
    script.extend(source2_readback("0", "FIFT", "4.000000E0", "0.000000"));
    script.push(step(":SYST:ERR?", Reply::line(NO_ERROR)));
    let (fake, scope) = connect(script).await;

    let applied = scope
        .configure_generator_channel(
            2,
            &GeneratorConfig {
                amplitude_vpp: Some(4.0),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(!applied.state.output_on);
    assert_eq!(applied.state.channel, Some(2));
    assert_eq!(applied.state.output_load, Some(GeneratorLoad::FiftyOhm));
    assert_eq!(applied.state.peak_volts, Some(4.0));
    assert_eq!(applied.state.limits.max_vpp(), 20.0);
    fake.finish();
}

/// A load switched to 50 Ω on the front panel doubles what an open circuit
/// sees; output-on reads the load again and refuses.
#[tokio::test]
async fn output_on_rechecks_the_load_setting() {
    let mut script = vec![afg100()];
    script.extend(source2_now("FIFT", "3.000000E0", "0.000000"));
    let (fake, scope) = connect(script).await;
    let limits = GeneratorLimits::for_family(Family::Mho900, 5.0, 2.5).unwrap();
    let scope = scope.with_generator_limits(limits);

    let err = scope.generator_channel_output_on(2).await.unwrap_err();

    assert!(matches!(err, Error::GeneratorLimit { .. }), "{err:?}");
    fake.finish();
}

/// The option is asked about once; after that only the channel is read.
#[tokio::test]
async fn output_on_and_off_on_a_high_z_channel() {
    let mut script = vec![afg100()];
    script.extend(source2_now("OMEG", "3.000000E0", "0.000000"));
    script.push(step(":SOURce2:OUTPut:STATe ON", Reply::None));
    script.extend(source2_readback("1", "OMEG", "3.000000E0", "0.000000"));
    script.push(step(":SOURce2:OUTPut:STATe OFF", Reply::None));
    script.extend(source2_readback("0", "OMEG", "3.000000E0", "0.000000"));
    let (fake, scope) = connect(script).await;
    let limits = GeneratorLimits::for_family(Family::Mho900, 5.0, 2.5).unwrap();
    let scope = scope.with_generator_limits(limits);

    let on = scope.generator_channel_output_on(2).await.unwrap();
    assert!(on.output_on);
    assert_eq!(on.peak_volts, Some(1.5));
    let off = scope.generator_channel_output_off(2).await.unwrap();
    assert!(!off.output_on);
    fake.finish();
}
