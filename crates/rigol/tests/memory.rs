//! Reading a whole record from the scope's internal memory
//! ([`Dho900::read_memory`]) against a scripted fake instrument.
//!
//! The command sequence follows "Procedures of reading the waveform data
//! from the internal memory" in the DHO800/DHO900 and MHO900 Programming
//! Guides (3.28.5). Unlike the other scope tests, the RAW-mode replies here
//! are not captured from an instrument: no RAW read has been run on hardware
//! yet. The preambles keep the captured NORMal-mode scaling (6.6667E-06 V per
//! code is 50 mV/div over 7500 codes) with RAW mode's type 2.

use rigol::{Dho900, Error, Family, InputImpedance, MemoryReadOptions, SessionConfig};
use rigol_fake::{Fake, Reply, Step, step};

const MHO_IDN: &str = "RIGOL TECHNOLOGIES,MHO984,MHO9XXXXXXXXXX,00.01.00";
const DHO_IDN: &str = "RIGOL TECHNOLOGIES,DHO924S,DHO9S000000001,00.01.05";

/// 10 points at 4 GSa/s; 0 V is code 9421 (32768 - 23347).
const PREAMBLE: &str = "1,2,10,1,2.500000E-10,-1.250000E-9,0.000000,6.6667E-06,-23347,32768";

async fn connect(family: Family, mut script: Vec<Step>) -> (Fake, Dho900) {
    let idn = match family {
        Family::Mho900 => MHO_IDN,
        _ => DHO_IDN,
    };
    script.insert(0, step("*IDN?", Reply::line(idn)));
    let fake = Fake::start(script).await;
    let scope = Dho900::connect_as(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
        family,
    )
    .await
    .expect("connect");
    (fake, scope)
}

/// `:CHANnel1:…?` as `Dho900::channel` reads it, with the impedance query on
/// an MHO900 only.
fn channel_reads(impedance: Option<&str>) -> Vec<Step> {
    let mut s = vec![
        step(":CHANnel1:DISPlay?", Reply::line("1")),
        step(":CHANnel1:SCALe?", Reply::line("5.000000E-02")),
        step(":CHANnel1:OFFSet?", Reply::line("0.000000")),
        step(":CHANnel1:COUPling?", Reply::line("DC")),
        step(":CHANnel1:PROBe?", Reply::line("1.000000E0")),
        step(":CHANnel1:BWLimit?", Reply::line("OFF")),
        step(":CHANnel1:INVert?", Reply::line("0")),
    ];
    if let Some(z) = impedance {
        s.push(step(":CHANnel1:IMPedance?", Reply::line(z)));
    }
    s
}

fn trigger_reads(status: &str, sweep: &str) -> Vec<Step> {
    vec![
        step(":TRIGger:STATus?", Reply::line(status)),
        step(":TRIGger:SWEep?", Reply::line(sweep)),
    ]
}

fn settings_reads(depth: &str) -> Vec<Step> {
    vec![
        step(":ACQuire:MDEPth?", Reply::line(depth)),
        step(":ACQuire:SRATe?", Reply::line("4.000000E+9")),
        step(":ACQuire:TYPE?", Reply::line("NORM")),
        step(":TIMebase:MAIN:SCALe?", Reply::line("2.000000E-4")),
        step(":TIMebase:MAIN:OFFSet?", Reply::line("0.000000")),
        step(":TRIGger:MODE?", Reply::line("EDGE")),
        step(":TRIGger:POSition?", Reply::line("5.000000E+0")),
    ]
}

fn words(codes: impl IntoIterator<Item = u16>) -> Vec<u8> {
    codes.into_iter().flat_map(u16::to_le_bytes).collect()
}

/// Codes 9421, 9422, …: sample `i` (from 0) is 9421 + i.
fn codes(from: usize, to: usize) -> Vec<u8> {
    words((from..to).map(|i| 9421 + i as u16))
}

