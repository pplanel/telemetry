//! With no endpoint set, the service must log exactly as it would without
//! this crate. An empty set of layers is not neutral: `Vec<Layer>` with
//! nothing in it tells `tracing` to enable nothing, which silences the
//! subscriber's other layers too.
//!
//! The subscriber is global, so each case runs in a child process: this test
//! re-executes itself with `CASE` set.

use std::process::Command;

fn run(case: &str) -> String {
    run_with(case, &[])
}

fn run_with(case: &str, env: &[(&str, &str)]) -> String {
    let out = Command::new(std::env::current_exe().unwrap())
        .envs(env.iter().copied())
        .args(["--exact", "child", "--nocapture", "--test-threads=1"])
        .env("CASE", case)
        .env_remove("RUST_LOG")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{case} failed:\n{stderr}");
    stderr
}

#[test]
fn init_still_logs_to_stderr() {
    let stderr = run("init");
    assert!(stderr.contains("hello from init"), "silenced:\n{stderr}");
}

#[test]
fn layers_composed_into_a_registry_do_not_silence_it() {
    let stderr = run("layers");
    assert!(stderr.contains("hello from layers"), "silenced:\n{stderr}");
}

#[test]
fn a_malformed_endpoint_does_not_stop_the_service() {
    let stderr = run_with("init", &[("OTEL_EXPORTER_OTLP_ENDPOINT", "not a url ::")]);
    assert!(
        stderr.contains("hello from init"),
        "service did not run:\n{stderr}"
    );
}

#[test]
fn child() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    match std::env::var("CASE").as_deref() {
        Ok("init") => {
            let _guard = telemetry::init(telemetry::service!("probe")).unwrap();
            tracing::info!("hello from init");
        }
        Ok("layers") => {
            // What airshell does: its own layer, plus ours.
            let (otlp, _guard) = telemetry::layers(telemetry::service!("probe")).unwrap();
            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .with(otlp)
                .init();
            tracing::info!("hello from layers");
        }
        _ => {}
    }
}
