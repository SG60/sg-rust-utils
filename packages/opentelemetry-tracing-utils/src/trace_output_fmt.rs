use opentelemetry::trace::TraceContextExt;
use serde::ser::{SerializeMap, Serializer as _};
use std::io;
use tracing::{Event, Subscriber};
use tracing_serde::AsSerde;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

pub struct WriteAdaptor<'a> {
    fmt_write: &'a mut dyn std::fmt::Write,
}

impl<'a> WriteAdaptor<'a> {
    pub fn new(fmt_write: &'a mut dyn std::fmt::Write) -> Self {
        Self { fmt_write }
    }
}

impl<'a> io::Write for WriteAdaptor<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s =
            std::str::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        self.fmt_write
            .write_str(s)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(s.as_bytes().len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Derived from https://github.com/tokio-rs/tracing/issues/1531#issuecomment-1136971089 combined
/// with default Json formatter
pub struct JsonWithTraceId;

pub struct TraceInfo {
    pub trace_id: String,
    pub span_id: String,
}

pub fn lookup_trace_info<S>(
    span_ref: &tracing_subscriber::registry::SpanRef<S>,
) -> Option<TraceInfo>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    span_ref
        .extensions()
        .get::<tracing_opentelemetry::OtelData>()
        .map(|o| {
            TraceInfo {
                // commented out line was from the original, conversation here:
                // https://github.com/tokio-rs/tracing/issues/1531#issuecomment-1137296115
                // trace_id: o.parent_cx.span().span_context().trace_id().to_string(),
                trace_id: o
                    .builder
                    .trace_id
                    .unwrap_or(o.parent_cx.span().span_context().trace_id())
                    .to_string(),
                span_id: o
                    .builder
                    .span_id
                    .unwrap_or(opentelemetry::trace::SpanId::INVALID)
                    .to_string(),
            }
        })
}

impl<S, N> FormatEvent<S, N> for JsonWithTraceId
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let meta = event.metadata();

        let mut visit = || {
            let mut serializer = serde_json::Serializer::new(WriteAdaptor::new(&mut writer));

            let mut serializer = serializer.serialize_map(None)?;
            serializer.serialize_entry("level", &meta.level().as_serde())?;

            let _format_field_marker: std::marker::PhantomData<N> = std::marker::PhantomData;

            use tracing_serde::fields::AsMap;
            serializer.serialize_entry("fields", &event.field_map())?;

            serializer.serialize_entry("target", meta.target())?;

            if let Some(ref span_ref) = ctx.lookup_current() {
                if let Some(trace_info) = lookup_trace_info(span_ref) {
                    serializer.serialize_entry("span_id", &trace_info.span_id)?;
                    serializer.serialize_entry("trace_id", &trace_info.trace_id)?;
                }
            }

            serializer.end()
        };

        visit().map_err(|_| std::fmt::Error)?;
        writeln!(writer)
    }
}
