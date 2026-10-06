use rmcp::model::{ErrorCode, ErrorData};

/// Maps a tool failure onto an MCP error.
///
/// A safety refusal is deliberately reported as an *invalid parameter* rather
/// than an internal error: the caller asked for something the instrument is not
/// allowed to do, which is a bad request, not a malfunction. Getting this right
/// is what lets a model understand it should lower the setpoint rather than
/// retry the same call.
pub fn tool_error(e: anyhow::Error) -> ErrorData {
    if let Some(err) = e.downcast_ref::<rigol::Error>() {
        let code = match err {
            rigol::Error::LimitExceeded { .. } | rigol::Error::NoSuchChannel(_) => {
                ErrorCode::INVALID_PARAMS
            }
            // Misconfiguration, not a malfunction: RIGOL_FAMILY names one kind
            // of instrument and another one answered at RIGOL_HOST.
            rigol::Error::WrongInstrument { .. } => ErrorCode::INVALID_REQUEST,
            _ => ErrorCode::INTERNAL_ERROR,
        };
        return ErrorData::new(code, err.to_string(), None);
    }
    ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None)
}
