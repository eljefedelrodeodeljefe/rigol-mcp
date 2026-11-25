//! Writing a full scope record to the host: an `.npz` with the raw codes,
//! the derived volts and time axis, the scaling as scalars, and a JSON
//! sidecar beside it.
//!
//! The file format is NumPy's: an `.npz` is a ZIP archive of `.npy` files,
//! stored uncompressed as `numpy.savez` writes it, and `numpy.load` reads it
//! back as a dict of arrays. An `.npy` file is a short header (magic, a
//! version, and a Python dict literal giving dtype and shape, padded to 64
//! bytes) followed by the array's bytes. That header is written here; the
//! ZIP container, with the ZIP64 records a member above 4 GiB needs, comes
//! from the `zip` crate with every compression feature off.
//!
//! Nothing here holds a record in memory. The codes arrive from the scope
//! block by block and go to a spool file; the archive is then built from the
//! spool, a block at a time, and hashed by reading it back.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rigol::{Family, Identity, MemoryRecord};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::error::Refused;

/// Samples converted per step when building the archive from the spool.
const BLOCK_SAMPLES: usize = 64 * 1024;

/// Names the sidecar's `format` field, for readers to check.
pub const SIDECAR_FORMAT: &str = "rigol-mcp/save_waveform/1";

/// The files one save produces, reserved before the instrument is touched.
///
/// Creating them with `create_new` is the overwrite check: it fails if a
/// file exists, and no other writer can take the name between the check
/// and the write. Until [`Target::finish`] succeeds, dropping this removes
/// all of them, so a failed save leaves nothing behind.
pub struct Target {
    pub npz: PathBuf,
    pub sidecar: PathBuf,
    spool_path: PathBuf,
    npz_file: Option<File>,
    sidecar_file: Option<File>,
    spool: Option<BufWriter<File>>,
    /// Which of the files this save created, and so may remove: never one
    /// that was already there.
    created: [bool; 3],
    committed: bool,
}

impl Target {
    /// Checks `path` (absolute, ending in `.npz`, in an existing
    /// directory) and creates the `.npz`, its `.json` sidecar and a spool
    /// file beside them, refusing if any of them exists.
    pub fn reserve(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Refused(format!("path must be absolute: {}", path.display())).into());
        }
        if path.extension().and_then(|e| e.to_str()) != Some("npz") {
            return Err(Refused(format!("path must end in .npz: {}", path.display())).into());
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Refused(format!("not a file name: {}", path.display())))?;
        let dir = path
            .parent()
            .ok_or_else(|| Refused(format!("no directory in {}", path.display())))?;
        if !dir.is_dir() {
            return Err(Refused(format!("directory does not exist: {}", dir.display())).into());
        }
        let sidecar = path.with_extension("json");
        let spool_path = dir.join(format!(".{name}.codes.partial"));

        let mut target = Self {
            npz: path.to_path_buf(),
            sidecar,
            spool_path,
            npz_file: None,
            sidecar_file: None,
            spool: None,
            created: [false; 3],
            committed: false,
        };
        target.npz_file = Some(create_new(&target.npz)?);
        target.created[0] = true;
        target.sidecar_file = Some(create_new(&target.sidecar)?);
        target.created[1] = true;
        target.spool = Some(BufWriter::new(create_new(&target.spool_path)?));
        target.created[2] = true;
        Ok(target)
    }

    /// Appends one block of raw codes to the spool, little-endian.
    pub fn spool_codes(&mut self, codes: &[u16], scratch: &mut Vec<u8>) -> io::Result<()> {
        let spool = self
            .spool
            .as_mut()
            .ok_or_else(|| io::Error::other("spool already closed"))?;
        scratch.clear();
        scratch.extend(codes.iter().flat_map(|c| c.to_le_bytes()));
        spool.write_all(scratch)
    }

    /// Builds the `.npz` from the spool, hashes it, writes the sidecar, and
    /// removes the spool. Blocking file I/O: run it off the async runtime.
    pub fn finish(
        mut self,
        identity: &Identity,
        family: Family,
        record: &MemoryRecord,
    ) -> Result<Value> {
        let spool = self.spool.take().context("spool already closed")?;
        let spool = spool.into_inner().map_err(|e| e.into_error())?;
        spool.sync_all()?;
        drop(spool);

        let npz_file = self.npz_file.take().context("npz already written")?;
        write_npz(BufWriter::new(npz_file), &self.spool_path, record)
            .with_context(|| format!("writing {}", self.npz.display()))?;
        let (sha256, npz_bytes) = sha256_file(&self.npz)?;

        let sidecar = sidecar(&self.npz, &sha256, npz_bytes, identity, family, record);
        let mut out = BufWriter::new(
            self.sidecar_file
                .take()
                .context("sidecar already written")?,
        );
        serde_json::to_writer_pretty(&mut out, &sidecar)?;
        out.write_all(b"\n")?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;

        std::fs::remove_file(&self.spool_path)?;
        self.created[2] = false;
        self.committed = true;
        Ok(json!({
            "npz": self.npz.display().to_string(),
            "sidecar": self.sidecar.display().to_string(),
            "npz_sha256": sha256,
            "npz_bytes": npz_bytes,
        }))
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        // Close before removing.
        self.spool.take();
        self.npz_file.take();
        self.sidecar_file.take();
        let [npz, sidecar, spool] = self.created;
        if spool {
            let _ = std::fs::remove_file(&self.spool_path);
        }
        if !self.committed {
            if npz {
                let _ = std::fs::remove_file(&self.npz);
            }
            if sidecar {
                let _ = std::fs::remove_file(&self.sidecar);
            }
        }
    }
}

