//! One shared setup for logs, spans and metrics in Pedro's Rust services.
//!
//! ```no_run
//! let _telemetry = telemetry::init(telemetry::service!("ghola"))?;
//! # Ok::<(), telemetry::Error>(())
//! ```

mod host;
mod otlp;

pub use opentelemetry;
pub use opentelemetry_sdk;

use std::fmt;
use std::io::IsTerminal;

use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// A boxed layer, as returned by [`layers`].
pub type BoxLayer<S> = Box<dyn Layer<S> + Send + Sync + 'static>;

/// Name and version of the service being instrumented. Build it with [`service!`].
#[derive(Debug, Clone, Copy)]
pub struct Service {
    pub name: &'static str,
    pub version: &'static str,
}

/// Expands to the service name plus the *caller's* `CARGO_PKG_VERSION`.
#[macro_export]
macro_rules! service {
    ($name:expr) => {
        $crate::Service {
            name: $name,
            version: env!("CARGO_PKG_VERSION"),
        }
    };
}

/// A bug in the configuration, never a network problem.
#[derive(Debug)]
pub enum Error {
    /// `RUST_LOG` could not be parsed.
    Filter(tracing_subscriber::filter::ParseError),
    /// A global subscriber was already installed.
    Install(tracing_subscriber::util::TryInitError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Filter(e) => write!(f, "invalid RUST_LOG: {e}"),
            Error::Install(e) => write!(f, "installing tracing subscriber: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Filter(e) => Some(e),
            Error::Install(e) => Some(e),
        }
    }
}

/// Keep for the life of the process. Dropping it flushes every exporter, and
/// `std::process::exit` skips the drop, so avoid it.
#[must_use = "dropping the guard flushes and stops telemetry"]
pub struct Guard {
    service: Service,
    host: String,
    providers: otlp::Providers,
}

impl Guard {
    pub fn service(&self) -> Service {
        self.service
    }

    /// Short host name (`arrakis`, `caladan`, `imperium`).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The global `MeterProvider`, or `None` when no metrics endpoint is set.
    /// `opentelemetry::global::meter` reaches the same provider.
    pub fn meter_provider(&self) -> Option<&opentelemetry_sdk::metrics::SdkMeterProvider> {
        self.providers.meter.as_ref()
    }
}

impl fmt::Debug for Guard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Guard")
            .field("service", &self.service)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.providers.shutdown();
    }
}

/// `RUST_LOG`, defaulting to `info`. A set but broken value is a config bug.
fn env_filter() -> Result<EnvFilter, Error> {
    match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => EnvFilter::try_new(v).map_err(Error::Filter),
        _ => Ok(EnvFilter::new("info")),
    }
}

/// The OTLP layers (spans, logs) without installing anything, for services
/// that compose their own subscriber. Each layer carries its own `RUST_LOG`
/// filter, so it does not change what your other layers see. Metrics are set
/// up as a side effect (see [`Guard::meter_provider`]).
///
/// Never empty: with no endpoint set it holds one quiet layer. (An empty
/// `Vec<Layer>` would tell `tracing` that nothing is enabled and silence your
/// other layers.) A signal whose exporter cannot be built is switched off with
/// a line on stderr. Keep the [`Guard`] alive.
pub fn layers<S>(service: Service) -> Result<(Vec<BoxLayer<S>>, Guard), Error>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
{
    let (layers, providers) = otlp::build(service)?;
    let guard = Guard {
        service,
        host: host::host_name(),
        providers,
    };
    Ok((layers, guard))
}

/// Installs the global `tracing` subscriber: `RUST_LOG` filtering, a stderr
/// layer, and the OTLP layers for each signal that has an endpoint. With none
/// set, stderr only.
pub fn init(service: Service) -> Result<Guard, Error> {
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_filter(env_filter()?)
        .with_filter(filter_fn(|m| !otlp::is_otel_internal(m.target())));

    let (otlp_layers, guard) = layers(service)?;

    tracing_subscriber::registry()
        .with(stderr)
        .with(otlp_layers)
        .try_init()
        .map_err(Error::Install)?;

    Ok(guard)
}

#[cfg(test)]
mod tests {
    #[test]
    fn service_macro_uses_callers_version() {
        let s = service!("demo");
        assert_eq!(s.name, "demo");
        assert_eq!(s.version, env!("CARGO_PKG_VERSION"));
    }
}
