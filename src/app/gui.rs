use std::sync::Arc;

use eframe::egui;
use qrcodegen::{QrCode, QrCodeEcc};
use tokio::runtime::Runtime;

use crate::{
    app::state::AppState,
    dglab::pairing,
    domain::{
        BAND_COUNT,
        types::{BandRouting, DglabChannel},
    },
    pipeline::engine::PipelineEngine,
};

pub struct DgLinkGuiApp {
    state: AppState,
    engine: PipelineEngine,
    qr_texture: Option<egui::TextureHandle>,
    qr_error: Option<String>,
    last_qr_payload: String,
}

impl DgLinkGuiApp {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        let mut app = Self {
            state: AppState::default(),
            engine: PipelineEngine::new(runtime),
            qr_texture: None,
            qr_error: None,
            last_qr_payload: String::new(),
        };
        app.start_engine();
        app
    }

    fn draw_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Program WS URL:");
            ui.text_edit_singleline(&mut self.state.websocket_url);

            if ui.button("Use Local LAN IP").clicked() {
                self.state.clear_error();
                if !self.state.refresh_lan_ws_url() {
                    self.state.set_error(
                        "No LAN IPv4 detected. URL fell back to 127.0.0.1 (phone cannot connect).",
                    );
                }
                self.last_qr_payload.clear();
                self.restart_engine_if_running();
            }

            if ui.button("New Session UUID").clicked() {
                self.state.rotate_session_id();
                self.last_qr_payload.clear();
                self.restart_engine_if_running();
            }

            if ui.button("Copy QR Payload").clicked() {
                ui.ctx()
                    .copy_text(pairing::build_qr_payload(&self.state.websocket_url));
            }

            let button_label = if self.engine.is_running() {
                "Stop WS Server"
            } else {
                "Start WS Server"
            };

            if ui.button(button_label).clicked() {
                if self.engine.is_running() {
                    self.engine.stop();
                    self.state.running = false;
                } else {
                    self.start_engine();
                }
            }
        });

        if let Some(err) = &self.state.last_error {
            ui.colored_label(egui::Color32::from_rgb(200, 40, 40), err);
        }
    }

    fn draw_pairing_panel(&mut self, ui: &mut egui::Ui) {
        let qr_payload = pairing::build_qr_payload(&self.state.websocket_url);
        ui.group(|ui| {
            ui.label("DGLab 3.0 Pairing QR");
            ui.label("Scan this QR code in the mobile app to connect.");
            ui.code(qr_payload.as_str());

            if let Some(texture) = &self.qr_texture {
                ui.image((texture.id(), texture.size_vec2()));
            }

            if let Some(err) = &self.qr_error {
                ui.colored_label(egui::Color32::from_rgb(200, 40, 40), err);
            }

            if pairing::ws_url_uses_loopback(&self.state.websocket_url) {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 40, 40),
                    "Current host is loopback. Use 'Use Local LAN IP' before scanning.",
                );
            } else {
                ui.small("WS URL should be a LAN IP reachable from your phone.");
            }

            let server_text = if self.engine.is_running() {
                "WS server status: running"
            } else {
                "WS server status: stopped"
            };
            ui.small(server_text);
        });
    }

    fn draw_strength_range(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label("DGLab Strength Range (0-200)");
            ui.add(egui::Slider::new(&mut self.state.strength_range.min, 0..=200).text("Min"));
            ui.add(egui::Slider::new(&mut self.state.strength_range.max, 0..=200).text("Max"));
            self.state.strength_range = self.state.strength_range.normalized();
        });
    }

    fn draw_band_editor(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label("Band Routing (4 bands)");
            for index in 0..BAND_COUNT {
                let band_value = self.state.band_values[index];
                let routing = &mut self.state.band_routing[index];
                Self::draw_band_row(ui, index, routing, band_value);
                ui.separator();
            }
        });
    }

    fn draw_band_row(ui: &mut egui::Ui, index: usize, routing: &mut BandRouting, band_value: f32) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut routing.enabled, format!("Band {}", index + 1));
            ui.add(egui::Slider::new(&mut routing.threshold, 0.0..=1.0).text("Trigger"));
            routing.threshold = routing.threshold.clamp(0.0, 1.0);

            egui::ComboBox::from_id_salt(format!("band_channel_{index}"))
                .selected_text(routing.channel.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut routing.channel,
                        DglabChannel::A,
                        DglabChannel::A.label(),
                    );
                    ui.selectable_value(
                        &mut routing.channel,
                        DglabChannel::B,
                        DglabChannel::B.label(),
                    );
                });

            ui.add(
                egui::ProgressBar::new(band_value.clamp(0.0, 1.0))
                    .desired_width(140.0)
                    .text(format!("{band_value:.2}")),
            );
        });
    }

    fn refresh_qr_texture_if_needed(&mut self, ctx: &egui::Context) {
        let payload = pairing::build_qr_payload(&self.state.websocket_url);
        if payload == self.last_qr_payload {
            return;
        }

        match build_qr_image(&payload) {
            Ok(image) => {
                self.qr_texture = Some(ctx.load_texture(
                    "dglab_pairing_qr",
                    image,
                    egui::TextureOptions::NEAREST,
                ));
                self.qr_error = None;
            }
            Err(err) => {
                self.qr_texture = None;
                self.qr_error = Some(err);
            }
        }

        self.last_qr_payload = payload;
    }

    fn start_engine(&mut self) {
        self.state.clear_error();
        match self.engine.start(&self.state.websocket_url) {
            Ok(()) => {
                self.state.running = true;
            }
            Err(err) => {
                self.state.running = false;
                self.state.set_error(err.to_string());
            }
        }
    }

    fn restart_engine_if_running(&mut self) {
        if !self.engine.is_running() {
            return;
        }

        self.state.clear_error();
        match self.engine.restart(&self.state.websocket_url) {
            Ok(()) => {
                self.state.running = true;
            }
            Err(err) => {
                self.state.running = false;
                self.state
                    .set_error(format!("failed to restart ws server: {err}"));
            }
        }
    }
}

