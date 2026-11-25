//! Pure tool logic for the oscilloscope — no MCP types cross this boundary.

use anyhow::Result;
use std::path::Path;

use rigol::{
    ChannelConfig, DEFAULT_MEMORY_CHUNK_POINTS, Dho900, EdgeTriggerConfig, GeneratorConfig,
    LogicAnalyzerConfig, MemoryReadOptions,
};
use serde_json::{Value, json};

use crate::params::*;
use crate::record;

pub async fn identity(scope: &Dho900) -> Result<Value> {
    let mut v = serde_json::to_value(scope.identity())?;
    v["channels"] = json!(scope.channel_count());
    Ok(v)
}

pub async fn lan_config(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.lan_config().await?)?)
}

pub async fn instrument_errors(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.errors().await?)?)
}

pub async fn status(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.status().await?)?)
}

pub async fn measure(scope: &Dho900, p: &MeasureParams) -> Result<Value> {
    Ok(serde_json::to_value(
        scope.measure(p.item, p.channel).await?,
    )?)
}

pub async fn measure_statistics(scope: &Dho900, p: &MeasureParams) -> Result<Value> {
    Ok(serde_json::to_value(
        scope.measure_statistics(p.item, p.channel).await?,
    )?)
}

pub async fn screenshot(scope: &Dho900) -> Result<Vec<u8>> {
    Ok(scope.screenshot().await?)
}

/// The capture, thinned to at most `max_points` min/max buckets, with
/// statistics over every sample.
pub async fn waveform(scope: &Dho900, p: &WaveformParams) -> Result<Value> {
    let limit = p
        .max_points
        .unwrap_or(DEFAULT_WAVEFORM_POINTS)
        .clamp(1, MAX_WAVEFORM_POINTS);
    let w = scope.waveform(p.channel).await?;
    let buckets = w.decimate(limit);
    Ok(json!({
        "channel": w.channel,
        "stats": w.stats(),
        "seconds_per_sample": w.preamble.x_increment,
        "captured_points": w.volts.len(),
        "returned_buckets": buckets.len(),
        "buckets": buckets,
    }))
}

/// The whole acquisition record of one channel, read from the scope's
/// memory a block at a time and saved on this host as an `.npz` and a JSON
/// sidecar (see [`crate::record`]).
///
/// Writes nothing on the instrument unless `stop_if_running` is set and the
/// scope is acquiring; the caller gates that. The files are reserved before
/// anything is sent, so an existing file is refused with the scope untouched.
pub async fn save_waveform(scope: &Dho900, p: &SaveWaveformParams) -> Result<Value> {
    let mut target = record::Target::reserve(Path::new(&p.path))?;
    let opts = MemoryReadOptions {
        chunk_points: p.chunk_points.unwrap_or(DEFAULT_MEMORY_CHUNK_POINTS),
        stop_if_running: p.stop_if_running.unwrap_or(false),
        expect_impedance: p.expect_impedance,
    };
    let mut scratch = Vec::new();
    let rec = scope
        .read_memory(p.channel, &opts, |codes| {
            target.spool_codes(codes, &mut scratch)
        })
        .await?;

    let identity = scope.identity().clone();
    let family = scope.family();
    let (files, rec) = tokio::task::spawn_blocking(move || {
        target.finish(&identity, family, &rec).map(|f| (f, rec))
    })
    .await??;
    tracing::info!(
        channel = rec.channel,
        points = rec.points,
        npz = %files["npz"].as_str().unwrap_or_default(),
        sha256 = %files["npz_sha256"].as_str().unwrap_or_default(),
        "saved waveform record"
    );
    let mut v = files;
    v["channel"] = json!(rec.channel);
    v["points"] = json!(rec.points);
    v["chunks"] = json!(rec.chunks);
    v["sample_rate"] = json!(rec.sample_rate);
    v["input_impedance"] = json!(rec.input_impedance);
    v["stopped_for_read"] = json!(rec.stopped_for_read);
    v["restored_with"] = json!(rec.restored_with);
    v["trigger_position"] = json!(rec.trigger_position);
    Ok(v)
}

// ── Writes ───────────────────────────────────────────────────────────────────