/// Ten points in blocks of four: 1-4, 5-8, 9-10.
fn transfer_of_ten() -> Vec<Step> {
    vec![
        step(":WAVeform:SOURce CHANnel1", Reply::None),
        step(":WAVeform:MODE RAW", Reply::None),
        step(":WAVeform:FORMat WORD", Reply::None),
        step(":WAVeform:STARt 1", Reply::None),
        step(":WAVeform:STOP 4", Reply::None),
        step(":WAVeform:PREamble?", Reply::line(PREAMBLE)),
        step(":WAVeform:DATA?", Reply::block(&codes(0, 4), 3)),
        step(":WAVeform:STOP 8", Reply::None),
        step(":WAVeform:STARt 5", Reply::None),
        step(":WAVeform:DATA?", Reply::block(&codes(4, 8), 5)),
        step(":WAVeform:STOP 10", Reply::None),
        step(":WAVeform:STARt 9", Reply::None),
        step(":WAVeform:DATA?", Reply::block(&codes(8, 10), 64)),
        step(":WAVeform:PREamble?", Reply::line(PREAMBLE)),
        step(":TRIGger:STATus?", Reply::line("STOP")),
    ]
}

fn transfer_reset() -> Vec<Step> {
    vec![
        step(":WAVeform:MODE NORMal", Reply::None),
        step(":WAVeform:STARt 1", Reply::None),
        step(":WAVeform:STOP 1000", Reply::None),
    ]
}

