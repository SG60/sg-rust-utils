# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- next-header -->

## [Unreleased] - ReleaseDate

## [0.7.0] - 2025-11-21

### Added
- Support for the `OTEL_TRACES_EXPORTER` env var to control whether to use OTLP or not.

## [0.6.0] - 2025-04-21

### Changed

- Now using newer versions of the Rust OTEL ecosystem crates

### Deprecated

- No longer possible to use the global `shutdown_tracer_provider()` function. Instead, you should hold on to the `LoggingSetupBuildResult` struct, returned by `set_up_logging()`, and call shutdown on the actual provider.

## [0.5.1] - 2024-11-05

### Changed

- Widened dependency requirements.


## [0.5.0] - 2024-11-01

### Added

- make_tower_http_otel_trace_layer function, to create a tower layer that will propagate OTEL traces and also log requests.

<!-- next-url -->
[Unreleased]: https://github.com/sg60/sg-rust-utils/compare/opentelemetry-tracing-utils-v0.7.0...HEAD
[0.7.0]: https://github.com/sg60/sg-rust-utils/compare/opentelemetry-tracing-utils-v0.6.0...opentelemetry-tracing-utils-v0.7.0
[0.6.0]: https://github.com/sg60/sg-rust-utils/compare/opentelemetry-tracing-utils-v0.5.1...opentelemetry-tracing-utils-v0.6.0
[0.5.1]: https://github.com/sg60/sg-rust-utils/compare/opentelemetry-tracing-utils-v0.5.0...opentelemetry-tracing-utils-v0.5.1
[0.5.0]: https://github.com/sg60/sg-rust-utils/compare/opentelemetry-tracing-utils-v0.4.2...opentelemetry-tracing-utils-v0.5.0