pub async fn run(scope: &Dho900) -> Result<Value> {
    scope.run().await?;
    Ok(json!({ "sent": ":RUN" }))
}

pub async fn stop(scope: &Dho900) -> Result<Value> {
    scope.stop().await?;
    Ok(json!({ "sent": ":STOP" }))
}

pub async fn single(scope: &Dho900) -> Result<Value> {
    scope.single().await?;
    Ok(json!({ "sent": ":SINGle" }))
}

pub async fn autoset(scope: &Dho900) -> Result<Value> {
    scope.autoset().await?;
    Ok(json!({ "sent": ":AUToset" }))
}

pub async fn clear_measurements(scope: &Dho900) -> Result<Value> {
    scope.clear_measurements().await?;
    Ok(json!({ "sent": scope.clear_measurements_command() }))
}

pub async fn reset_measurement_statistics(scope: &Dho900) -> Result<Value> {
    let errors = scope.reset_measurement_statistics().await?;
    Ok(json!({ "sent": ":MEASure:STATistic:RESet", "instrument_errors": errors }))
}

pub async fn configure_channel(scope: &Dho900, p: &ConfigureChannelParams) -> Result<Value> {
    let cfg = ChannelConfig {
        displayed: p.displayed,
        volts_per_div: p.volts_per_div,
        offset_volts: p.offset_volts,
        coupling: p.coupling,
        probe_ratio: p.probe_ratio,
    };
    Ok(serde_json::to_value(
        scope.configure_channel(p.channel, &cfg).await?,
    )?)
}

pub async fn configure_timebase(scope: &Dho900, p: &ConfigureTimebaseParams) -> Result<Value> {
    Ok(serde_json::to_value(
        scope
            .configure_timebase(p.seconds_per_div, p.offset_seconds)
            .await?,
    )?)
}

pub async fn configure_edge_trigger(
    scope: &Dho900,
    p: &ConfigureEdgeTriggerParams,
) -> Result<Value> {
    let cfg = EdgeTriggerConfig {
        channel: p.channel,
        digital: p.digital,
        slope: p.slope,
        level_volts: p.level_volts,
        sweep: p.sweep,
    };
    Ok(serde_json::to_value(
        scope.configure_edge_trigger(&cfg).await?,
    )?)
}

// ── Logic analyser ───────────────────────────────────────────────────────────

pub async fn logic_analyzer(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.logic_analyzer().await?)?)
}

pub async fn configure_logic_analyzer(
    scope: &Dho900,
    p: &ConfigureLogicAnalyzerParams,
) -> Result<Value> {
    let cfg = LogicAnalyzerConfig {
        enabled: p.enabled,
        show: p.show.clone().unwrap_or_default(),
        hide: p.hide.clone().unwrap_or_default(),
        pod1_threshold_volts: p.pod1_threshold_volts,
        pod2_threshold_volts: p.pod2_threshold_volts,
    };
    Ok(serde_json::to_value(
        scope.configure_logic_analyzer(&cfg).await?,
    )?)
}

// ── Signal generator ─────────────────────────────────────────────────────────

pub async fn generator(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.generator().await?)?)
}

pub async fn configure_generator(scope: &Dho900, p: &ConfigureGeneratorParams) -> Result<Value> {
    let cfg = GeneratorConfig {
        shape: p.shape,
        frequency_hz: p.frequency_hz,
        amplitude_vpp: p.amplitude_vpp,
        offset_volts: p.offset_volts,
    };
    Ok(serde_json::to_value(
        scope.configure_generator(&cfg).await?,
    )?)
}

pub async fn generator_output_on(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.generator_output_on().await?)?)
}

pub async fn generator_output_off(scope: &Dho900) -> Result<Value> {
    Ok(serde_json::to_value(scope.generator_output_off().await?)?)
}

// ── MHO900 signal generator: two channels, optional ──────────────────────────

pub async fn generator_channel(scope: &Dho900, p: &GeneratorChannelParams) -> Result<Value> {
    Ok(serde_json::to_value(
        scope.generator_channel(p.channel).await?,
    )?)
}

