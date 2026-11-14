//! The JSON event format: one `serde_json` object per line, with the configured fields.

use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oxbloom_core::Clock;

use serde_json::{Map, Number, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields};
use tracing_subscriber::registry::LookupSpan;

use crate::settings::{JsonField, JsonOptions};

/// Writes each event as one JSON object on one line.
///
/// The keys are the configured names of the enabled fields. Event fields other than `message`
/// are added as extra keys with their JSON types; the built-in keys win a name clash. Pair it
/// with [`JsonFields`] so span fields keep their types too. The timestamp is read from the
/// clock, which is the system time unless a test supplies another clock.
#[derive(Clone)]
pub(crate) struct JsonFormat {
    options: JsonOptions,
    clock: Arc<dyn Clock>,
}

impl JsonFormat {
    pub(crate) fn new(options: JsonOptions, clock: Arc<dyn Clock>) -> Self {
        JsonFormat { options, clock }
    }
}

impl fmt::Debug for JsonFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonFormat")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl<S, N> FormatEvent<S, N> for JsonFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.normalized_metadata();
        let meta = meta.as_ref().unwrap_or_else(|| event.metadata());
        let mut visitor = MapVisitor::default();
        event.record(&mut visitor);
        let message = visitor.message.take();
        let mut map = visitor.map;
        let opts = &self.options;
        let mut put = |field: JsonField, value: Value| {
            if opts.is_on(field) {
                map.insert(opts.name(field).to_string(), value);
            }
        };
        put(
            JsonField::Timestamp,
            Value::String(rfc3339(self.clock.now())),
        );
        put(
            JsonField::Level,
            Value::String(meta.level().as_str().to_ascii_lowercase()),
        );
        put(JsonField::Target, Value::String(meta.target().to_string()));
        if let Some(message) = message {
            put(JsonField::Message, Value::String(message));
        }
        if opts.is_on(JsonField::Span)
            && let Some(span) = ctx.lookup_current()
        {
            let mut object = span
                .extensions()
                .get::<FormattedFields<N>>()
                .and_then(|f| serde_json::from_str::<Map<String, Value>>(&f.fields).ok())
                .unwrap_or_default();
            object.insert("name".into(), Value::String(span.name().to_string()));
            put(JsonField::Span, Value::Object(object));
        }
        let thread = std::thread::current();
        let thread = match thread.name() {
            Some(name) => name.to_string(),
            None => format!("{:?}", thread.id()),
        };
        put(JsonField::Thread, Value::String(thread));
        let line = serde_json::to_string(&Value::Object(map)).map_err(|_| fmt::Error)?;
        writeln!(writer, "{line}")
    }
}

/// Records span fields as a JSON object, so [`JsonFormat`] can keep their types.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct JsonFields;

impl<'writer> FormatFields<'writer> for JsonFields {
    fn format_fields<R: RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut visitor = MapVisitor::default();
        fields.record(&mut visitor);
        visitor.finish_into(&mut writer)
    }

    fn add_fields(
        &self,
        current: &'writer mut FormattedFields<Self>,
        fields: &tracing::span::Record<'_>,
    ) -> fmt::Result {
        let mut visitor = MapVisitor::default();
        if let Ok(old) = serde_json::from_str::<Map<String, Value>>(&current.fields) {
            visitor.map = old;
        }
        fields.record(&mut visitor);
        let mut text = String::new();
        visitor.finish_into(&mut Writer::new(&mut text))?;
        current.fields = text;
        Ok(())
    }
}

/// Collects fields as JSON values; `message` is kept apart.
#[derive(Default)]
struct MapVisitor {
    map: Map<String, Value>,
    message: Option<String>,
}

impl MapVisitor {
    fn put(&mut self, field: &Field, value: Value) {
        if field.name() == "message"
            && let Value::String(text) = value
        {
            self.message = Some(text);
        } else {
            self.map.insert(field.name().to_string(), value);
        }
    }

    /// Writes the collected fields (message included, for span fields) as one object.
    fn finish_into(mut self, writer: &mut Writer<'_>) -> fmt::Result {
        if let Some(message) = self.message.take() {
            self.map.insert("message".into(), Value::String(message));
        }
        let text = serde_json::to_string(&self.map).map_err(|_| fmt::Error)?;
        writer.write_str(&text)
    }
}

impl Visit for MapVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.put(field, Value::String(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, Value::String(value.to_string()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, Value::Number(value.into()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, Value::Bool(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        // NaN and infinities have no JSON form: keep them as text.
        let value = Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(value.to_string()));
        self.put(field, value);
    }
}

/// `time` as an RFC 3339 UTC timestamp with millisecond precision.
fn rfc3339(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(secs: u64, millis: u64) -> String {
        rfc3339(UNIX_EPOCH + Duration::from_millis(secs * 1000 + millis))
    }

    #[test]
    fn epoch_formats() {
        assert_eq!(at(0, 0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn known_instants_format() {
        assert_eq!(at(951_782_400, 0), "2000-02-29T00:00:00.000Z");
        assert_eq!(at(1_709_211_599, 999), "2024-02-29T12:59:59.999Z");
        assert_eq!(at(1_793_534_400, 7), "2026-11-01T12:00:00.007Z");
        assert_eq!(at(4_102_444_799, 0), "2099-12-31T23:59:59.000Z");
    }
}
