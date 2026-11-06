//! Client-side control of RIGOL test instruments over their raw SCPI socket.
//!
//! Three layers, deliberately separable:
//!
//! - [`scpi::Session`] — the transport. A newline-framed ASCII line protocol over
//!   TCP, with one command in flight at a time. It checks nothing: a command
//!   sent through a session you open yourself is not held to any ceiling.
//! - [`limits::Limits`] and [`generator::GeneratorLimits`] — safety ceilings,
//!   checked *here* rather than in any caller, and built only through
//!   validating constructors.
//! - [`dp800::Dp800`] and [`dho900::Dho900`] — the typed device models. They
//!   keep their session private, so nothing that drives an instrument through
//!   one can bypass its ceilings.
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
    EdgeTriggerConfig, GeneratorState, LA_THRESHOLD_LIMIT_VOLTS, LogicAnalyzer,
    LogicAnalyzerConfig, MeasureItem, MeasureStatistics, Preamble, Reading, ScopeLan, ScopeStatus,
    Slope, Sweep, Timebase, Waveform, WaveformStats,
};
pub use dp800::{Channel, Dp800, LanConfig, Measurement, OutputState, Protection, Rating};
pub use error::{Error, Result};
pub use generator::{GeneratorConfig, GeneratorLimits, Signal, Waveshape};
pub use idn::{Family, Identity};
pub use limits::Limits;
pub use scpi::{
    DEFAULT_MAX_BLOCK_BYTES, DEFAULT_PORT, DEFAULT_TIMEOUT, InstrumentError, Session, SessionConfig,
};
