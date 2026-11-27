//! Logging with redaction (R16.2, design §15).
//!
//! Every event goes through [`RedactingFields`], which prints the message and
//! only the field names on [`ALLOWED_FIELDS`]. Any other field is replaced by
//! `[redacted]`, so a careless `tracing::info!(clipboard = %text)` can't leak
//! content. Messages themselves are static strings written by developers;
//! never format user content into them.

use std::fmt;

use tracing::field::{Field, Visit};
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::FormatFields;
use tracing_subscriber::fmt::format::Writer;

/// Field names that may appear in logs with their values.
pub const ALLOWED_FIELDS: &[&str] = &[
    "message",
    "peer",
    "addr",
    "cap",
    "mode",
    "error",
    "port",
    "reason",
    "rtt_ms",
    "count",
    "version",
    "code",
    "via",
    "path",
    "created",
    // Counters and timings of the screen viewer: numbers only.
    "received",
    "dropped",
    "decoded",
    "decode_avg_ms",
    "decode_max_ms",
    "longest_gap_ms",
    "secs",
];

/// Field formatter that redacts every field not on the allow-list.
#[derive(Debug, Default, Clone, Copy)]
pub struct RedactingFields;

struct Visitor<'a, 'w> {
    writer: &'a mut Writer<'w>,
    first: bool,
    result: fmt::Result,
}

impl Visitor<'_, '_> {
    fn write(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.result.is_err() {
            return;
        }
        let sep = if self.first { "" } else { " " };
        self.first = false;
        let name = field.name();
        self.result = if name == "message" {
            write!(self.writer, "{sep}{value:?}")
        } else if ALLOWED_FIELDS.contains(&name) {
            write!(self.writer, "{sep}{name}={value:?}")
        } else {
            write!(self.writer, "{sep}{name}=[redacted]")
        };
    }
}

impl Visit for Visitor<'_, '_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" || !ALLOWED_FIELDS.contains(&field.name()) {
            self.write(field, &format_args!("{value}"));
        } else {
            self.write(field, &value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.write(field, value);
    }
}

impl<'w> FormatFields<'w> for RedactingFields {
    fn format_fields<R: RecordFields>(&self, mut writer: Writer<'w>, fields: R) -> fmt::Result {
        let mut v = Visitor {
            writer: &mut writer,
            first: true,
            result: Ok(()),
        };
        fields.record(&mut v);
        v.result
    }
}

/// Install the global subscriber. `filter` uses `RUST_LOG` syntax; the
/// `URDEVICE_LOG` environment variable overrides it. Safe to call twice.
pub fn init(filter: &str) {
    use tracing_subscriber::EnvFilter;
    let filter = std::env::var("URDEVICE_LOG").unwrap_or_else(|_| filter.to_owned());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .fmt_fields(RedactingFields)
        .with_target(false)
        .with_ansi(false)
        .try_init();
}

/// Like [`init`], but writes through `writer` (Android sends lines to logcat).
pub fn init_with_writer<W>(filter: &str, writer: W)
where
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .fmt_fields(RedactingFields)
        .with_target(false)
        .with_ansi(false)
        .without_time()
        .with_writer(writer)
        .try_init();
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn unknown_fields_are_redacted() {
        let buf = Buf::default();
        let writer = buf.clone();
        let sub = tracing_subscriber::fmt()
            .fmt_fields(RedactingFields)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(sub, || {
            tracing::info!(peer = "abcd", clipboard = "hunter2", token = ?[1u8, 2], "sent");
        });
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(out.contains("sent"));
        assert!(out.contains("peer=\"abcd\""));
        assert!(out.contains("clipboard=[redacted]"));
        assert!(out.contains("token=[redacted]"));
        assert!(!out.contains("hunter2"));
    }
}