/// A query after the last command, so the test sees every command arrive
/// before it checks the script: commands get no reply to wait for.
fn sync() -> Step {
    step(":SYST:ERR?", Reply::line(r#"0,"No error""#))
}

fn opts(chunk_points: usize) -> MemoryReadOptions {
    MemoryReadOptions {
        chunk_points,
        ..MemoryReadOptions::default()
    }
}

/// A stopped MHO984 is read as it is, block by block, and only the waveform
/// transfer settings change. The blocks arrive in order and nothing is lost
/// at their edges.
#[tokio::test]
async fn reads_a_stopped_record_in_blocks() {
    let mut script = channel_reads(Some("FIFT"));
    script.extend(trigger_reads("STOP", "NORM"));
    script.extend(settings_reads("1.0000E+01"));
    script.extend(transfer_of_ten());
    script.extend(transfer_reset());
    script.push(sync());
    let (fake, scope) = connect(Family::Mho900, script).await;

    let mut blocks = Vec::new();
    let rec = scope
        .read_memory(1, &opts(4), |b| {
            blocks.push(b.to_vec());
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(blocks.iter().map(Vec::len).collect::<Vec<_>>(), [4, 4, 2]);
    let all: Vec<u16> = blocks.concat();
    assert_eq!(all, (0..10).map(|i| 9421 + i).collect::<Vec<u16>>());
    assert_eq!((rec.points, rec.memory_depth, rec.chunks), (10, 10, 3));
    assert_eq!(rec.input_impedance, InputImpedance::FiftyOhm);
    assert_eq!(rec.sample_rate, 4.0e9);
    assert_eq!(rec.trigger_position, Some(5.0));
    assert!(!rec.stopped_for_read);
    assert_eq!(rec.restored_with, None);
    scope.errors().await.unwrap();
    fake.finish();
}

/// The preamble read in RAW mode scales codes as on screen: 0 V at code
/// 9421, 50 mV one division (7500 codes) above, and one sample interval
/// of 1/sample rate.
#[tokio::test]
async fn scales_the_raw_preamble() {
    let mut script = channel_reads(Some("FIFT"));
    script.extend(trigger_reads("STOP", "NORM"));
    script.extend(settings_reads("1.0000E+01"));
    script.extend(transfer_of_ten());
    script.extend(transfer_reset());
    script.push(sync());
    let (fake, scope) = connect(Family::Mho900, script).await;

    let rec = scope.read_memory(1, &opts(4), |_| Ok(())).await.unwrap();

    let p = &rec.preamble;
    assert!(p.volts(9421).abs() < 1e-12);
    assert!((p.volts(9421 + 7500) - 0.05).abs() < 1e-5);
    assert_eq!(p.x_increment, 1.0 / rec.sample_rate);
    assert_eq!(p.seconds(0), -1.25e-9);
    assert!((p.seconds(5) - 0.0).abs() < 1e-21);
    assert_eq!(rec.preamble_raw, PREAMBLE);
    scope.errors().await.unwrap();
    fake.finish();
}

/// A required 50 Ω input that reads 1 MΩ is refused after the channel
/// reads, before the trigger state is even asked.
#[tokio::test]
async fn refuses_the_wrong_input_impedance() {
    let (fake, scope) = connect(Family::Mho900, channel_reads(Some("OMEG"))).await;

    let err = scope
        .read_memory(
            1,
            &MemoryReadOptions {
                expect_impedance: Some(InputImpedance::FiftyOhm),
                ..MemoryReadOptions::default()
            },
            |_| Ok(()),
        )
        .await
        .unwrap_err();

    match &err {
        Error::WrongImpedance {
            channel: 1,
            expected: InputImpedance::FiftyOhm,
            actual: InputImpedance::OneMegohm,
        } => {}
        e => panic!("expected WrongImpedance, got {e:?}"),
    }
    assert!(
        err.to_string().contains("1 MΩ, not the required 50 Ω"),
        "{err}"
    );
    fake.finish();
}

/// A running scope is refused unless the caller allows stopping it, and
/// nothing is sent to change that.
#[tokio::test]
async fn refuses_a_running_scope_by_default() {
    let mut script = channel_reads(Some("FIFT"));
    script.extend(trigger_reads("TD", "AUTO"));
    let (fake, scope) = connect(Family::Mho900, script).await;

    let err = scope
        .read_memory(1, &opts(4), |_| Ok(()))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Conflicting(_)), "{err:?}");
    assert!(err.to_string().contains("stopped"), "{err}");
    fake.finish();
}

/// Allowed to, the read stops a running scope, waits for STOP, reads, and
/// starts it again with `:RUN`.
#[tokio::test]
async fn stops_and_restarts_a_running_scope_when_allowed() {
    let mut script = channel_reads(Some("FIFT"));
    script.extend(trigger_reads("TD", "AUTO"));
    script.push(step(":STOP", Reply::None));
    script.push(step(":TRIGger:STATus?", Reply::line("TD")));
    script.push(step(":TRIGger:STATus?", Reply::line("STOP")));
    script.extend(settings_reads("1.0000E+01"));
    script.extend(transfer_of_ten());
    script.extend(transfer_reset());
    script.push(step(":RUN", Reply::None));
    script.push(sync());
    let (fake, scope) = connect(Family::Mho900, script).await;

    let rec = scope
        .read_memory(
            1,
            &MemoryReadOptions {
                chunk_points: 4,
                stop_if_running: true,
                expect_impedance: Some(InputImpedance::FiftyOhm),
            },
            |_| Ok(()),
        )
        .await
        .unwrap();

    assert!(rec.stopped_for_read);
    assert_eq!(rec.restored_with, Some(":RUN"));
    assert_eq!(rec.trigger_status_before, "TD");
    scope.errors().await.unwrap();
    fake.finish();
}

/// A single sweep waiting for its trigger is re-armed with `:SINGle`, not
/// switched to continuous acquisition, even when the read fails.
#[tokio::test]
async fn rearms_a_single_sweep_after_a_failed_read() {
    let mut script = channel_reads(Some("FIFT"));
    script.extend(trigger_reads("WAIT", "SINGle"));
    script.push(step(":STOP", Reply::None));
    script.push(step(":TRIGger:STATus?", Reply::line("STOP")));
    script.extend(settings_reads("1.0000E+01"));
    script.extend([
        step(":WAVeform:SOURce CHANnel1", Reply::None),
        step(":WAVeform:MODE RAW", Reply::None),
        step(":WAVeform:FORMat WORD", Reply::None),
        step(":WAVeform:STARt 1", Reply::None),
        step(":WAVeform:STOP 4", Reply::None),
        step(":WAVeform:PREamble?", Reply::line(PREAMBLE)),
        step(":WAVeform:DATA?", Reply::block(&codes(0, 4), 64)),
    ]);
    // The sink fails on the first block: the host disk is full.
    script.extend(transfer_reset());
    script.push(step(":SINGle", Reply::None));
    script.push(sync());
    let (fake, scope) = connect(Family::Mho900, script).await;

    let err = scope
        .read_memory(
            1,
            &MemoryReadOptions {
                chunk_points: 4,
                stop_if_running: true,
                expect_impedance: None,
            },
            |_| Err(std::io::Error::other("disk full")),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Io(_)), "{err:?}");
    scope.errors().await.unwrap();
    fake.finish();
}

/// A DHO input is 1 MΩ without asking: its firmware has no impedance query
/// and would leave one unanswered.
#[tokio::test]
async fn reports_a_dho_input_as_one_megohm_without_asking() {
    let mut script = channel_reads(None);
    script.extend(trigger_reads("STOP", "SINGle"));
    script.extend(settings_reads("1.0000E+01"));
    script.extend(transfer_of_ten());
    script.extend(transfer_reset());
    script.push(sync());
    let (fake, scope) = connect(Family::Dho900, script).await;

    let rec = scope.read_memory(1, &opts(4), |_| Ok(())).await.unwrap();

    assert_eq!(rec.input_impedance, InputImpedance::OneMegohm);
    assert_eq!(rec.channel_state.input_impedance, None);
    assert_eq!(rec.points, 10);
    scope.errors().await.unwrap();
    fake.finish();
}

/// A DHO is refused a 50 Ω requirement it can never meet.
#[tokio::test]
async fn refuses_fifty_ohms_on_a_dho() {
    let (fake, scope) = connect(Family::Dho900, channel_reads(None)).await;
    let err = scope
        .read_memory(
            1,
            &MemoryReadOptions {
                expect_impedance: Some(InputImpedance::FiftyOhm),
                ..MemoryReadOptions::default()
            },
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::WrongImpedance { .. }), "{err:?}");
    fake.finish();
}