pub async fn configure_generator_channel(
    scope: &Dho900,
    p: &ConfigureGeneratorChannelParams,
) -> Result<Value> {
    let cfg = GeneratorConfig {
        shape: p.shape,
        frequency_hz: p.frequency_hz,
        amplitude_vpp: p.amplitude_vpp,
        offset_volts: p.offset_volts,
    };
    Ok(serde_json::to_value(
        scope.configure_generator_channel(p.channel, &cfg).await?,
    )?)
}

pub async fn generator_channel_output_on(
    scope: &Dho900,
    p: &GeneratorChannelParams,
) -> Result<Value> {
    Ok(serde_json::to_value(
        scope.generator_channel_output_on(p.channel).await?,
    )?)
}

pub async fn generator_channel_output_off(
    scope: &Dho900,
    p: &GeneratorChannelParams,
) -> Result<Value> {
    Ok(serde_json::to_value(
        scope.generator_channel_output_off(p.channel).await?,
    )?)
}

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::io::Read;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use rigol::{Family, InputImpedance, SessionConfig};
    use rigol_fake::{Fake, Reply};

    /// Counts live heap bytes and their high-water mark, so a test can show a
    /// save never holds the record.
    struct Counting;

    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    // SAFETY: forwards every call to the system allocator unchanged and only
    // adds bookkeeping on the side.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: the caller's contract for `alloc` is passed on as is.
            let p = unsafe { System.alloc(layout) };
            if !p.is_null() {
                let now = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
                PEAK.fetch_max(now, Ordering::Relaxed);
            }
            p
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: as for `alloc`.
            unsafe { System.dealloc(ptr, layout) };
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
    }

    #[global_allocator]
    static ALLOC: Counting = Counting;

    const MHO_IDN: &str = "RIGOL TECHNOLOGIES,MHO984,MHO9XXXXXXXXXX,00.01.00";

    /// 4 GSa/s, 50 mV/div, 0 V at code 9421; RAW mode (type 2).
    const PREAMBLE: &str =
        "1,2,4000000,1,2.500000E-10,-5.000000E-4,0.000000,6.6667E-06,-23347,32768";

    /// Sample `i` of the fake record: a 12-bit pattern that differs at every
    /// block edge.
    fn code(i: usize) -> u16 {
        ((i * 7919 + 13) % 4096) as u16 + 7373
    }

    /// An MHO984, stopped, with `depth` points of channel 1 in memory,
    /// answering RAW reads for any STARt/STOP the client sets.
    async fn stateful_mho(depth: usize) -> (Fake, Dho900) {
        let (mut start, mut stop) = (1usize, 1000usize);
        let fake = Fake::start_with(move |cmd| {
            let line = |s: &str| Some(Reply::line(s));
            if let Some(n) = cmd.strip_prefix(":WAVeform:STARt ") {
                start = n.parse().ok()?;
                return Some(Reply::None);
            }
            if let Some(n) = cmd.strip_prefix(":WAVeform:STOP ") {
                stop = n.parse().ok()?;
                return Some(Reply::None);
            }
            match cmd {
                "*IDN?" => line(MHO_IDN),
                ":CHANnel1:DISPlay?" => line("1"),
                ":CHANnel1:SCALe?" => line("5.000000E-02"),
                ":CHANnel1:OFFSet?" => line("0.000000"),
                ":CHANnel1:COUPling?" => line("DC"),
                ":CHANnel1:PROBe?" => line("1.000000E0"),
                ":CHANnel1:BWLimit?" => line("OFF"),
                ":CHANnel1:INVert?" => line("0"),
                ":CHANnel1:IMPedance?" => line("FIFT"),
                ":TRIGger:STATus?" => line("STOP"),
                ":TRIGger:SWEep?" => line("SINGle"),
                ":TRIGger:MODE?" => line("EDGE"),
                ":TRIGger:POSition?" => line("2.000000E+6"),
                ":ACQuire:MDEPth?" => line(&format!("{depth:E}")),
                ":ACQuire:SRATe?" => line("4.000000E+9"),
                ":ACQuire:TYPE?" => line("NORM"),
                ":TIMebase:MAIN:SCALe?" => line("1.000000E-4"),
                ":TIMebase:MAIN:OFFSet?" => line("0.000000"),
                ":WAVeform:PREamble?" => line(PREAMBLE),
                ":WAVeform:SOURce CHANnel1"
                | ":WAVeform:MODE RAW"
                | ":WAVeform:MODE NORMal"
                | ":WAVeform:FORMat WORD" => Some(Reply::None),
                ":WAVeform:DATA?" => {
                    let data: Vec<u8> = (start - 1..stop.min(depth))
                        .flat_map(|i| code(i).to_le_bytes())
                        .collect();
                    Some(Reply::block(&data, 64 * 1024))
                }
                _ => None,
            }
        })
        .await;
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

    /// A fresh, empty directory for one test.
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rigol-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn params(path: &Path, chunk_points: usize) -> SaveWaveformParams {
        SaveWaveformParams {
            channel: 1,
            path: path.display().to_string(),
            stop_if_running: None,
            expect_impedance: Some(InputImpedance::FiftyOhm),
            chunk_points: Some(chunk_points),
        }
    }

    /// One `.npy` member of an archive: its header dict and its data.
    fn npy(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> (String, Vec<u8>) {
        let mut f = zip.by_name(&format!("{name}.npy")).unwrap();
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).unwrap();
        assert_eq!(&bytes[..8], b"\x93NUMPY\x01\x00");
        let len = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        assert_eq!((10 + len) % 64, 0, "{name}: data not 64-byte aligned");
        let header = String::from_utf8(bytes[10..10 + len].to_vec()).unwrap();
        (header, bytes[10 + len..].to_vec())
    }

    fn f64s(b: &[u8]) -> Vec<f64> {
        b.as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes(*c))
            .collect()
    }

    /// Ten points in blocks of four land in the archive in order, scaled by
    /// the preamble, with the sidecar describing them and hashing the archive.
    #[tokio::test]
    async fn saves_codes_volts_time_and_a_sidecar() {
        let (fake, scope) = stateful_mho(10).await;
        let dir = scratch_dir("small");
        let path = dir.join("record.npz");

        let out = save_waveform(&scope, &params(&path, 4)).await.unwrap();

        assert_eq!(out["points"], json!(10));
        assert_eq!(out["chunks"], json!(3));
        assert_eq!(out["input_impedance"], json!("fifty_ohm"));
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();

        let (h, codes) = npy(&mut zip, "codes");
        assert!(
            h.contains("'descr': '<u2'") && h.contains("'shape': (10,)"),
            "{h}"
        );
        let codes: Vec<u16> = codes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        assert_eq!(codes, (0..10).map(code).collect::<Vec<_>>());

        let (_, volts) = npy(&mut zip, "volts");
        let volts = f64s(&volts);
        for (v, c) in volts.iter().zip(&codes) {
            assert_eq!(*v, (f64::from(*c) + 23347.0 - 32768.0) * 6.6667e-6);
        }
        let (_, time) = npy(&mut zip, "time");
        let time = f64s(&time);
        assert_eq!(time[0], -5.0e-4);
        assert!((time[9] - (-5.0e-4 + 9.0 * 2.5e-10)).abs() < 1e-18);

        let (h, z) = npy(&mut zip, "input_impedance_ohms");
        assert!(h.contains("'shape': ()"), "{h}");
        assert_eq!(f64s(&z), [50.0]);
        assert_eq!(f64s(&npy(&mut zip, "sample_rate").1), [4.0e9]);
        assert_eq!(f64s(&npy(&mut zip, "trigger_position").1), [2.0e6]);

        let sidecar: Value =
            serde_json::from_slice(&std::fs::read(dir.join("record.json")).unwrap()).unwrap();
        let (sha, bytes) = record::sha256_file(&path).unwrap();
        assert_eq!(sidecar["npz_sha256"], json!(sha));
        assert_eq!(sidecar["npz_bytes"], json!(bytes));
        assert_eq!(out["npz_sha256"], json!(sha));
        assert_eq!(sidecar["format"], json!(record::SIDECAR_FORMAT));
        assert_eq!(sidecar["instrument"]["identity"]["model"], json!("MHO984"));
        assert_eq!(sidecar["input_impedance_ohms"], json!(50.0));
        assert_eq!(sidecar["channel_settings"]["volts_per_div"], json!(0.05));
        assert_eq!(sidecar["transfer"]["stopped_for_read"], json!(false));
        assert_eq!(sidecar["preamble"]["x_increment"], json!(2.5e-10));
        // Nothing else is left in the directory: the spool is gone.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        fake.finish();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An existing `.npz` or `.json` is refused before the scope is asked
    /// anything, and is left exactly as it was.
    #[tokio::test]
    async fn refuses_to_overwrite_and_leaves_existing_files_alone() {
        let (fake, scope) = stateful_mho(10).await;
        let dir = scratch_dir("overwrite");
        let path = dir.join("record.npz");

        std::fs::write(&path, b"keep").unwrap();
        let err = save_waveform(&scope, &params(&path, 4)).await.unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
        assert!(!dir.join("record.json").exists());
        let mcp = crate::error::tool_error(err);
        assert_eq!(mcp.code, rmcp::model::ErrorCode::INVALID_PARAMS);

        std::fs::remove_file(&path).unwrap();
        std::fs::write(dir.join("record.json"), b"keep").unwrap();
        let err = save_waveform(&scope, &params(&path, 4)).await.unwrap_err();
        assert!(err.to_string().contains("record.json"), "{err}");
        assert_eq!(std::fs::read(dir.join("record.json")).unwrap(), b"keep");
        assert!(!path.exists(), "the npz this save created must go again");

        // Only *IDN? reached the instrument.
        fake.finish();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_a_relative_path_or_another_extension() {
        let (fake, scope) = stateful_mho(10).await;
        for bad in ["record.npz", "/tmp/record.npy"] {
            let mut p = params(Path::new(bad), 4);
            p.path = bad.into();
            let err = save_waveform(&scope, &p).await.unwrap_err();
            assert!(
                err.downcast_ref::<crate::error::Refused>().is_some(),
                "{err}"
            );
        }
        fake.finish();
    }

    /// A read the library refuses (here a 1 MΩ requirement on a 50 Ω input)
    /// leaves no file behind.
    #[tokio::test]
    async fn a_failed_read_leaves_no_files() {
        let (fake, scope) = stateful_mho(10).await;
        let dir = scratch_dir("failed");
        let path = dir.join("record.npz");
        let mut p = params(&path, 4);
        p.expect_impedance = Some(InputImpedance::OneMegohm);

        let err = save_waveform(&scope, &p).await.unwrap_err();

        assert!(err.to_string().contains("not the required 1 MΩ"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        fake.finish();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A 4 Mpts record, the length of a whole capture at 4 GSa/s, streams
    /// through in 250 kpts blocks without the heap ever holding even its
    /// codes (8 MB), let alone its volts (32 MB). Other tests share the
    /// allocator, so the bound has headroom; the record itself would blow it.
    #[tokio::test]
    async fn streams_a_large_record_in_bounded_memory() {
        const DEPTH: usize = 4_000_000;
        let (fake, scope) = stateful_mho(DEPTH).await;
        let dir = scratch_dir("large");
        let path = dir.join("record.npz");

        let base = LIVE.load(Ordering::Relaxed);
        PEAK.store(base, Ordering::Relaxed);
        let out = save_waveform(&scope, &params(&path, 250_000))
            .await
            .unwrap();
        let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);

        assert_eq!(out["points"], json!(DEPTH));
        assert_eq!(out["chunks"], json!(16));
        assert!(peak < 6_000_000, "peak heap above baseline: {peak} bytes");

        // Spot-check the far end of the record and every block edge.
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let (h, codes) = npy(&mut zip, "codes");
        assert!(h.contains(&format!("'shape': ({DEPTH},)")), "{h}");
        for i in (0..DEPTH)
            .step_by(250_000)
            .flat_map(|i| [i, i.saturating_sub(1)])
        {
            assert_eq!(
                u16::from_le_bytes([codes[2 * i], codes[2 * i + 1]]),
                code(i)
            );
        }
        assert_eq!(
            u16::from_le_bytes([codes[2 * DEPTH - 2], codes[2 * DEPTH - 1]]),
            code(DEPTH - 1)
        );
        fake.finish();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