impl eframe::App for DgLinkGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.refresh_qr_texture_if_needed(ctx);

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.draw_top_bar(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("DG-Lab Audio Link");
            ui.label("Windows speaker output -> 4-band analysis -> DGLab A/B waveform output");
            ui.separator();
            self.draw_pairing_panel(ui);
            ui.separator();
            self.draw_strength_range(ui);
            ui.separator();
            self.draw_band_editor(ui);
        });

        if self.engine.is_running() && !self.state.running {
            self.state.running = true;
        } else if !self.engine.is_running() && self.state.running {
            self.state.running = false;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn build_qr_image(payload: &str) -> Result<egui::ColorImage, String> {
    const SCALE: usize = 5;
    const BORDER_MODULES: usize = 4;

    let text = payload.trim();
    if text.is_empty() {
        return Err("QR payload is empty.".to_owned());
    }

    let qr = QrCode::encode_text(text, QrCodeEcc::Medium)
        .map_err(|err| format!("QR encode failed: {err:?}"))?;

    let module_size =
        usize::try_from(qr.size()).map_err(|_| "Invalid QR matrix size.".to_owned())?;
    let image_size = (module_size + BORDER_MODULES * 2) * SCALE;
    let mut pixels = vec![egui::Color32::WHITE; image_size * image_size];

    for y in 0..image_size {
        for x in 0..image_size {
            let module_x = x / SCALE;
            let module_y = y / SCALE;

            let in_qr_x = module_x >= BORDER_MODULES && module_x < BORDER_MODULES + module_size;
            let in_qr_y = module_y >= BORDER_MODULES && module_y < BORDER_MODULES + module_size;
            let is_dark = if in_qr_x && in_qr_y {
                let qr_x = (module_x - BORDER_MODULES) as i32;
                let qr_y = (module_y - BORDER_MODULES) as i32;
                qr.get_module(qr_x, qr_y)
            } else {
                false
            };

            let color = if is_dark {
                egui::Color32::BLACK
            } else {
                egui::Color32::WHITE
            };
            pixels[y * image_size + x] = color;
        }
    }

    Ok(egui::ColorImage {
        size: [image_size, image_size],
        pixels,
    })
}
