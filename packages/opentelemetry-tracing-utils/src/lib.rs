//! Utilities for tracing and logging.
//!
//! Some fairly opinionated!

use anyhow::Result;
use std::str::FromStr;
use tracing_opentelemetry::OpenTelemetryLayer;

// tracing
use opentelemetry::{global, trace::TracerProvider as _};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::TracerProvider};
pub use opentelemetry_semantic_conventions as semcov;
use tonic::{metadata::MetadataKey, service::Interceptor};
use tracing::Span;
pub use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan, TestWriter},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Layer,
};

use self::trace_output_fmt::JsonWithTraceId;

pub mod trace_output_fmt;

pub use opentelemetry::global::shutdown_tracer_provider;

/// Set up an OTEL pipeline when the OTLP endpoint is set. Otherwise just set up tokio tracing
/// support.
pub fn set_up_logging() -> Result<()> {
    LoggingSetupBuilder::new().build()
}

#[derive(Debug)]
pub struct LoggingSetupBuilder {
    pub otlp_output_enabled: bool,
    pub pretty_logs: bool,
    pub use_test_writer: bool,
}
impl Default for LoggingSetupBuilder {
    fn default() -> Self {
        let otlp_enabled = std::env::var("NO_OTLP")
            .unwrap_or_else(|_| "0".to_owned())
            .as_str()
            == "0";

        // either use the otlp state or PRETTY_LOGS env var to decide log format
        let pretty_logs = std::env::var("PRETTY_LOGS")
            .map(|e| &e == "1")
            .unwrap_or_else(|_| !otlp_enabled);

        Self {
            otlp_output_enabled: otlp_enabled,
            pretty_logs,
            use_test_writer: false,
        }
    }
}

impl LoggingSetupBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn build(&self) -> Result<()> {
        let otlp_enabled = self.otlp_output_enabled;

        global::set_text_map_propagator(TraceContextPropagator::new());

        let provider = TracerProvider::builder()
            .with_simple_exporter(opentelemetry_stdout::SpanExporter::default())
            .build();
        let basic_no_otlp_tracer = provider.tracer(env!("CARGO_PKG_NAME"));

        // Install a new OpenTelemetry trace pipeline
        let otlp_tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            // trace config. Collects service.name etc.
            .with_trace_config(opentelemetry_sdk::trace::config())
            .with_exporter(opentelemetry_otlp::new_exporter().tonic())
            .install_batch(opentelemetry_sdk::runtime::TokioCurrentThread)?;

        let tracer = match otlp_enabled {
            true => otlp_tracer,
            // BUG: the non-otlp tracer isn't correctly setting context/linking ids
            false => basic_no_otlp_tracer,
        };

        // Create a tracing layer with the configured tracer
        let opentelemetry: OpenTelemetryLayer<_, _> = tracing_opentelemetry::layer()
            .with_error_fields_to_exceptions(true)
            .with_error_records_to_exceptions(true)
            .with_tracer(tracer);

        let use_test_writer = self.use_test_writer;
        let pretty_logs = self.pretty_logs;

        #[derive(Debug)]
        enum MaybeTestWriterLayer<N, E> {
            WithTestWriter(fmt::Layer<tracing_subscriber::Registry, N, E, TestWriter>),
            NoTestWriter(fmt::Layer<tracing_subscriber::Registry>),
        }

        let base_layer = fmt::Layer::default();
        let base_layer: MaybeTestWriterLayer<_, _> = match use_test_writer {
            false => MaybeTestWriterLayer::NoTestWriter(base_layer),
            true => MaybeTestWriterLayer::WithTestWriter(base_layer.with_test_writer()),
        };

        // Include an option for when there is no otlp endpoint available. In this case, pretty print
        // events, as the data doesn't need to be nicely formatted json for analysis.
        let format_layers = match pretty_logs {
            // json fmt layer
            false => match base_layer {
                MaybeTestWriterLayer::NoTestWriter(layer) => {
                    layer.json().event_format(JsonWithTraceId).boxed()
                }
                MaybeTestWriterLayer::WithTestWriter(layer) => {
                    layer.json().event_format(JsonWithTraceId).boxed()
                }
            },
            // pretty fmt layer
            true => match base_layer {
                MaybeTestWriterLayer::NoTestWriter(layer) => {
                    layer.pretty().with_span_events(FmtSpan::NONE).boxed()
                }
                MaybeTestWriterLayer::WithTestWriter(layer) => {
                    layer.pretty().with_span_events(FmtSpan::NONE).boxed()
                }
            },
        };

        let layers = opentelemetry.and_then(format_layers);

