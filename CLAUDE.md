# telemetry

One shared setup for logs, spans and metrics in Pedro's Rust services, so
they all report the same way to the observability backend on imperium.
Every service calls one `init()`. That gives it the same resource attributes
(`service.name`, `host.name`), the same OTLP export, the same stderr output
and the same flush on exit. Those shared names are what let one Grafana
dashboard join a ghola ingest, the airshell connection it went over and the
nixcache request behind its build.

Status: design, no code yet (only `cargo new --lib`). Repo:
github.com/pplanel/telemetry (not created yet). The plan this belongs to,
"observability for every service", is in nix-darwin's CLAUDE.md (decided
2026-10-19). This crate is step 3 of it.

## Where the data goes

- **Backend, on imperium only:** VictoriaLogs, VictoriaMetrics and
  VictoriaTraces, each on 127.0.0.1, with Grafana in front (nix-darwin,
  `modules/darwin/observability.nix`, step 1). All three take OTLP directly.
- **Transport: OTLP over HTTP with protobuf, not gRPC.** VictoriaMetrics
  and VictoriaLogs take OTLP over HTTP only. HTTP also keeps tonic out of
  every service. (VictoriaTraces' gRPC listener is optional; check its flags
  before relying on it either way.)
- **The endpoint comes from the environment, never from code.** nix-darwin
  sets the standard variables on each launchd job (step 2):
  `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`,
  `OTEL_RESOURCE_ATTRIBUTES` (with `host.name`).
  - Open in nix-darwin: a collector per Mac, or a direct path to imperium
    (airshell forward or tailscale).
  - With a collector, one base endpoint is enough (`/v1/logs`, `/v1/traces`,
    `/v1/metrics`).
  - Straight to Victoria*, the paths differ per backend
    (`/insert/opentelemetry/v1/logs`, `/opentelemetry/v1/metrics`, …). Then
    the per-signal variables `OTEL_EXPORTER_OTLP_{LOGS,TRACES,METRICS}_ENDPOINT`
    are needed. The crate must honour both forms. The OpenTelemetry SDK's
    exporters already do; don't parse them by hand.

## API (proposed)

```rust
fn main() -> anyhow::Result<()> {
    // Keep the guard for the life of the process: dropping it flushes
    // every exporter. std::process::exit skips the drop, so avoid it.
    let _telemetry = telemetry::init(telemetry::service!("ghola"))?;
    // ...
}
```

- `service!("name")` expands to the name plus the caller's
  `CARGO_PKG_VERSION`, so `service.version` is the service's version, not
  this crate's.
- `init()` installs the global `tracing` subscriber:
  - `EnvFilter` from `RUST_LOG`, defaulting to `info`;
  - a human-readable stderr layer, without ANSI when stderr isn't a tty
    (launchd's `StandardErrorPath`);
  - the OTLP layers: spans through `tracing-opentelemetry`, events as OTLP
    logs through `opentelemetry-appender-tracing`.
- `init()` also returns, inside the guard, the global `MeterProvider` for
  metrics.
- **`layers()` for services that compose their own subscriber.** It returns
  the same layers without installing anything. airshell is the case: it has
  its own JSON file layer (`crates/airshell-net/src/logging.rs`) and stdout
  is its ssh data channel, so logs must never go there. The crate never
  writes to stdout.

## Rules

- **Telemetry never takes a service down.**
  - No endpoint set means export is off and stderr only.
  - imperium unreachable means batch exporters with bounded queues, dropping
    when full, with one warning on stderr, not one per batch.
  - `init()` fails only on a bug in the configuration, never because the
    network is down.
  - Buffering on disk while a Mac is away from imperium is the collector's
    job (if nix-darwin adopts it), not this crate's.
- **Same names everywhere.**
  - Resource: `service.name`, `service.version`, `host.name` (the short
    name: `arrakis`, `caladan`, `imperium`, from `OTEL_RESOURCE_ATTRIBUTES`,
    falling back to the hostname up to the first dot).
  - Attributes follow the OpenTelemetry semantic conventions where one
    exists (`http.request.method`, `url.path`, `error.type`, …).
  - Service-specific ones are prefixed with the service (`ghola.session_id`,
    `airshell.peer`, `airshell.close_reason`).
  - When two services name the same thing, the name goes in this crate as a
    `const`, so they can't drift.
- **The `opentelemetry*` crates move in lockstep.** `opentelemetry`,
  `opentelemetry_sdk`, `opentelemetry-otlp`, `opentelemetry-appender-tracing`
  and `tracing-opentelemetry` must be versions made for each other. Bump
  them together, check the pairing on crates.io, and never pin a version from
  memory. Consumers get them through this crate (re-exported), so they never
  list them themselves.
- **No secrets in telemetry.** No tokens, passwords or transcript text in
  attributes or log fields. ghola already masks secrets on ingest; spans
  carry ids and counts, not content.

## Consumers

One PR each, in its own repo, after this crate exists:

| Service | Today | With this crate |
|---|---|---|
| ghola (`pplanel/ghola`) | no `tracing`, ~14 `println!`/`eprintln!` | `init()`; spans per ingested file, per processed session, per Gemini batch |
| airshell (`pplanel/airshell`) | `tracing-subscriber` with stderr + JSON file | `layers()` added to its registry; spans per connection, with why it closed and which interface it used |
| nixcache (`pplanel/nixcache`, `proxy/`) | `tracing-subscriber` fmt + `EnvFilter` | `init()` in place of its `fmt().init()`; a span per request |

claude-mesh is off (`mesh.enable = false`) and is skipped.

## Building with nix

- The consumers build with `rustPlatform.buildRustPackage` and `cargoLock`.
  A git dependency on this crate needs its hash in
  `cargoLock.outputHashes`, updated whenever the pinned rev changes.
- If the repo is private, like airshell, fetching it needs the GitHub token
  that nix-darwin already templates into `/etc/nix/github-token.conf`.
  Making this repo public avoids that.
- No flake of its own is needed while it's only a library.

## Pedro and this repo

- Pedro is the only author of commits and PRs here: no Claude trailers.
- Merges are his.
