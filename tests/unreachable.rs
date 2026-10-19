//! With an endpoint nobody listens on, `init()` still succeeds inside a Tokio
//! runtime, the service keeps running, and export problems cost one stderr line.
//!
//! The subscriber is global, so the work runs in a child process: this test
//! re-executes itself with `CHILD=1`.

use std::process::Command;

#[test]
fn unreachable_backend_costs_one_warning() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child", "--nocapture", "--test-threads=1"])
        .env("CHILD", "1")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:1")
        .env("OTEL_SERVICE_NAME", "probe")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child failed:\n{stderr}");
    assert!(stderr.contains("still running"), "service log missing:\n{stderr}");
    let warnings = stderr.matches("telemetry: export problem").count();
    assert_eq!(warnings, 1, "expected one export warning:\n{stderr}");
    assert_eq!(out.stdout.windows(5).filter(|w| *w == b"probe").count(), 0, "wrote to stdout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child() {
    if std::env::var_os("CHILD").is_none() {
        return;
    }
    let guard = telemetry::init(telemetry::service!("probe")).expect("init must not fail");
    assert!(guard.meter_provider().is_some());
    for i in 0..50 {
        tracing::info_span!("work", i).in_scope(|| tracing::info!("still running"));
    }
    opentelemetry::global::meter("probe").u64_counter("probe.count").build().add(1, &[]);
    drop(guard);
}
