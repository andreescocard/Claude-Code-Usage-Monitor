mod app;
mod config;
mod diagnose;
mod models;
mod poller;
mod strings;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt::init();
    tracing::info!("starting cosmic-applet-claude-usage");

    let settings = cosmic::app::Settings::default()
        .no_main_window(true)
        .exit_on_close(false);

    cosmic::app::run::<app::ClaudeUsageApplet>(settings, ())
}
