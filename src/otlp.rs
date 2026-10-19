//! OTLP export: providers, the `tracing` layers that feed them, and the
//! one-warning-only reporter for export problems.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::{BoxLayer, Error, Service, env_filter, host};

/// How long the guard waits for each provider to flush on drop.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy)]
enum Signal {
    Logs,
    Traces,
    Metrics,
}

impl Signal {
    fn specific_var(self) -> &'static str {
        match self {
            Signal::Logs => "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            Signal::Traces => "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            Signal::Metrics => "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        }
    }
}

/// Export is on for a signal when its own endpoint variable or the base one is
/// set. The exporters read the values themselves; this only decides on/off.
fn enabled(signal: Signal) -> bool {
    let set = |var: &str| std::env::var(var).is_ok_and(|v| !v.trim().is_empty());
    set(signal.specific_var()) || set("OTEL_EXPORTER_OTLP_ENDPOINT")
}

fn resource(service: Service) -> Resource {
    let mut builder = Resource::builder();
    if std::env::var_os("OTEL_SERVICE_NAME").is_none() {
        builder = builder.with_service_name(service.name);
    }
    builder
        .with_attribute(KeyValue::new("service.version", service.version))
        .with_attribute(KeyValue::new("host.name", host::host_name()))
        .build()
}

#[derive(Default)]
pub(crate) struct Providers {
    tracer: Option<SdkTracerProvider>,
    logger: Option<SdkLoggerProvider>,
    pub(crate) meter: Option<SdkMeterProvider>,
}

impl Providers {
    /// Flush and stop every provider. Failures are ignored on purpose:
    /// telemetry never takes the service down.
    pub(crate) fn shutdown(&self) {
        if let Some(p) = &self.tracer {
            let _ = p.shutdown_with_timeout(SHUTDOWN_TIMEOUT);
        }
        if let Some(p) = &self.logger {
            let _ = p.shutdown_with_timeout(SHUTDOWN_TIMEOUT);
        }
        if let Some(p) = &self.meter {
            let _ = p.shutdown_with_timeout(SHUTDOWN_TIMEOUT);
        }
    }
}

fn exporter_err(e: impl std::fmt::Display) -> Error {
    Error::Exporter(e.to_string())
}

/// Build the providers for the signals that have an endpoint, and the layers
/// that feed them. Nothing is installed.
pub(crate) fn build<S>(service: Service) -> Result<(Vec<BoxLayer<S>>, Providers), Error>
where
    S: Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
{
    let resource = resource(service);
    let mut layers: Vec<BoxLayer<S>> = Vec::new();
    let mut providers = Providers::default();

    if enabled(Signal::Traces) {
        // The batch processors are bounded queues that drop when full.
        let exporter = SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(exporter_err)?;
        let provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(exporter)
            .build();
        let tracer = provider.tracer(service.name);
        layers.push(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(env_filter()?)
                .with_filter(filter_fn(|m| !is_otel_internal(m.target())))
                .boxed(),
        );
        providers.tracer = Some(provider);
    }

    if enabled(Signal::Logs) {
        let exporter = LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(exporter_err)?;
        let provider = SdkLoggerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(exporter)
            .build();
        // The bridge must never see the export stack's own events, or an
        // unreachable backend would log about itself forever.
        layers.push(
            OpenTelemetryTracingBridge::new(&provider)
                .with_filter(env_filter()?)
                .with_filter(filter_fn(|m| !is_otel_internal(m.target())))
                .boxed(),
        );
        providers.logger = Some(provider);
    }

    if enabled(Signal::Metrics) {
        let exporter = MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(exporter_err)?;
        let provider = SdkMeterProvider::builder()
            .with_resource(resource)
            .with_periodic_exporter(exporter)
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        providers.meter = Some(provider);
    }

    if providers.tracer.is_some() || providers.logger.is_some() || providers.meter.is_some() {
        layers.push(ExportWarning::default().boxed());
    }

    Ok((layers, providers))
}

/// Targets whose events come from the export path itself.
pub(crate) fn is_otel_internal(target: &str) -> bool {
    ["opentelemetry", "hyper", "reqwest", "h2"]
        .iter()
        .any(|p| target.starts_with(p))
}

/// Prints the first export problem to stderr and drops the rest, so a backend
/// that stays down costs one line, not one per batch.
#[derive(Default)]
struct ExportWarning {
    reported: AtomicBool,
}

impl<S: Subscriber> Layer<S> for ExportWarning {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() > Level::WARN || !meta.target().starts_with("opentelemetry") {
            return;
        }
        if self.reported.swap(true, Ordering::Relaxed) {
            return;
        }
        let mut message = Message::default();
        event.record(&mut message);
        eprintln!(
            "telemetry: export problem, further ones are not reported: {} {}",
            meta.name(),
            message.0
        );
    }
}

#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        self.0.push_str(&format!("{}={value:?}", field.name()));
    }
}
