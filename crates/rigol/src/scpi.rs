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
//! - A session that timed out or failed mid-exchange is *poisoned*: the
//!   instrument may still send the reply it owes, and pairing that late reply
//!   with the next query would shift every answer after it by one. A poisoned
//!   session reconnects before its next command. Draining is not an option —
//!   there is no way to know whether a late reply is still coming.

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
    conn: Mutex<Conn>,
}

/// The socket and whether it can still be trusted. Kept behind one lock so
/// that checking, reconnecting and using the socket are a single step.
struct Conn {
    io: BufReader<TcpStream>,
    poisoned: bool,
}

impl Session {
    pub async fn connect(addr: impl Into<String>, cfg: SessionConfig) -> Result<Self> {
        let addr = addr.into();
        let io = open(&addr, cfg.timeout).await?;
        tracing::debug!(%addr, "scpi session established");
        Ok(Self {
            addr,
            cfg,
            conn: Mutex::new(Conn {
                io,
                poisoned: false,
            }),
        })
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Re-establishes the socket in place, so a dropped link does not require a
    /// process restart.
    pub async fn reconnect(&self) -> Result<()> {
        let mut conn = self.conn.lock().await;
        conn.io = open(&self.addr, self.cfg.timeout).await?;
        conn.poisoned = false;
        tracing::info!(addr = %self.addr, "scpi session reconnected");
        Ok(())
    }

    /// Sends a command that produces no reply.
    pub async fn command(&self, cmd: &str) -> Result<()> {
        let mut conn = self.ready().await?;
        tracing::debug!(%cmd, "scpi command");
        let result = write_line(&mut conn.io, cmd).await;
        conn.poison_on_err(result)
    }

    /// Sends a query and reads exactly one line back.
    pub async fn query(&self, cmd: &str) -> Result<String> {
        let mut conn = self.ready().await?;
        tracing::debug!(%cmd, "scpi query");
        let result = async {
            write_line(&mut conn.io, cmd).await?;
            read_line(&mut conn.io, cmd, self.cfg.timeout).await
        }
        .await;
        conn.poison_on_err(result)
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

    /// Takes the lock and returns a connection that is safe to use, replacing
    /// a poisoned socket first. If the reconnect fails the session stays
    /// poisoned, so the next call tries again.
    async fn ready(&self) -> Result<tokio::sync::MutexGuard<'_, Conn>> {
        let mut conn = self.conn.lock().await;
        if conn.poisoned {
            conn.io = open(&self.addr, self.cfg.timeout).await?;
            conn.poisoned = false;
            tracing::info!(addr = %self.addr, "scpi session reconnected after a failed exchange");
        }
        Ok(conn)
    }
}

impl Conn {
    /// Any failure mid-exchange leaves the stream in an unknown position:
    /// part of a reply may be buffered, or a late one may still be on its way.
    fn poison_on_err<T>(&mut self, result: Result<T>) -> Result<T> {
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

async fn open(addr: &str, timeout: Duration) -> Result<BufReader<TcpStream>> {
    let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Timeout {
            cmd: format!("connect {addr}"),
            waited: timeout,
        })??;
    let _ = stream.set_nodelay(true);
    Ok(BufReader::new(stream))
}

async fn read_line(io: &mut BufReader<TcpStream>, cmd: &str, timeout: Duration) -> Result<String> {
    let mut line = String::new();
    let read = tokio::time::timeout(timeout, io.read_line(&mut line))
        .await
        .map_err(|_| Error::Timeout {
            cmd: cmd.to_string(),
            waited: timeout,
        })??;
    if read == 0 {
        return Err(Error::ConnectionClosed);
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

async fn write_line(io: &mut BufReader<TcpStream>, cmd: &str) -> Result<()> {
    let stream = io.get_mut();
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
