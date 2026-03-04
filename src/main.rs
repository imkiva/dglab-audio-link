mod app;
mod audio;
mod dglab;
mod domain;
mod pipeline;
mod signal;

use std::sync::Arc;

use anyhow::{Result, anyhow};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    init_logging();

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
        "DG-Lab Audio Link",
        native_options,
        Box::new(move |_cc| Ok(Box::new(app::gui::DgLinkGuiApp::new(runtime.clone())))),
    )
    .map_err(|err| anyhow!(err.to_string()))?;

    Ok(())
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
