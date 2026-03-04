use std::sync::Arc;

use eframe::egui;
use qrcodegen::{QrCode, QrCodeEcc};
use tokio::runtime::Runtime;

use crate::{
    app::state::AppState,
    dglab::{
        pairing,
        protocol::{
            StrengthControlMode, build_clear_message, build_pulse_message, build_strength_message,
        },
    },
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

            let app_status = if self.state.app_bound {
                format!(
                    "App status: bound (app_id={})",
                    self.state.app_id.as_deref().unwrap_or("?")
                )
            } else if self.state.app_connected {
                "App status: connected, waiting bind".to_owned()
            } else {
                "App status: not connected".to_owned()
            };
            ui.small(app_status);

            if let Some(info) = &self.state.last_server_info {
                ui.small(format!("Server info: {info}"));
            }
        });
    }

    fn draw_strength_range(&mut self, ui: &mut egui::Ui) {
        let slider_max = self.state.effective_global_strength_slider_max();
        self.state.strength_range.max = self.state.strength_range.max.min(slider_max);
        self.state.strength_range.min = self
            .state
            .strength_range
            .min
            .min(self.state.strength_range.max);

        ui.group(|ui| {
            ui.label("DGLab Strength Range (0-200)");
            ui.checkbox(
                &mut self.state.auto_limit_with_app_soft_limit,
                "Auto-limit sliders by App soft limit",
            );

            ui.add(
                egui::Slider::new(&mut self.state.strength_range.min, 0..=slider_max).text("Min"),
            );
            ui.add(
                egui::Slider::new(&mut self.state.strength_range.max, 0..=slider_max).text("Max"),
            );
            self.state.strength_range = self.state.strength_range.normalized();

            if let Some(report) = self.state.app_strength_report {
                ui.small(format!(
                    "App strength A:{} B:{} | soft A:{} B:{}",
                    report.a_strength, report.b_strength, report.a_soft_limit, report.b_soft_limit
                ));
            } else {
                ui.small("No app strength report yet. Send/receive once after bind.");
            }

            if slider_max < 200 {
                ui.small(format!(
                    "Current global max is limited by App soft limit: {slider_max}"
                ));
            }
        });
    }

    fn draw_protocol_debug_panel(&mut self, ui: &mut egui::Ui) {
        let debug_strength_max = self
            .state
            .effective_debug_strength_slider_max(self.state.debug_strength_channel);
        self.state.debug_strength_value = self.state.debug_strength_value.min(debug_strength_max);

        ui.group(|ui| {
            ui.label("Protocol Debug (Manual)");
            ui.small("Send raw control messages to App after bind. Fails will be shown explicitly.");

            ui.horizontal(|ui| {
                ui.label("Strength");
                egui::ComboBox::from_id_salt("debug_strength_channel")
                    .selected_text(self.state.debug_strength_channel.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.state.debug_strength_channel,
                            DglabChannel::A,
                            DglabChannel::A.label(),
                        );
                        ui.selectable_value(
                            &mut self.state.debug_strength_channel,
                            DglabChannel::B,
                            DglabChannel::B.label(),
                        );
                    });

                egui::ComboBox::from_id_salt("debug_strength_mode")
                    .selected_text(self.state.debug_strength_mode.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.state.debug_strength_mode,
                            StrengthControlMode::Decrease,
                            StrengthControlMode::Decrease.label(),
                        );
                        ui.selectable_value(
                            &mut self.state.debug_strength_mode,
                            StrengthControlMode::Increase,
                            StrengthControlMode::Increase.label(),
                        );
                        ui.selectable_value(
                            &mut self.state.debug_strength_mode,
                            StrengthControlMode::SetValue,
                            StrengthControlMode::SetValue.label(),
                        );
                    });

                ui.add(
                    egui::Slider::new(&mut self.state.debug_strength_value, 0..=debug_strength_max)
                        .text("Value"),
                );

                if ui.button("Send Strength").clicked() {
                    self.send_debug_strength_message();
                }
            });

            ui.horizontal(|ui| {
                ui.label("Clear");
                egui::ComboBox::from_id_salt("debug_clear_channel")
                    .selected_text(self.state.debug_clear_channel.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.state.debug_clear_channel,
                            DglabChannel::A,
                            DglabChannel::A.label(),
                        );
                        ui.selectable_value(
                            &mut self.state.debug_clear_channel,
                            DglabChannel::B,
                            DglabChannel::B.label(),
                        );
                    });

                if ui.button("Send Clear").clicked() {
                    let message = build_clear_message(self.state.debug_clear_channel);
                    self.send_manual_protocol_message(message, None);
                }
            });

            ui.horizontal(|ui| {
                ui.label("Pulse");
                egui::ComboBox::from_id_salt("debug_pulse_channel")
                    .selected_text(self.state.debug_pulse_channel.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.state.debug_pulse_channel,
                            DglabChannel::A,
                            DglabChannel::A.label(),
                        );
                        ui.selectable_value(
                            &mut self.state.debug_pulse_channel,
                            DglabChannel::B,
                            DglabChannel::B.label(),
                        );
                    });

                if ui.button("Load Sample").clicked() {
                    self.state.debug_pulse_values =
                        "0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A"
                            .to_owned();
                }

                if ui.button("Send Pulse").clicked() {
                    match build_pulse_message(
                        self.state.debug_pulse_channel,
                        &self.state.debug_pulse_values,
                    ) {
                        Ok(message) => self.send_manual_protocol_message(message, None),
                        Err(err) => self.state.set_error(err),
                    }
                }
            });

            ui.add(
                egui::TextEdit::multiline(&mut self.state.debug_pulse_values)
                    .hint_text("Pulse HEX list, e.g. 0A0A0A0A00000000 0A0A0A0A0A0A0A0A")
                    .desired_rows(3),
            );
            ui.small("Pulse HEX format: 16 hex chars each. Split by spaces, comma, semicolon or new line.");

            if let Some(last) = &self.state.last_protocol_action {
                ui.small(format!("Last action: {last}"));
            }
            if let Some(last_app) = &self.state.last_app_message {
                ui.small(format!("Last app msg: {last_app}"));
            }
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

    fn send_debug_strength_message(&mut self) {
        let channel = self.state.debug_strength_channel;
        let mode = self.state.debug_strength_mode;
        let requested = self.state.debug_strength_value;
        let mut effective = requested;
        let mut note = None;

        if self.state.auto_limit_with_app_soft_limit {
            match mode {
                StrengthControlMode::SetValue => {
                    if let Some(soft_limit) = self.state.app_soft_limit_for_channel(channel) {
                        if effective > soft_limit {
                            effective = soft_limit;
                            note = Some(format!(
                                "clamped to app soft limit ({soft_limit}) for channel {}",
                                channel.label()
                            ));
                        }
                    } else {
                        note = Some("app soft limit unknown; no clamp applied".to_owned());
                    }
                }
                StrengthControlMode::Increase => {
                    if let (Some(current), Some(soft_limit)) = (
                        self.state.app_current_strength_for_channel(channel),
                        self.state.app_soft_limit_for_channel(channel),
                    ) {
                        let max_delta = soft_limit.saturating_sub(current);
                        if max_delta == 0 {
                            self.state.set_error(format!(
                                "channel {} already at soft limit {soft_limit}; increase skipped",
                                channel.label()
                            ));
                            self.state
                                .set_protocol_action("send skipped by local guard".to_owned());
                            return;
                        }
                        if effective > max_delta {
                            effective = max_delta;
                            note = Some(format!(
                                "increase clamped to {max_delta} (current {current}, soft {soft_limit})"
                            ));
                        }
                    }
                }
                StrengthControlMode::Decrease => {
                    if let Some(current) = self.state.app_current_strength_for_channel(channel) {
                        if effective > current {
                            effective = current;
                            note = Some(format!("decrease clamped to current strength {current}"));
                        }
                    }
                }
            }
        }

        let message = build_strength_message(channel, mode, effective);
        self.send_manual_protocol_message(message, note);
    }

    fn send_manual_protocol_message(&mut self, message: String, note: Option<String>) {
        self.state.clear_error();
        let message_for_status = message.clone();
        match self.engine.send_app_message(message) {
            Ok(()) => {
                let mut action = format!("sent {message_for_status}");
                if let Some(note) = note {
                    action.push_str(" (");
                    action.push_str(&note);
                    action.push(')');
                }
                self.state.set_protocol_action(action);
            }
            Err(err) => {
                self.state.set_error(err.to_string());
                self.state
                    .set_protocol_action(format!("send failed: {message_for_status}"));
            }
        }
    }

    fn sync_engine_snapshot(&mut self) {
        let snapshot = self.engine.snapshot();
        self.state.app_connected = snapshot.app_connected;
        self.state.app_bound = snapshot.app_bound;
        self.state.app_id = snapshot.app_id;
        self.state.last_app_message = snapshot.last_app_message;
        self.state.last_server_info = snapshot.last_server_info;

        if let Some(report) = snapshot.latest_strength {
            self.state.app_strength_report = Some(report);
            if self.state.auto_limit_with_app_soft_limit {
                let global_max = self.state.effective_global_strength_slider_max();
                self.state.strength_range.max = self.state.strength_range.max.min(global_max);
                self.state.strength_range.min = self
                    .state
                    .strength_range
                    .min
                    .min(self.state.strength_range.max);
                let debug_max = self
                    .state
                    .effective_debug_strength_slider_max(self.state.debug_strength_channel);
                self.state.debug_strength_value = self.state.debug_strength_value.min(debug_max);
            }
        }
    }
}

impl eframe::App for DgLinkGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_engine_snapshot();
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
            self.draw_protocol_debug_panel(ui);
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
