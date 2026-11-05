//! Client-side control of RIGOL test instruments over their raw SCPI socket.
//!
//! Three layers, deliberately separable:
//!
//! - [`scpi::Session`] — the transport. A newline-framed ASCII line protocol over
//!   TCP, with one command in flight at a time.
//! - [`limits::Limits`] — per-channel safety ceilings, checked *here* rather than
//!   in any caller, so nothing above can bypass them.
//! - [`dp800::Dp800`] — the typed device model for the DP800 series.
//!
//! Deliberately not VXI-11: the instrument exposes ONC-RPC portmap on tcp/111,
//! but the raw socket is a line protocol with identical capability and none of
//! the XDR machinery.

pub mod dho900;
pub mod dp800;
pub mod error;
pub mod generator;
pub mod idn;
pub mod limits;
pub mod scpi;

pub use dho900::{
    Applied, Bucket, ChannelConfig, ChannelState, Coupling, Dho900, DigitalChannel, EdgeTrigger,
    EdgeTriggerConfig, GeneratorState, MeasureItem, MeasureStatistics, Preamble, Reading, ScopeLan,
    ScopeStatus, Slope, Sweep, Timebase, Waveform, WaveformStats,
};
pub use dp800::{Channel, Dp800, LanConfig, Measurement, OutputState, Protection, Rating};
pub use error::{Error, Result};
pub use generator::{GeneratorConfig, GeneratorLimits, Signal, Waveshape};
pub use idn::{Family, Identity};
pub use limits::Limits;
pub use scpi::{
    DEFAULT_MAX_BLOCK_BYTES, DEFAULT_PORT, DEFAULT_TIMEOUT, InstrumentError, Session, SessionConfig,
};
