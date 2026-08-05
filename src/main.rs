//! AIChat Binary Entry Point

fn main() -> gtk4::glib::ExitCode {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting AIChat application...");
    aichat::run_app()
}
