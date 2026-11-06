//! A scripted fake instrument for exercising the real `rigol::Session` over TCP.
//!
//! Test-only: this crate is a dev-dependency and is never published.
//!
//! The fake plays a fixed script of `(expected command, reply)` steps, in
//! order, across however many connections the client opens, so reconnects
//! are part of what a test can observe. A command that does not match the
//! next step is recorded as a failure and gets no reply, and
//! [`Fake::finish`] fails the test if any step was missed or mismatched.
//!
//! Where the order of commands is not fixed, as with concurrent callers,
//! [`Fake::start_with`] answers each command from a function instead, which
//! can model the instrument's state.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::tcp::OwnedWriteHalf;

/// What the fake sends back for one step.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A line, newline-terminated by the fake.
    Line(String),
    /// Nothing at all, as when firmware ignores a query it does not know.
    Silence,
    /// A line sent after a delay — a reply that outlives the client's timeout.
    LateLine(Duration, String),
    /// Bytes written exactly as given, in chunks of the given size.
    Raw { bytes: Vec<u8>, chunk: usize },
    /// No reply expected (a command, not a query).
    None,
}

impl Reply {
    pub fn line(s: &str) -> Self {
        Self::Line(s.to_string())
    }

    /// An IEEE 488.2 definite-length block with a trailing newline, as the
    /// instrument sends it, split into `chunk`-byte writes.
    pub fn block(payload: &[u8], chunk: usize) -> Self {
        let len = payload.len().to_string();
        let mut bytes = format!("#{}{}", len.len(), len).into_bytes();
        bytes.extend_from_slice(payload);
        bytes.push(b'\n');
        Self::Raw { bytes, chunk }
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    pub expect: String,
    pub reply: Reply,
}

pub fn step(expect: &str, reply: Reply) -> Step {
    Step {
        expect: expect.to_string(),
        reply,
    }
}

/// Answers one command, or `None` for one the instrument would not know.
pub type Handler = Box<dyn FnMut(&str) -> Option<Reply> + Send>;

#[derive(Default)]
struct State {
    script: VecDeque<Step>,
    handler: Option<Handler>,
    failures: Vec<String>,
    connections: usize,
}

pub struct Fake {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl Fake {
    pub async fn start(script: Vec<Step>) -> Self {
        Self::serve(State {
            script: script.into(),
            ..State::default()
        })
        .await
    }

    /// Answers every command with `handler` instead of a script. A `None`
    /// from it is recorded as a failure and gets no reply.
    pub async fn start_with(handler: impl FnMut(&str) -> Option<Reply> + Send + 'static) -> Self {
        Self::serve(State {
            handler: Some(Box::new(handler)),
            ..State::default()
        })
        .await
    }

    async fn serve(state: State) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let state = Arc::new(Mutex::new(state));

        let shared = state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                shared.lock().unwrap().connections += 1;
                let shared = shared.clone();
                tokio::spawn(async move {
                    let (mut read, mut write) = stream.into_split();
                    let mut pending: Vec<u8> = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = match read.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        pending.extend_from_slice(&buf[..n]);
                        // One read can carry several commands; answer each.
                        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
                            let line: Vec<u8> = pending.drain(..=pos).collect();
                            let cmd = String::from_utf8_lossy(&line).trim_end().to_string();
                            let next = shared.lock().unwrap().next_reply(&cmd);
                            if let Some(reply) = next {
                                respond(&mut write, reply).await;
                            }
                        }
                    }
                });
            }
        });

        Self { addr, state }
    }

    pub fn addr(&self) -> String {
        self.addr.to_string()
    }

    /// How many TCP connections the client has opened so far.
    pub fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }

    /// Fails the test unless every step ran and nothing unexpected arrived.
    pub fn finish(&self) {
        let st = self.state.lock().unwrap();
        assert!(
            st.failures.is_empty(),
            "fake instrument saw: {:?}",
            st.failures
        );
        let left: Vec<_> = st.script.iter().map(|s| s.expect.clone()).collect();
        assert!(left.is_empty(), "steps never reached: {left:?}");
    }
}

impl State {
    fn next_reply(&mut self, cmd: &str) -> Option<Reply> {
        if let Some(handler) = self.handler.as_mut() {
            let reply = handler(cmd);
            if reply.is_none() {
                self.failures.push(format!("unhandled command `{cmd}`"));
            }
            return reply;
        }
        match self.script.front() {
            Some(s) if s.expect == cmd => self.script.pop_front().map(|s| s.reply),
            Some(s) => {
                let msg = format!("expected `{}`, got `{cmd}`", s.expect);
                self.failures.push(msg);
                None
            }
            None => {
                self.failures.push(format!("unscripted command `{cmd}`"));
                None
            }
        }
    }
}

async fn respond(write: &mut OwnedWriteHalf, reply: Reply) {
    match reply {
        Reply::Line(s) => {
            let _ = write.write_all(format!("{s}\n").as_bytes()).await;
        }
        Reply::Silence | Reply::None => {}
        Reply::LateLine(after, s) => {
            // Delay in place: the fake answers one command at a time, like the
            // instrument, so a later command on this socket waits behind it.
            tokio::time::sleep(after).await;
            let _ = write.write_all(format!("{s}\n").as_bytes()).await;
        }
        Reply::Raw { bytes, chunk } => {
            for part in bytes.chunks(chunk.max(1)) {
                if write.write_all(part).await.is_err() {
                    return;
                }
                let _ = write.flush().await;
                tokio::task::yield_now().await;
            }
        }
    }
}
