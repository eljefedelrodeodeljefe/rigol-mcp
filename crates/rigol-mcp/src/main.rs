use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

mod error;
mod params;
mod server;
mod tools;

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr — stdout is reserved for MCP JSON-RPC messages.
    tracing_subscriber::fmt()
        .with_env_filter(log_filter(std::env::var("RUST_LOG").ok()))
        .with_writer(std::io::stderr)
        .init();

    let server = server::RigolServer::from_env()?;
    tracing::info!("rigol-mcp starting");

    let svc = server.serve(stdio()).await?;
    svc.waiting().await?;

    Ok(())
}

/// `RUST_LOG` if set, otherwise info. `EnvFilter::from_default_env` with
/// no `RUST_LOG` hides info, and with it every "ENERGISING output" line.
fn log_filter(rust_log: Option<String>) -> EnvFilter {
    EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .parse_lossy(rust_log.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The energise and write lines are info level, and they are the audit
    /// trail; with no RUST_LOG they must still be written.
    #[test]
    fn logs_info_unless_rust_log_says_otherwise() {
        assert_eq!(log_filter(None).max_level_hint(), Some(LevelFilter::INFO));
        assert_eq!(
            log_filter(Some("warn".into())).max_level_hint(),
            Some(LevelFilter::WARN)
        );
    }
}
