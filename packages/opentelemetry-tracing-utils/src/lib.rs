//! Utilities for tracing and logging.
//!
//! Some fairly opinionated!

use anyhow::Result;
use std::str::FromStr;
use tracing_opentelemetry::OpenTelemetryLayer;

// tracing
use opentelemetry::{global, propagation::TextMapCompositePropagator, trace::TracerProvider};
use opentelemetry_sdk::{
    propagation::{BaggagePropagator, TraceContextPropagator},
    trace::SdkTracerProvider,
};
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

/// Set up an OTEL pipeline when the OTLP endpoint is set. Otherwise just set up tokio tracing
/// support.
pub fn set_up_logging() -> Result<LoggingSetupBuildResult> {
    LoggingSetupBuilder::new().build()
}

/// The env var that controls whether to use OTLP or not.
/// Accepts "otlp" or "none".
pub const OTEL_TRACES_EXPORTER: &str = "OTEL_TRACES_EXPORTER";
/// This will override OTEL_TRACES_EXPORTER if set.
pub const NO_OTLP: &str = "NO_OTLP";

#[derive(Debug)]
pub struct LoggingSetupBuilder {
    pub otlp_output_enabled: bool,
    pub pretty_logs: bool,
    pub use_test_writer: bool,
}
impl Default for LoggingSetupBuilder {
    fn default() -> Self {
        let no_otlp = match std::env::var(NO_OTLP).as_deref() {
            Ok("0") => false,
            Ok(_) => true,
            Err(_) => false,
        };

        let otel_traces_exporter = match std::env::var(OTEL_TRACES_EXPORTER).as_deref() {
            Ok("otlp") => OtelTracesExporterOption::Otlp,
            Ok("none") => OtelTracesExporterOption::None,
            _ => OtelTracesExporterOption::Otlp,
        };

        let otlp_enabled = no_otlp == false
            && match otel_traces_exporter {
                OtelTracesExporterOption::Otlp => true,
                OtelTracesExporterOption::None => false,
            };

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

enum OtelTracesExporterOption {
    Otlp,
    None,
}

impl LoggingSetupBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn build(&self) -> Result<LoggingSetupBuildResult> {
        let otlp_enabled = self.otlp_output_enabled;

        // First create 1 or more propagators
        let baggage_propagator = BaggagePropagator::new();
        let trace_context_propagator = TraceContextPropagator::new();

        // Then create a composite propagator
        let composite_propagator = TextMapCompositePropagator::new(vec![
            Box::new(baggage_propagator),
            Box::new(trace_context_propagator),
        ]);

        global::set_text_map_propagator(composite_propagator);

        // An exporter to be used when there is no OTLP endpoint
        let basic_no_otlp_tracer_provider = SdkTracerProvider::builder().build();

        // Install a new OpenTelemetry trace pipeline
        // OTLP over GRPC tracer exporter
        let otlp_tracer_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .build()?;
        // OTLP tracer setup, using the exporter from above
        let otlp_tracer: SdkTracerProvider = SdkTracerProvider::builder()
            .with_batch_exporter(otlp_tracer_exporter)
            .build();

        let tracer_provider = match otlp_enabled {
            true => otlp_tracer,
            // BUG: the non-otlp tracer isn't correctly setting context/linking ids
            false => basic_no_otlp_tracer_provider,
        };
        let tracer = tracer_provider.tracer(env!("CARGO_PKG_NAME"));

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

        // Set the global tracer provider using a clone of the tracer_provider.
        // Setting global tracer provider is required if other parts of the application
        // uses global::tracer() or global::tracer_with_version() to get a tracer.
        // Cloning simply creates a new reference to the same tracer provider. It is
        // important to hold on to the tracer_provider here, so as to invoke
        // shutdown on it when application ends.
        global::set_tracer_provider(tracer_provider.clone());

        Ok(LoggingSetupBuildResult { tracer_provider })
    }
}

/// Hang on to this to be able to call `shutdown()` on the providers.
pub struct LoggingSetupBuildResult {
    /// Hang on to this to be able to call `shutdown()` on the provider.
    pub tracer_provider: SdkTracerProvider,
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
    use opentelemetry::global;
    use opentelemetry_http::HeaderExtractor;
    use tower::{Layer, Service};
    use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
    use tower_http::trace::TraceLayer;
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
                propagator.inject_context(
                    &context,
                    &mut opentelemetry_http::HeaderInjector(request.headers_mut()),
                )
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

    /// Trace context propagation: associate a new span with the OTel trace of the given request,
    /// if any and valid.
    ///
    /// This uses the tower-http crate
    ///
    /// For propagation to work, RUST_LOG needs to include this crate, and also tower_http if you
    /// want access log events from there.
    pub fn make_tower_http_otel_trace_layer<BodyType>() -> TraceLayer<
        SharedClassifier<ServerErrorsAsFailures>,
        impl (Fn(&Request<BodyType>) -> tracing::Span) + Clone,
    > {
        tower_http::trace::TraceLayer::new_for_http().make_span_with(
            |request: &http::Request<BodyType>| {
                let context = get_otel_context_from_request(request);

                let span = tracing::debug_span!(
                        "request",
                        method = %request.method(),
                        uri = %request.uri(),
                        version = ?request.version(),
                        headers = ?request.headers());

                span.set_parent(context);

                span
            },
        )
    }

    /// Just get the context, don't set the parent context
    pub fn get_otel_context_from_request<BodyType>(
        request: &Request<BodyType>,
    ) -> opentelemetry::Context {
        // Return context, either from request or pre-existing if no or invalid data is received.
        let parent_context = global::get_text_map_propagator(|propagator| {
            let extracted = propagator.extract(&HeaderExtractor(request.headers()));
            trace!("extracted: {:#?}", &extracted);
            extracted
        });
        trace!("parent context (extraction): {:#?}", parent_context);

        parent_context
    }

    #[cfg(test)]
    mod tests {
        use opentelemetry::{baggage::BaggageExt, trace::TraceContextExt};

        use crate::tower_tracing::get_otel_context_from_request;

        /// Test whether propagation from standard headers is working
        #[tokio::test]
        async fn test_trace_context_extractor() {
            let _ = crate::set_up_logging().map_err(|err| dbg!(err));

            let request: http::Request<String> = http::Request::builder()
                .uri("/")
                .header(
                    "traceparent",
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                )
                .header("tracestate", "asdf=123456")
                .header(
                    "baggage",
                    "userId=alice,serverNode=DF%2028,isProduction=false",
                )
                .body("".to_string())
                .unwrap();

            let context = get_otel_context_from_request(&request);

            dbg!(&context);
            dbg!(&context.has_active_span());
            assert!(context.has_active_span());

            let baggage = context.baggage();
            assert_eq!(baggage.get("userId"), Some(&"alice".into()));
            assert_eq!(baggage.get("serverNode"), Some(&"DF 28".into()));
            assert_eq!(baggage.get("isProduction"), Some(&"false".into()));
        }
    }
}
