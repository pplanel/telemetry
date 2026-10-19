//! `layers()` for services that compose their own subscriber (airshell).
//!
//! One test only: it edits the process environment, which is not safe next to
//! other threads reading it.

use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn layers_follow_the_endpoint_and_install_nothing() {
    // SAFETY: this is the only test in this binary, so no other thread reads
    // the environment while it changes.
    unsafe {
        for var in [
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        ] {
            std::env::remove_var(var);
        }
    }

    let (none, guard) = telemetry::layers::<Registry>(telemetry::service!("airshell")).unwrap();
    assert_eq!(none.len(), 1, "no endpoint: only the quiet layer");
    assert!(guard.meter_provider().is_none());
    drop(guard);

    // Only the traces endpoint: the traces layer, plus the quiet one.
    unsafe {
        std::env::set_var(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "http://127.0.0.1:1/v1/traces",
        )
    };
    let (traces_only, guard) =
        telemetry::layers::<Registry>(telemetry::service!("airshell")).unwrap();
    assert_eq!(traces_only.len(), 2);
    assert!(guard.meter_provider().is_none());
    drop(guard);

    // The base endpoint turns on all three signals; metrics add no layer.
    unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:1") };
    let (all, guard) = telemetry::layers::<Registry>(telemetry::service!("airshell")).unwrap();
    assert_eq!(all.len(), 3);
    assert!(guard.meter_provider().is_some());

    // Composed into the caller's own registry, scoped, nothing global.
    let subscriber = Registry::default().with(all);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!("connection", airshell.peer = "imperium")
            .in_scope(|| tracing::info!("closed"));
    });
    drop(guard);
}