fn create_new(path: &Path) -> Result<File> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            Err(Refused(format!("refusing to overwrite {}", path.display())).into())
        }
        Err(e) => Err(anyhow::Error::new(e).context(format!("creating {}", path.display()))),
    }
}

/// The archive: `codes` (`<u2`), `volts` and `time` (`<f8`), each one value
/// per sample, then the scaling and settings as 0-d arrays.
fn write_npz(out: BufWriter<File>, spool: &Path, r: &MemoryRecord) -> Result<()> {
    let n = r.points;
    let p = &r.preamble;
    let mut zip = ZipWriter::new(out);

    let mut codes = BufReader::new(File::open(spool)?);
    start_member(&mut zip, "codes", "<u2", Some(n), 2)?;
    let copied = io::copy(&mut (&mut codes).take(2 * n as u64), &mut zip)?;
    if copied != 2 * n as u64 {
        anyhow::bail!("spool holds {copied} bytes, expected {}", 2 * n);
    }

    let mut codes = BufReader::new(File::open(spool)?);
    start_member(&mut zip, "volts", "<f8", Some(n), 8)?;
    let mut raw = vec![0u8; 2 * BLOCK_SAMPLES];
    let mut out = Vec::with_capacity(8 * BLOCK_SAMPLES);
    let mut left = n;
    while left > 0 {
        let take = left.min(BLOCK_SAMPLES);
        codes.read_exact(&mut raw[..2 * take])?;
        out.clear();
        for w in raw[..2 * take].as_chunks::<2>().0 {
            out.extend_from_slice(&p.volts(u16::from_le_bytes(*w)).to_le_bytes());
        }
        zip.write_all(&out)?;
        left -= take;
    }

    start_member(&mut zip, "time", "<f8", Some(n), 8)?;
    let mut i = 0;
    while i < n {
        let end = (i + BLOCK_SAMPLES).min(n);
        out.clear();
        for k in i..end {
            out.extend_from_slice(&p.seconds(k).to_le_bytes());
        }
        zip.write_all(&out)?;
        i = end;
    }

    let scalars_f64 = [
        ("x_increment", p.x_increment),
        ("x_origin", p.x_origin),
        ("x_reference", p.x_reference),
        ("y_increment", p.y_increment),
        ("y_origin", p.y_origin),
        ("y_reference", p.y_reference),
        ("sample_rate", r.sample_rate),
        ("volts_per_div", r.channel_state.volts_per_div),
        ("offset_volts", r.channel_state.offset_volts),
        ("probe_ratio", r.channel_state.probe_ratio),
        ("input_impedance_ohms", r.input_impedance.ohms()),
        ("seconds_per_div", r.seconds_per_div),
        ("timebase_offset_seconds", r.timebase_offset_seconds),
        ("trigger_position", r.trigger_position.unwrap_or(f64::NAN)),
    ];
    for (name, v) in scalars_f64 {
        start_member(&mut zip, name, "<f8", None, 8)?;
        zip.write_all(&v.to_le_bytes())?;
    }
    let scalars_i64 = [
        ("channel", i64::from(r.channel)),
        ("points", i64::try_from(n)?),
        ("memory_depth", i64::try_from(r.memory_depth)?),
    ];
    for (name, v) in scalars_i64 {
        start_member(&mut zip, name, "<i8", None, 8)?;
        zip.write_all(&v.to_le_bytes())?;
    }

    let out = zip.finish()?;
    out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(())
}

