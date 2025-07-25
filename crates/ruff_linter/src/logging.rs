use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::Result;
use colored::Colorize;
use fern;
use log::Level;
use ruff_python_parser::ParseError;
use rustc_hash::FxHashSet;

use ruff_source_file::{LineColumn, LineIndex, OneIndexed, SourceCode};

use crate::fs;
use crate::source_kind::SourceKind;
use ruff_notebook::Notebook;

pub static IDENTIFIERS: LazyLock<Mutex<Vec<&'static str>>> = LazyLock::new(Mutex::default);

/// Warn a user once, with uniqueness determined by the given ID.
#[macro_export]
macro_rules! warn_user_once_by_id {
    ($id:expr, $($arg:tt)*) => {
        use colored::Colorize;
        use log::warn;

        if let Ok(mut states) = $crate::logging::IDENTIFIERS.lock() {
            if !states.contains(&$id) {
                let message = format!("{}", format_args!($($arg)*));
                warn!("{}", message.bold());
                states.push($id);
            }
        }
    };
}

pub static MESSAGES: LazyLock<Mutex<FxHashSet<String>>> = LazyLock::new(Mutex::default);

/// Warn a user once, if warnings are enabled, with uniqueness determined by the content of the
/// message.
#[macro_export]
macro_rules! warn_user_once_by_message {
    ($($arg:tt)*) => {
        use colored::Colorize;
        use log::warn;

        if let Ok(mut states) = $crate::logging::MESSAGES.lock() {
            let message = format!("{}", format_args!($($arg)*));
            if !states.contains(&message) {
                warn!("{}", message.bold());
                states.insert(message);
            }
        }
    };
}

/// Warn a user once, with uniqueness determined by the calling location itself.
#[macro_export]
macro_rules! warn_user_once {
    ($($arg:tt)*) => {
        use colored::Colorize;
        use log::warn;

        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let message = format!("{}", format_args!($($arg)*));
            warn!("{}", message.bold());
        }
    };
}

#[macro_export]
macro_rules! warn_user {
    ($($arg:tt)*) => {{
        use colored::Colorize;
        use log::warn;

        let message = format!("{}", format_args!($($arg)*));
        warn!("{}", message.bold());
    }};
}

#[macro_export]
macro_rules! notify_user {
    ($($arg:tt)*) => {
        println!(
            "[{}] {}",
            jiff::Zoned::now()
                .strftime("%H:%M:%S %p")
                .to_string()
                .dimmed(),
            format_args!($($arg)*)
        )
    }
}

#[derive(Debug, Default, PartialOrd, Ord, PartialEq, Eq, Copy, Clone)]
pub enum LogLevel {
    /// No output ([`log::LevelFilter::Off`]).
    Silent,
    /// Only show lint violations, with no decorative output
    /// ([`log::LevelFilter::Off`]).
    Quiet,
    /// All user-facing output ([`log::LevelFilter::Info`]).
    #[default]
    Default,
    /// All user-facing output ([`log::LevelFilter::Debug`]).
    Verbose,
}

impl LogLevel {
    #[expect(clippy::trivially_copy_pass_by_ref)]
    const fn level_filter(&self) -> log::LevelFilter {
        match self {
            LogLevel::Default => log::LevelFilter::Info,
            LogLevel::Verbose => log::LevelFilter::Debug,
            LogLevel::Quiet => log::LevelFilter::Off,
            LogLevel::Silent => log::LevelFilter::Off,
        }
    }
}

pub fn set_up_logging(level: LogLevel) -> Result<()> {
    fern::Dispatch::new()
        .format(|out, message, record| match record.level() {
            Level::Error => {
                out.finish(format_args!(
                    "{}{} {}",
                    "error".red().bold(),
                    ":".bold(),
                    message
                ));
            }
            Level::Warn => {
                out.finish(format_args!(
                    "{}{} {}",
                    "warning".yellow().bold(),
                    ":".bold(),
                    message
                ));
            }
            Level::Info | Level::Debug | Level::Trace => {
                out.finish(format_args!(
                    "{}[{}][{}] {}",
                    jiff::Zoned::now().strftime("[%Y-%m-%d][%H:%M:%S]"),
                    record.target(),
                    record.level(),
                    message
                ));
            }
        })
        .level(level.level_filter())
        .level_for("globset", log::LevelFilter::Warn)
        .level_for("ty_python_semantic", log::LevelFilter::Warn)
        .level_for("salsa", log::LevelFilter::Warn)
        .chain(std::io::stderr())
        .apply()?;
    Ok(())
}

