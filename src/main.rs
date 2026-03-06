#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod audio;
mod pipeline;
mod types;

use std::sync::Arc;

use anyhow::{Result, anyhow};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

fn main() -> Result<()> {
    let log_buffer = app::logs::GuiLogBuffer::new();
    let (log_reload_handle, initial_log_level) = init_logging(log_buffer.clone());
    let language = app::i18n::detect_system_language();

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([800.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        language.app_title(),
        native_options,
        Box::new(move |cc| {
            app::gui::install_cjk_font(&cc.egui_ctx);
            Ok(Box::new(app::gui::DgLinkGuiApp::new(
                runtime.clone(),
                language,
                log_buffer.clone(),
                log_reload_handle.clone(),
                initial_log_level,
            )))
        }),
    )
    .map_err(|err| anyhow!(err.to_string()))?;

    Ok(())
}

fn init_logging(
    log_buffer: app::logs::GuiLogBuffer,
) -> (app::logs::GuiLogReloadHandle, app::logs::GuiLogLevel) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let initial_log_level = app::logs::GuiLogLevel::from_filter_text(&filter.to_string());
    let (filter_layer, reload_handle) = tracing_subscriber::reload::Layer::new(filter);

    let _ = tracing_subscriber::registry()
        .with(filter_layer)
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(move || app::logs::GuiLogWriter::new(log_buffer.clone())),
        )
        .try_init();

    (reload_handle, initial_log_level)
}