/// Opens `<name>.npy` in the archive and writes its header. `len` is the
/// length of a 1-d array, `None` a 0-d one.
fn start_member<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    name: &str,
    descr: &str,
    len: Option<usize>,
    item_size: usize,
) -> Result<()> {
    let header = npy_header(descr, len);
    let size = header.len() as u64 + (len.unwrap_or(1) * item_size) as u64;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(size >= u64::from(u32::MAX));
    zip.start_file(format!("{name}.npy"), options)?;
    zip.write_all(&header)?;
    Ok(())
}

/// An NPY version 1.0 header for a little-endian array of `descr`, 1-d of
/// `len` items or 0-d, padded so the data starts on a 64-byte boundary as
/// NumPy's own writer does.
pub fn npy_header(descr: &str, len: Option<usize>) -> Vec<u8> {
    let shape = match len {
        Some(n) => format!("({n},)"),
        None => "()".to_string(),
    };
    let mut dict = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
    // magic (6) + version (2) + header length (2) + dict + '\n'
    let unpadded = 10 + dict.len() + 1;
    dict.push_str(&" ".repeat(unpadded.next_multiple_of(64) - unpadded));
    dict.push('\n');
    let mut out = Vec::with_capacity(10 + dict.len());
    out.extend_from_slice(b"\x93NUMPY\x01\x00");
    let len = u16::try_from(dict.len()).expect("a short header");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(dict.as_bytes());
    out
}

/// SHA-256 of a file, as lower-case hex, and its length, read a block at a
/// time.
pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    let hex = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok((hex, total))
}

fn sidecar(
    npz: &Path,
    sha256: &str,
    npz_bytes: u64,
    identity: &Identity,
    family: Family,
    r: &MemoryRecord,
) -> Value {
    let saved_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(f64::NAN);
    json!({
        "format": SIDECAR_FORMAT,
        "npz": npz.file_name().map(|n| n.to_string_lossy().into_owned()),
        "npz_sha256": sha256,
        "npz_bytes": npz_bytes,
        "saved_unix_seconds": saved_unix_seconds,
        "instrument": {
            "family": family.to_string(),
            "identity": identity,
        },
        "channel": r.channel,
        "points": r.points,
        "memory_depth": r.memory_depth,
        "transfer": {
            "mode": "RAW",
            "format": "WORD",
            "chunk_points": r.chunk_points,
            "chunks": r.chunks,
            "stopped_for_read": r.stopped_for_read,
            "restored_with": r.restored_with,
            "trigger_status_before": r.trigger_status_before,
        },
        "preamble": r.preamble,
        "preamble_raw": r.preamble_raw,
        "channel_settings": r.channel_state,
        "input_impedance": r.input_impedance,
        "input_impedance_ohms": r.input_impedance.ohms(),
        "acquisition": {
            "sample_rate": r.sample_rate,
            "type": r.acquisition_type,
        },
        "timebase": {
            "seconds_per_div": r.seconds_per_div,
            "offset_seconds": r.timebase_offset_seconds,
        },
        "trigger": {
            "mode": r.trigger_mode,
            "sweep": r.trigger_sweep,
            "position": r.trigger_position,
        },
        "arrays": {
            "codes": "<u2, raw WORD sample codes as read",
            "volts": "<f8, (codes - y_origin - y_reference) * y_increment",
            "time": "<f8, seconds: x_origin + (i - x_reference) * x_increment",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header NumPy itself writes for `np.save(f, np.zeros(3, '<u2'))`.
    #[test]
    fn writes_the_header_numpy_writes() {
        let h = npy_header("<u2", Some(3));
        assert_eq!(h.len(), 128);
        assert_eq!(&h[..8], b"\x93NUMPY\x01\x00");
        assert_eq!(u16::from_le_bytes([h[8], h[9]]), 118);
        let dict = std::str::from_utf8(&h[10..]).unwrap();
        assert!(dict.starts_with("{'descr': '<u2', 'fortran_order': False, 'shape': (3,), }"));
        assert!(dict.ends_with(" \n"));
        let scalar = npy_header("<f8", None);
        assert_eq!(scalar.len() % 64, 0);
        assert!(
            std::str::from_utf8(&scalar[10..])
                .unwrap()
                .contains("'shape': ()")
        );
    }

    #[test]
    fn hashes_a_file() {
        let dir = std::env::temp_dir().join(format!("rigol-mcp-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("abc");
        std::fs::write(&f, b"abc").unwrap();
        let (hex, n) = sha256_file(&f).unwrap();
        assert_eq!(n, 3);
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
