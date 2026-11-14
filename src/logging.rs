use tracing::level_filters::LevelFilter;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Clone, Copy)]
struct Mode {
    json: bool,
    level: LevelFilter,
}
impl Mode {
    fn new(verbosity: u8, unattended: bool) -> Self {
        Self {
            json: unattended,
            level: match verbosity {
                0 if !unattended => LevelFilter::WARN,
                0 => LevelFilter::INFO,
                1 => LevelFilter::DEBUG,
                _ => LevelFilter::TRACE,
            },
        }
    }
}

pub fn init(verbosity: u8, unattended: bool) {
    // SubscriberInitExt also installs the log-to-tracing bridge used by dependencies.
    subscriber(verbosity, unattended, std::io::stderr).init();
}

fn subscriber<W>(
    verbosity: u8,
    unattended: bool,
    writer: W,
) -> Box<dyn tracing::Subscriber + Send + Sync>
where
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    let mode = Mode::new(verbosity, unattended);
    // Never dump configurations, raw TXT, or keys. Interactive output uses
    // readable warnings/errors; unattended runtime retains journal JSON events.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(mode.level)
        .with_writer(writer);
    if mode.json {
        Box::new(subscriber.json().finish())
    } else {
        Box::new(subscriber.with_ansi(false).without_time().finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_verbose_and_unattended_policy() {
        for unattended in [false, true] {
            for (verbosity, expected) in [(1, LevelFilter::DEBUG), (2, LevelFilter::TRACE)] {
                let mode = Mode::new(verbosity, unattended);
                assert_eq!(mode.json, unattended);
                assert_eq!(mode.level, expected);
            }
        }
        let normal = Mode::new(0, false);
        assert!(!normal.json);
        assert_eq!(normal.level, LevelFilter::WARN);
        let unattended = Mode::new(0, true);
        assert!(unattended.json);
        assert_eq!(unattended.level, LevelFilter::INFO);
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn actual_output_keeps_errors_and_warnings_and_respects_verbosity() {
        for unattended in [false, true] {
            for verbosity in 0..=2 {
                let buffer = Buffer::default();
                let writer = buffer.clone();
                tracing::subscriber::with_default(
                    subscriber(verbosity, unattended, move || writer.clone()),
                    || {
                        tracing::trace!("trace-diagnostic");
                        tracing::debug!("debug-diagnostic");
                        tracing::info!("resource-cleanup");
                        tracing::warn!("important-warning");
                        tracing::error!("cleanup-failure");
                    },
                );
                let text = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
                assert!(text.contains("important-warning"));
                assert!(text.contains("cleanup-failure"));
                assert_eq!(
                    text.contains("resource-cleanup"),
                    unattended || verbosity > 0
                );
                assert_eq!(text.contains("debug-diagnostic"), verbosity > 0);
                assert_eq!(text.contains("trace-diagnostic"), verbosity > 1);
                assert_eq!(text.contains("\"fields\""), unattended);
                if unattended {
                    for line in text.lines() {
                        serde_json::from_str::<serde_json::Value>(line).unwrap();
                    }
                }
            }
        }
    }
}
