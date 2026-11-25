use rmcp::model::{ErrorCode, ErrorData};

/// Maps a tool failure onto an MCP error.
///
/// A safety refusal is deliberately reported as an *invalid parameter* rather
/// than an internal error: the caller asked for something the instrument is not
/// allowed to do, which is a bad request, not a malfunction. Getting this right
/// is what lets a model understand it should lower the setpoint rather than
/// retry the same call.
pub fn tool_error(e: anyhow::Error) -> ErrorData {
    if let Some(refused) = e.downcast_ref::<Refused>() {
        return ErrorData::new(ErrorCode::INVALID_PARAMS, refused.to_string(), None);
    }
    if let Some(err) = e.downcast_ref::<rigol::Error>() {
        let code = match err {
            rigol::Error::LimitExceeded { .. }
            | rigol::Error::NoSuchChannel(_)
            | rigol::Error::NoSuchInput { .. }
            | rigol::Error::OutOfRange { .. }
            | rigol::Error::NotOnThisModel { .. }
            | rigol::Error::Conflicting(_)
            | rigol::Error::GeneratorLimit { .. }
            | rigol::Error::WrongImpedance { .. } => ErrorCode::INVALID_PARAMS,
            // Misconfiguration, not a malfunction: RIGOL_FAMILY names one kind
            // of instrument and another one answered at RIGOL_HOST.
            rigol::Error::WrongInstrument { .. } => ErrorCode::INVALID_REQUEST,
            _ => ErrorCode::INTERNAL_ERROR,
        };
        return ErrorData::new(code, err.to_string(), None);
    }
    ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None)
}

/// A request refused by the server itself rather than the library, such as
/// a host path that already exists. Reported as invalid parameters, like a
/// library refusal.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused: {}", self.0)
    }
}

impl std::error::Error for Refused {}
