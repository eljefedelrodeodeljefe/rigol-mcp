use std::time::Duration;

use crate::idn::Family;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error talking to the instrument: {0}")]
    Io(#[from] std::io::Error),

    /// Distinct from an empty reply on purpose. The DP832A takes 1-2 seconds to
    /// answer; a short timeout returns nothing and is easily mistaken for a
    /// closed port. Callers need to tell those apart.
    #[error("timed out after {waited:?} waiting for a reply to `{cmd}`")]
    Timeout { cmd: String, waited: Duration },

    #[error("the instrument closed the connection")]
    ConnectionClosed,

    /// The reply broke the framing the command calls for, e.g. a binary block
    /// whose header is malformed.
    #[error("malformed reply to `{cmd}`: {detail}")]
    Protocol { cmd: String, detail: String },

    /// A binary block declared a length above the session's cap. Checked
    /// before allocating, so a corrupt header cannot exhaust memory.
    #[error("reply to `{cmd}` declares a {len}-byte block, above the {cap}-byte cap")]
    BlockTooLarge { cmd: String, len: usize, cap: usize },

    #[error("could not parse the reply to `{cmd}`: {reply:?}")]
    Parse { cmd: String, reply: String },

    /// The instrument that answered is not the family it was opened as.
    /// Raised before any family-specific command is sent.
    #[error("expected a {expected} instrument, but `{model}` answered")]
    WrongInstrument { expected: Family, model: String },

    #[error("channel {0} does not exist (this instrument has channels 1-3)")]
    NoSuchChannel(u8),

    #[error("channel {channel} does not exist (this instrument has channels 1-{count})")]
    NoSuchInput { channel: u8, count: u8 },

    /// The safety ceiling. Enforced here in the library, below the MCP layer,
    /// so no tool argument can route around it.
    #[error(
        "refused: {what} {requested}{unit} on channel {channel} exceeds the configured ceiling of {ceiling}{unit}"
    )]
    LimitExceeded {
        what: &'static str,
        unit: &'static str,
        channel: u8,
        requested: f64,
        ceiling: f64,
    },

    #[error("instrument reported error {code}: {message}")]
    Instrument { code: i32, message: String },
}
