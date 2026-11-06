//! Every write to an instrument leaves one info line naming the operation
//! and the values it sends, so the log is an audit trail of what reached
//! the hardware.
//!
//! One test in its own binary, with a global subscriber: tracing caches
//! callsite interest process-wide, and a thread-local subscriber in a test
//! binary that runs other tests in parallel misses lines.

use std::sync::{Arc, Mutex};

use rigol::{Dho900, Dp800, GeneratorConfig, Limits, SessionConfig};
use rigol_fake::{Fake, Reply, Step, step};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    /// The info lines logged since the last call.
    fn take_info_lines(&self) -> Vec<String> {
        let bytes = std::mem::take(&mut *self.0.lock().unwrap());
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter(|l| l.contains(" INFO "))
            .map(str::to_string)
            .collect()
    }
}

fn assert_lines(lines: &[String], expected: &[(&str, &[&str])]) {
    assert_eq!(lines.len(), expected.len(), "{lines:#?}");
    for (line, (op, values)) in lines.iter().zip(expected) {
        assert!(line.contains(&format!("op=\"{op}\"")), "{op}: {line}");
        for v in *values {
            assert!(line.contains(v), "{op} lacks {v}: {line}");
        }
    }
}

fn generator_now(amp: &str) -> Vec<Step> {
    vec![
        step(":SOURce:FUNCtion?", Reply::line("SIN")),
        step(":SOURce:FREQuency?", Reply::line("1.000000E+3")),
        step(":SOURce:VOLTage:AMPLitude?", Reply::line(amp)),
        step(":SOURce:VOLTage:OFFSet?", Reply::line("0.000000")),
    ]
}

fn generator_readback(output: &str, amp: &str) -> Vec<Step> {
    let mut v = generator_now(amp);
    v.push(step(":SOURce:OUTPut:STATe?", Reply::line(output)));
    v
}

#[tokio::test]
async fn every_write_logs_one_info_line_with_its_values() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    // ── Power supply ─────────────────────────────────────────────────────────
    let fake = Fake::start(vec![
        step(
            "*IDN?",
            Reply::line("RIGOL TECHNOLOGIES,DP832A,DP8A000000001,00.01.19"),
        ),
        step(":APPL? CH1", Reply::line("CH1:30V/3A,0.000,0.100")),
        step(":APPL? CH2", Reply::line("CH2:30V/3A,0.000,3.000")),
        step(":APPL? CH3", Reply::line("CH3:5V/3A,0.000,3.000")),
        step(":SOUR1:VOLT 3.3", Reply::None),
        step(":SOUR1:CURR 0.25", Reply::None),
        step(":OUTP:OVP:VAL CH1,3.5", Reply::None),
        step(":OUTP:OCP:VAL CH1,0.4", Reply::None),
        step(":APPL? CH1", Reply::line("CH1:30V/3A,3.300,0.250")),
        step(":OUTP CH1,ON", Reply::None),
        step(":OUTP CH1,OFF", Reply::None),
        step(":OUTP? CH1", Reply::line("OFF")),
    ])
    .await;
    let psu = Dp800::connect(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
        Limits::new(),
    )
    .await
    .unwrap();
    captured.take_info_lines(); // the connect line

    psu.set_voltage(1, 3.3).await.unwrap();
    psu.set_current(1, 0.25).await.unwrap();
    psu.set_ovp(1, 3.5).await.unwrap();
    psu.set_ocp(1, 0.4).await.unwrap();
    psu.output_on(1).await.unwrap();
    psu.output_off(1).await.unwrap();
    // A query, so the fake has seen every command before finish().
    psu.output_state(1).await.unwrap();

    assert_lines(
        &captured.take_info_lines(),
        &[
            ("set_voltage", &["channel=1", "volts=3.3"]),
            ("set_current", &["channel=1", "amps=0.25"]),
            ("set_ovp", &["channel=1", "volts=3.5"]),
            ("set_ocp", &["channel=1", "amps=0.4"]),
            ("output_on", &["channel=1", "volts=3.3", "amps=0.25"]),
            ("output_off", &["channel=1"]),
        ],
    );
    fake.finish();

    // ── Signal generator ─────────────────────────────────────────────────────
    let mut script = vec![step(
        "*IDN?",
        Reply::line("RIGOL TECHNOLOGIES,DHO924S,DHO9S000000001,00.01.05"),
    )];
    script.extend(generator_now("5.000000E0"));
    script.push(step("*CLS", Reply::None));
    script.push(step(":SOURce:VOLTage:AMPLitude 1", Reply::None));
    script.extend(generator_readback("0", "1.000000E0"));
    script.push(step(":SYST:ERR?", Reply::line(r#"0,"No error""#)));
    script.extend(generator_now("1.000000E0"));
    script.push(step(":SOURce:OUTPut:STATe ON", Reply::None));
    script.extend(generator_readback("1", "1.000000E0"));
    script.push(step(":SOURce:OUTPut:STATe OFF", Reply::None));
    script.extend(generator_readback("0", "1.000000E0"));
    let fake = Fake::start(script).await;
    let scope = Dho900::connect(
        &fake.addr.ip().to_string(),
        Some(fake.addr.port()),
        SessionConfig::default(),
    )
    .await
    .unwrap();
    captured.take_info_lines();

    scope
        .configure_generator(&GeneratorConfig {
            amplitude_vpp: Some(1.0),
            ..Default::default()
        })
        .await
        .unwrap();
    scope.generator_output_on().await.unwrap();
    scope.generator_output_off().await.unwrap();

    assert_lines(
        &captured.take_info_lines(),
        &[
            (
                "configure_generator",
                &["amplitude_vpp=1", "offset_volts=0", "peak_volts=0.5"],
            ),
            (
                "generator_output_on",
                &["amplitude_vpp=1", "offset_volts=0", "peak_volts=0.5"],
            ),
            ("generator_output_off", &[]),
        ],
    );
    fake.finish();
}