        let tracing_registry = tracing_subscriber::registry()
            // Add a filter to the layers so that they only observe the spans that I want
            .with(layers.with_filter(
                // Parse env filter from RUST_LOG, setting a default directive if that fails.
                // Syntax for directives is here: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives
                EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    // e.g. "RUST_LOG=hello_rust_backend,warn" would do everything from hello_rust_backend, and only "warn" level or higher from elsewhere
                    EnvFilter::try_new("info")
                        .expect("hard-coded default directive should be valid")
                }),
            ));

        #[cfg(feature = "tokio-console")]
        let tracing_registry = tracing_registry.with(console_subscriber::spawn());

        tracing_registry.try_init()?;

        Ok(())
    }
}

/// This interceptor adds tokio tracing opentelemetry headers to grpc requests.
/// Allows stitching together distributed traces!
#[derive(Clone)]
pub struct GrpcInterceptor;
impl Interceptor for GrpcInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        // get otel context from current tokio tracing span
        let context = Span::current().context();

        opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&context, &mut MetadataInjector(req.metadata_mut()));
        });

        Ok(req)
    }
}

pub struct MetadataInjector<'a>(&'a mut tonic::metadata::MetadataMap);
impl<'a> opentelemetry::propagation::Injector for MetadataInjector<'a> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(key) = MetadataKey::from_str(key) {
            if let Ok(val) = value.parse() {
                self.0.insert(key, val);
            }
        }
    }
}

/// A tonic channel intercepted to provide distributed tracing context propagation.
pub type InterceptedGrpcService =
    tonic::codegen::InterceptedService<tonic::transport::Channel, GrpcInterceptor>;

#[cfg(feature = "tower")]
pub use tower_tracing::*;

#[cfg(feature = "tower")]
pub mod tower_tracing {
    use std::task::{Context, Poll};

    use http::Request;
    use opentelemetry::{
        global,
        propagation::{Extractor, Injector},
    };
    use tower::{Layer, Service};
    use tracing::trace;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    pub struct TracingLayer;

    impl<S> Layer<S> for TracingLayer {
        type Service = TracingService<S>;

        fn layer(&self, service: S) -> Self::Service {
            TracingService { service }
        }
    }

    /// A middleware that sorts tracing propagation to a client
    #[derive(Clone, Debug)]
    pub struct TracingService<S> {
        service: S,
    }

    impl<S, BodyType> Service<http::Request<BodyType>> for TracingService<S>
    where
        S: Service<http::Request<BodyType>>,
        BodyType: std::fmt::Debug,
    {
        type Response = S::Response;
        type Error = S::Error;
        type Future = S::Future;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.service.poll_ready(cx)
        }

        fn call(&mut self, mut request: Request<BodyType>) -> Self::Future {
            let old_headers = request.headers().clone();

            let context = tracing::Span::current().context();

            global::get_text_map_propagator(|propagator| {
                propagator.inject_context(&context, &mut HeaderInjector(request.headers_mut()))
            });

            trace!(
                "
--------------------------------------------------------------
old headers:
{:#?}
new headers:
{:#?}
-----------------------------------------------",
                old_headers,
                request.headers()
            );

            self.service.call(request)
        }
    }

    /// Trace context propagation: associate the current span with the OTel trace of the given request,
    /// if any and valid.
    pub fn extract_trace_context<BodyType>(request: Request<BodyType>) -> Request<BodyType>
    where
        BodyType: std::fmt::Debug,
    {
        // Current context, if no or invalid data is received.
        let parent_context = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        trace!("parent context (extraction): {:#?}", parent_context);
        tracing::Span::current().set_parent(parent_context);

        request
    }

    // NOTE: HeaderInjector and HeaderExtractor are here temporarily due to http v1 incompatibility
    struct HeaderInjector<'a>(pub &'a mut http::HeaderMap);

    impl<'a> Injector for HeaderInjector<'a> {
        /// Set a key and value in the HeaderMap.  Does nothing if the key or value are not valid inputs.
        fn set(&mut self, key: &str, value: String) {
            println!("In Header Injector set function!!");
            trace!("setting key: {}, to value: {}", key, value);
            trace!("old self.0: {:?}", self.0);
            if let Ok(name) = http::header::HeaderName::from_bytes(key.as_bytes()) {
                if let Ok(val) = http::header::HeaderValue::from_str(&value) {
                    self.0.insert(name, val);
                }
            }
            trace!("new self.0: {:?}", self.0);
        }
    }

    struct HeaderExtractor<'a>(pub &'a http::HeaderMap);

    impl<'a> Extractor for HeaderExtractor<'a> {
        /// Get a value for a key from the HeaderMap.  If the value is not valid ASCII, returns None.
        fn get(&self, key: &str) -> Option<&str> {
            self.0.get(key).and_then(|value| value.to_str().ok())
        }

        /// Collect all the keys from the HeaderMap.
        fn keys(&self) -> Vec<&str> {
            self.0
                .keys()
                .map(|value| value.as_str())
                .collect::<Vec<_>>()
        }
    }
}
