use tracing::level_filters::LevelFilter;

pub fn init(verbosity: u8) {
    let level = match verbosity {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };
    // Log only selected fields; never dump configurations, raw TXT, or keys.
    tracing_subscriber::fmt()
        .json()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .init();
}