/// A block shorter than asked for is the end of what the memory holds: the
/// record is what arrived, and no further block is asked for.
#[tokio::test]
async fn ends_the_record_at_a_short_block() {
    let mut script = channel_reads(Some("FIFT"));
    script.extend(trigger_reads("STOP", "NORM"));
    script.extend(settings_reads("1.0000E+01"));
    script.extend([
        step(":WAVeform:SOURce CHANnel1", Reply::None),
        step(":WAVeform:MODE RAW", Reply::None),
        step(":WAVeform:FORMat WORD", Reply::None),
        step(":WAVeform:STARt 1", Reply::None),
        step(":WAVeform:STOP 4", Reply::None),
        step(":WAVeform:PREamble?", Reply::line(PREAMBLE)),
        step(":WAVeform:DATA?", Reply::block(&codes(0, 4), 64)),
        step(":WAVeform:STOP 8", Reply::None),
        step(":WAVeform:STARt 5", Reply::None),
        step(":WAVeform:DATA?", Reply::block(&codes(4, 7), 64)),
        step(":WAVeform:PREamble?", Reply::line(PREAMBLE)),
        step(":TRIGger:STATus?", Reply::line("STOP")),
    ]);
    script.extend(transfer_reset());
    script.push(sync());
    let (fake, scope) = connect(Family::Mho900, script).await;

    let rec = scope.read_memory(1, &opts(4), |_| Ok(())).await.unwrap();

    assert_eq!((rec.points, rec.memory_depth, rec.chunks), (7, 10, 2));
    scope.errors().await.unwrap();
    fake.finish();
}

/// A record that restarted under the transfer, seen by a scope no longer
/// stopped at the end, is an error rather than a file mixing two
/// acquisitions.
#[tokio::test]
async fn refuses_a_record_that_changed_during_the_read() {
    let mut script = channel_reads(Some("FIFT"));
    script.extend(trigger_reads("STOP", "NORM"));
    script.extend(settings_reads("4"));
    script.extend([
        step(":WAVeform:SOURce CHANnel1", Reply::None),
        step(":WAVeform:MODE RAW", Reply::None),
        step(":WAVeform:FORMat WORD", Reply::None),
        step(":WAVeform:STARt 1", Reply::None),
        step(":WAVeform:STOP 4", Reply::None),
        step(":WAVeform:PREamble?", Reply::line(PREAMBLE)),
        step(":WAVeform:DATA?", Reply::block(&codes(0, 4), 64)),
        step(":WAVeform:PREamble?", Reply::line(PREAMBLE)),
        step(":TRIGger:STATus?", Reply::line("TD")),
    ]);
    script.extend(transfer_reset());
    script.push(sync());
    let (fake, scope) = connect(Family::Mho900, script).await;

    let err = scope
        .read_memory(1, &opts(4), |_| Ok(()))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("changed during the read"), "{err}");
    scope.errors().await.unwrap();
    fake.finish();
}

/// Chunk sizes outside 1 to 10 M points are refused before anything is
/// sent.
#[tokio::test]
async fn refuses_a_chunk_size_out_of_range() {
    let (fake, scope) = connect(Family::Mho900, vec![]).await;
    for n in [0, 10_000_001] {
        let err = scope
            .read_memory(1, &opts(n), |_| Ok(()))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::OutOfRange { .. }), "{err:?}");
    }
    fake.finish();
}