/// A wrapper around [`ParseError`] to translate byte offsets to user-facing
/// source code locations (typically, line and column numbers).
#[derive(Debug)]
pub struct DisplayParseError {
    error: ParseError,
    path: Option<PathBuf>,
    location: ErrorLocation,
}

impl DisplayParseError {
    /// Create a [`DisplayParseError`] from a [`ParseError`] and a [`SourceKind`].
    pub fn from_source_kind(
        error: ParseError,
        path: Option<PathBuf>,
        source_kind: &SourceKind,
    ) -> Self {
        Self::from_source_code(
            error,
            path,
            &SourceCode::new(
                source_kind.source_code(),
                &LineIndex::from_source_text(source_kind.source_code()),
            ),
            source_kind,
        )
    }

    /// Create a [`DisplayParseError`] from a [`ParseError`] and a [`SourceCode`].
    pub fn from_source_code(
        error: ParseError,
        path: Option<PathBuf>,
        source_code: &SourceCode,
        source_kind: &SourceKind,
    ) -> Self {
        // Translate the byte offset to a location in the originating source.
        let location =
            if let Some(jupyter_index) = source_kind.as_ipy_notebook().map(Notebook::index) {
                let source_location = source_code.line_column(error.location.start());

                ErrorLocation::Cell(
                    jupyter_index
                        .cell(source_location.line)
                        .unwrap_or(OneIndexed::MIN),
                    LineColumn {
                        line: jupyter_index
                            .cell_row(source_location.line)
                            .unwrap_or(OneIndexed::MIN),
                        column: source_location.column,
                    },
                )
            } else {
                ErrorLocation::File(source_code.line_column(error.location.start()))
            };

        Self {
            error,
            path,
            location,
        }
    }

    /// Return the path of the file in which the error occurred.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl std::error::Error for DisplayParseError {}

impl Display for DisplayParseError {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        if let Some(path) = self.path.as_ref() {
            write!(
                f,
                "{header} {path}{colon}",
                header = "Failed to parse".bold(),
                path = fs::relativize_path(path).bold(),
                colon = ":".cyan(),
            )?;
        } else {
            write!(f, "{header}", header = "Failed to parse at ".bold())?;
        }
        match &self.location {
            ErrorLocation::File(location) => {
                write!(
                    f,
                    "{row}{colon}{column}{colon} {inner}",
                    row = location.line,
                    column = location.column,
                    colon = ":".cyan(),
                    inner = self.error.error
                )
            }
            ErrorLocation::Cell(cell, location) => {
                write!(
                    f,
                    "{cell}{colon}{row}{colon}{column}{colon} {inner}",
                    cell = cell,
                    row = location.line,
                    column = location.column,
                    colon = ":".cyan(),
                    inner = self.error.error
                )
            }
        }
    }
}

#[derive(Debug)]
enum ErrorLocation {
    /// The error occurred in a Python file.
    File(LineColumn),
    /// The error occurred in a Jupyter cell.
    Cell(OneIndexed, LineColumn),
}

#[cfg(test)]
mod tests {
    use crate::logging::LogLevel;

    #[test]
    fn ordering() {
        assert!(LogLevel::Default > LogLevel::Silent);
        assert!(LogLevel::Default >= LogLevel::Default);
        assert!(LogLevel::Quiet > LogLevel::Silent);
        assert!(LogLevel::Verbose > LogLevel::Default);
        assert!(LogLevel::Verbose > LogLevel::Silent);
    }
}
