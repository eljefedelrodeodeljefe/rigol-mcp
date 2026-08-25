//! The transport core: a newline-framed ASCII line protocol over TCP.
//!
//! Everything here is shaped by what the instrument actually does, not by what
//! the programming guide says:
//!
//! - A timeout is its own error rather than an empty string: an empty reply and
//!   a too-short timeout are indistinguishable otherwise.
//! - tcp/5555 is the live port. tcp/5025, the LXI-conventional one, is closed.
//! - One command in flight at a time: request/response pairing on a single
//!   socket has no other safe option, and these PSUs tolerate few sessions.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::error::{Error, Result};

/// The DP832A's raw SCPI socket. Note this is *not* 5025 — that port is closed.
pub const DEFAULT_PORT: u16 = 5555;

/// Headroom, not necessity: measured latency is roughly 10-110 ms on a quiet
/// LAN. The margin is for a loaded network, not a slow instrument.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// One error popped from the instrument's `:SYSTem:ERRor?` queue.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct InstrumentError {
    pub code: i32,
    pub message: String,
}

impl InstrumentError {
    /// Parses `-113,"Undefined header; keyword cannot be found"`.
    pub fn parse(raw: &str) -> Option<Self> {
        let (code, message) = raw.split_once(',')?;
        Some(Self {
            code: code.trim().parse().ok()?,
            message: message.trim().trim_matches('"').to_string(),
        })
    }

    pub fn is_ok(&self) -> bool {
        self.code == 0
    }
}

pub struct Session {
    addr: String,
    cfg: SessionConfig,
    conn: Mutex<BufReader<TcpStream>>,
}

impl Session {
    pub async fn connect(addr: impl Into<String>, cfg: SessionConfig) -> Result<Self> {
        let addr = addr.into();
        let stream = tokio::time::timeout(cfg.timeout, TcpStream::connect(&addr))
            .await
            .map_err(|_| Error::Timeout {
                cmd: format!("connect {addr}"),
                waited: cfg.timeout,
            })??;
        let _ = stream.set_nodelay(true);
        tracing::debug!(%addr, "scpi session established");
        Ok(Self {
            addr,
            cfg,
            conn: Mutex::new(BufReader::new(stream)),
        })
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Re-establishes the socket in place, so a dropped link does not require a
    /// process restart.
    pub async fn reconnect(&self) -> Result<()> {
        let stream = tokio::time::timeout(self.cfg.timeout, TcpStream::connect(&self.addr))
            .await
            .map_err(|_| Error::Timeout {
                cmd: format!("reconnect {}", self.addr),
                waited: self.cfg.timeout,
            })??;
        let _ = stream.set_nodelay(true);
        *self.conn.lock().await = BufReader::new(stream);
        tracing::info!(addr = %self.addr, "scpi session reconnected");
        Ok(())
    }

    /// Sends a command that produces no reply.
    pub async fn command(&self, cmd: &str) -> Result<()> {
        let mut conn = self.conn.lock().await;
        tracing::debug!(%cmd, "scpi command");
        write_line(&mut conn, cmd).await
    }

    /// Sends a query and reads exactly one line back.
    pub async fn query(&self, cmd: &str) -> Result<String> {
        let mut conn = self.conn.lock().await;
        tracing::debug!(%cmd, "scpi query");
        write_line(&mut conn, cmd).await?;

        let mut line = String::new();
        let read = tokio::time::timeout(self.cfg.timeout, conn.read_line(&mut line))
            .await
            .map_err(|_| Error::Timeout {
                cmd: cmd.to_string(),
                waited: self.cfg.timeout,
            })??;

        if read == 0 {
            return Err(Error::ConnectionClosed);
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    /// Drains `:SYSTem:ERRor?` until the queue reports `0,"No error"`.
    ///
    /// Worth calling after any command that might have been rejected: this
    /// firmware answers undocumented headers with silence and only records the
    /// `-113` here. Bounded so a misbehaving instrument cannot spin us forever.
    pub async fn errors(&self) -> Result<Vec<InstrumentError>> {
        let mut found = Vec::new();
        for _ in 0..32 {
            let raw = self.query(":SYST:ERR?").await?;
            let Some(err) = InstrumentError::parse(&raw) else {
                break;
            };
            if err.is_ok() {
                break;
            }
            found.push(err);
        }
        Ok(found)
    }

    /// Clears the status registers and the error queue.
    pub async fn clear_status(&self) -> Result<()> {
        self.command("*CLS").await
    }
}

async fn write_line(conn: &mut BufReader<TcpStream>, cmd: &str) -> Result<()> {
    let stream = conn.get_mut();
    stream.write_all(cmd.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_instrument_error() {
        let e = InstrumentError::parse(r#"-113,"Undefined header; keyword cannot be found""#)
            .expect("should parse");
        assert_eq!(e.code, -113);
        assert_eq!(e.message, "Undefined header; keyword cannot be found");
        assert!(!e.is_ok());
    }

    #[test]
    fn recognises_the_empty_queue() {
        let e = InstrumentError::parse(r#"0,"No error""#).expect("should parse");
        assert!(e.is_ok());
    }

    #[test]
    fn rejects_junk() {
        assert!(InstrumentError::parse("not an error line").is_none());
    }
}
