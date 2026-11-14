//! The rotating log file layer.

use std::fmt as std_fmt;
use std::fs;
use std::io;
use std::sync::Arc;

use oxbloom_config::ConfigError;
use oxbloom_core::Clock;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::{DefaultFields, Writer};
use tracing_subscriber::fmt::{self, FormatFields};
use tracing_subscriber::layer::Layer;

use crate::error::LoggingError;
use crate::json::{JsonFields, JsonFormat};
use crate::settings::{FileFormat, FileOptions, FileRotation, JsonOptions};
use crate::system::Filtered;

/// The layer that writes to `opts.dir`, and the guard whose drop flushes it. The directory is
/// created when missing. Files are never colored. The JSON format reads its timestamp from
/// `clock`.
pub(crate) fn file_layer(
    opts: &FileOptions,
    json: &JsonOptions,
    clock: Arc<dyn Clock>,
) -> Result<(Box<dyn Layer<Filtered> + Send + Sync>, WorkerGuard), LoggingError> {
    if opts.dir.as_os_str().is_empty() {
        return Err(ConfigError::invalid("logging.file.path", "must not be empty").into());
    }
    if opts.name.is_empty() {
        return Err(ConfigError::invalid("logging.file.name", "must not be empty").into());
    }
    let io_err = |source| LoggingError::Io {
        path: opts.dir.clone(),
        source,
    };
    fs::create_dir_all(&opts.dir).map_err(io_err)?;
    let rotation = match opts.rotation {
        FileRotation::Hourly => Rotation::HOURLY,
        FileRotation::Daily => Rotation::DAILY,
        FileRotation::Never => Rotation::NEVER,
    };
    let mut builder = Builder::new()
        .rotation(rotation)
        .filename_prefix(&opts.name);
    if let Some(max) = opts.max_history {
        builder = builder.max_log_files(max);
    }
    let appender = builder
        .build(&opts.dir)
        .map_err(|e| io_err(io::Error::other(e)))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let layer = fmt::layer().with_writer(writer).with_ansi(false);
    let boxed = match opts.format {
        FileFormat::Full => layer.fmt_fields(FileFields::default()).boxed(),
        FileFormat::Compact => layer.compact().fmt_fields(FileFields::default()).boxed(),
        FileFormat::Json => layer
            .event_format(JsonFormat::new(json.clone(), clock))
            .fmt_fields(JsonFields)
            .boxed(),
    };
    Ok((boxed, guard))
}

/// The span field formatter of the text file formats: [`DefaultFields`] under a type of its own.
///
/// A span's formatted fields are stored once per formatter type. Sharing `DefaultFields` with a
/// console layer would hand the file the console's (colored) text, and a recorded field would be
/// appended once by each layer.
#[derive(Debug, Default)]
pub(crate) struct FileFields(DefaultFields);

impl<'writer> FormatFields<'writer> for FileFields {
    fn format_fields<R: RecordFields>(
        &self,
        writer: Writer<'writer>,
        fields: R,
    ) -> std_fmt::Result {
        self.0.format_fields(writer, fields)
    }
}
