use std::{collections::VecDeque, sync::Arc, time::Instant};

use eframe::egui;
use egui_plot::{Legend, Line, Plot, PlotPoints};
use qrcodegen::{QrCode, QrCodeEcc};
use tokio::runtime::Runtime;

use crate::{
    app::{
        i18n::{UiLanguage, tr},
        settings::{self, PersistedSettings},
        state::AppState,
    },
    audio::{
        analyzer::BAND_RANGES_HZ,
        capture::{default_output_device_name, list_output_device_names},
    },
    dglab::{
        pairing,
        protocol::{
            StrengthControlMode, build_clear_message, build_pulse_message, build_strength_message,
        },
    },
    domain::{
        BAND_COUNT,
        types::{AutoPulseMode, BandRouting, DglabChannel},
    },
    pipeline::engine::{PipelineEngine, PipelineSettings},
};

pub struct DgLinkGuiApp {
    state: AppState,
    engine: PipelineEngine,
    qr_texture: Option<egui::TextureHandle>,
    qr_error: Option<String>,
    last_qr_payload: String,
    prev_app_bound: bool,
    last_title_language: UiLanguage,
    last_persisted_settings: PersistedSettings,
    strength_history_started_at: Instant,
    strength_history: VecDeque<(f64, u16, u16)>,
}

const STRENGTH_HISTORY_MAX_POINTS: usize = 300;
const STRENGTH_HISTORY_WINDOW_SECONDS: f64 = 20.0;
const STRENGTH_HISTORY_SAMPLE_INTERVAL_SECONDS: f64 = 0.25;
const REPAINT_ACTIVE_MS: u64 = 120;
const REPAINT_RUNNING_MS: u64 = 250;
const REPAINT_IDLE_MS: u64 = 600;

pub fn install_cjk_font(ctx: &egui::Context) {
    #[cfg(target_os = "windows")]
    {
        const CANDIDATE_FONT_PATHS: [&str; 7] = [
            r"C:\Windows\Fonts\msyh.ttc",
            r"C:\Windows\Fonts\msyhl.ttc",
            r"C:\Windows\Fonts\msyhbd.ttc",
            r"C:\Windows\Fonts\simhei.ttf",
            r"C:\Windows\Fonts\Deng.ttf",
            r"C:\Windows\Fonts\Dengl.ttf",
            r"C:\Windows\Fonts\simsunb.ttf",
        ];

        for path in CANDIDATE_FONT_PATHS {
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };

            let mut fonts = egui::FontDefinitions::default();
            let cjk_font_name = "system_cjk".to_owned();
            fonts.font_data.insert(
                cjk_font_name.clone(),
                egui::FontData::from_owned(bytes).into(),
            );
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                family.insert(0, cjk_font_name.clone());
            }
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                family.push(cjk_font_name.clone());
            }
            ctx.set_fonts(fonts);
            tracing::info!("loaded CJK font for GUI: {path}");
            return;
        }

        tracing::warn!("no Windows CJK font file found, Chinese glyphs may not render correctly");
    }

    #[cfg(not(target_os = "windows"))]
    let _ = ctx;
}

impl DgLinkGuiApp {
    pub fn new(runtime: Arc<Runtime>, language: UiLanguage) -> Self {
        let mut state = AppState::default();
        state.language = language;
        match settings::load_settings() {
            Ok(Some(saved)) => saved.apply_to_state(&mut state),
            Ok(None) => {}
            Err(err) => tracing::warn!("failed to load persisted GUI settings: {err}"),
        }
        let initial_language = state.language;
        let last_persisted_settings = PersistedSettings::from_state(&state);

        let mut app = Self {
            state,
            engine: PipelineEngine::new(runtime),
            qr_texture: None,
            qr_error: None,
            last_qr_payload: String::new(),
            prev_app_bound: false,
            last_title_language: initial_language,
            last_persisted_settings,
            strength_history_started_at: Instant::now(),
            strength_history: VecDeque::new(),
        };
        app.refresh_output_device_list();
        app.persist_settings_if_changed();
        app.start_engine();
        app
    }

    fn tr(&self, en: &'static str, zh_cn: &'static str) -> &'static str {
        tr(self.state.language, en, zh_cn)
    }

    fn draw_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(self.tr("Program WS URL:", "程序 WS 地址："));
            ui.text_edit_singleline(&mut self.state.websocket_url);

            ui.label(self.tr("Language:", "语言："));
            egui::ComboBox::from_id_salt("ui_language")
                .selected_text(self.state.language.label())
                .show_ui(ui, |ui| {
                    for language in UiLanguage::all() {
                        ui.selectable_value(&mut self.state.language, language, language.label());
                    }
                });
        });

        ui.horizontal_wrapped(|ui| {
            if ui
                .button(self.tr("Use Local LAN IP", "使用本机局域网 IP"))
                .clicked()
            {
                self.state.clear_error();
                if !self.state.refresh_lan_ws_url() {
                    self.state.set_error(self.tr(
                        "No LAN IPv4 detected. URL fell back to 127.0.0.1 (phone cannot connect).",
                        "未检测到局域网 IPv4，地址已回退到 127.0.0.1（手机无法连接）。",
                    ));
                }
                self.last_qr_payload.clear();
                self.restart_engine_if_running();
            }

            if ui
                .button(self.tr("New Session UUID", "新建会话 UUID"))
                .clicked()
            {
                self.state.rotate_session_id();
                self.last_qr_payload.clear();
                self.restart_engine_if_running();
            }

            if ui
                .button(self.tr("Copy QR Payload", "复制二维码内容"))
                .clicked()
            {
                ui.ctx()
                    .copy_text(pairing::build_qr_payload(&self.state.websocket_url));
            }

            let button_label = if self.engine.is_running() {
                self.tr("Stop WS Server", "停止 WS 服务")
            } else {
                self.tr("Start WS Server", "启动 WS 服务")
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
        ui.label(self.tr(
            "Scan this QR code in the mobile app to connect.",
            "请在手机 App 中扫码连接。",
        ));
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
                self.tr(
                    "Current host is loopback. Use 'Use Local LAN IP' before scanning.",
                    "当前主机地址是回环地址。扫码前请先点“使用本机局域网 IP”。",
                ),
            );
        } else {
            ui.small(self.tr(
                "WS URL should be a LAN IP reachable from your phone.",
                "WS 地址应为手机可访问的局域网 IP。",
            ));
        }

        let server_text = if self.engine.is_running() {
            self.tr("WS server status: running", "WS 服务状态：运行中")
        } else {
            self.tr("WS server status: stopped", "WS 服务状态：已停止")
        };
        ui.small(server_text);

        let app_status = if self.state.app_bound {
            format!(
                "{} (app_id={})",
                self.tr("App status: bound", "App 状态：已绑定"),
                self.state.app_id.as_deref().unwrap_or("?")
            )
        } else if self.state.app_connected {
            self.tr(
                "App status: connected, waiting bind",
                "App 状态：已连接，等待绑定",
            )
            .to_owned()
        } else {
            self.tr("App status: not connected", "App 状态：未连接")
                .to_owned()
        };
        ui.small(app_status);

        if let Some(info) = &self.state.last_server_info {
            ui.small(format!("{}: {info}", self.tr("Server info", "服务信息")));
        }

        let audio_status = if self.state.audio_capture_running {
            format!(
                "{} ({})",
                self.tr("Audio capture: running", "音频采集：运行中"),
                self.state
                    .audio_input_device
                    .as_deref()
                    .unwrap_or(self.tr("<unknown input device>", "<未知输入设备>"))
            )
        } else {
            self.tr(
                "Audio capture: stopped/unavailable",
                "音频采集：已停止/不可用",
            )
            .to_owned()
        };
        ui.small(audio_status);
    }

    fn draw_strength_range(&mut self, ui: &mut egui::Ui) {
        let title = self.tr(
            "DGLab Strength Range (A/B, 0-200)",
            "DGLab 强度范围（A/B，0-200）",
        );
        let auto_limit_label = self.tr(
            "Auto-limit sliders by App soft limit",
            "按 App 软上限自动限制滑块",
        );
        let channel_a_label = self.tr("Channel A", "A 通道");
        let channel_b_label = self.tr("Channel B", "B 通道");
        let min_label = self.tr("Min", "最小");
        let max_label = self.tr("Max", "最大");
        let smooth_strength_label = self.tr("Smooth strength", "平滑强度");
        let smooth_factor_label = self.tr("Smoothing factor", "平滑系数");
        let smooth_factor_hint = self.tr(
            "0 = no smoothing, 1 = strongest smoothing (slower but still reaches target)",
            "0 = 不平滑，1 = 最强平滑（更慢但仍会到达目标）",
        );
        let app_strength_label = self.tr("App strength", "App 强度");
        let no_report_label = self.tr(
            "No app strength report yet. Send/receive once after bind.",
            "暂未收到 App 强度上报。绑定后收发一次即可同步。",
        );
        let soft_limit_note_label = self.tr(
            "Current max limited by App soft limit",
            "当前最大值受 App 软上限限制",
        );

        let slider_max_a = self
            .state
            .effective_strength_slider_max_for_channel(DglabChannel::A);
        let slider_max_b = self
            .state
            .effective_strength_slider_max_for_channel(DglabChannel::B);

        self.state.strength_range_a.max = self.state.strength_range_a.max.min(slider_max_a);
        self.state.strength_range_a.min = self
            .state
            .strength_range_a
            .min
            .min(self.state.strength_range_a.max);
        self.state.strength_range_b.max = self.state.strength_range_b.max.min(slider_max_b);
        self.state.strength_range_b.min = self
            .state
            .strength_range_b
            .min
            .min(self.state.strength_range_b.max);

        ui.group(|ui| {
            ui.label(title);
            ui.checkbox(
                &mut self.state.auto_limit_with_app_soft_limit,
                auto_limit_label,
            );
            ui.checkbox(
                &mut self.state.smooth_strength_enabled,
                smooth_strength_label,
            );
            if self.state.smooth_strength_enabled {
                ui.add(
                    egui::Slider::new(&mut self.state.smooth_strength_factor, 0.0..=1.0)
                        .text(smooth_factor_label),
                );
                self.state.smooth_strength_factor = self.state.normalized_smooth_strength_factor();
                ui.small(format!(
                    "{smooth_factor_hint}: {:.2}",
                    self.state.smooth_strength_factor
                ));
            }

            ui.columns(2, |columns| {
                columns[0].label(channel_a_label);
                columns[0].add(
                    egui::Slider::new(&mut self.state.strength_range_a.min, 0..=slider_max_a)
                        .text(min_label),
                );
                columns[0].add(
                    egui::Slider::new(&mut self.state.strength_range_a.max, 0..=slider_max_a)
                        .text(max_label),
                );
                self.state.strength_range_a = self.state.strength_range_a.normalized();

                columns[1].label(channel_b_label);
                columns[1].add(
                    egui::Slider::new(&mut self.state.strength_range_b.min, 0..=slider_max_b)
                        .text(min_label),
                );
                columns[1].add(
                    egui::Slider::new(&mut self.state.strength_range_b.max, 0..=slider_max_b)
                        .text(max_label),
                );
                self.state.strength_range_b = self.state.strength_range_b.normalized();
            });

            if let Some(report) = self.state.app_strength_report {
                ui.small(format!(
                    "{} A:{} B:{} | soft A:{} B:{}",
                    app_strength_label,
                    report.a_strength,
                    report.b_strength,
                    report.a_soft_limit,
                    report.b_soft_limit
                ));
            } else {
                ui.small(no_report_label);
            }

            if slider_max_a < 200 || slider_max_b < 200 {
                ui.small(format!(
                    "{}: A={slider_max_a}, B={slider_max_b}",
                    soft_limit_note_label
                ));
            }
        });
    }

    fn draw_waveform_panel(&mut self, ui: &mut egui::Ui) {
        let title = self.tr("Waveform Output", "波形输出");
        let pulse_mode_label = self.tr("Auto pulse mode", "自动波形模式");
        let pulse_mode_by_strength = self.tr("By strength", "按强度映射");
        let pulse_mode_always_max = self.tr("Always max waveform", "波形始终最高");
        let contrast_label = self.tr("Waveform contrast", "波形对比度");
        let contrast_hint = self.tr(
            "1.0 = linear mapping, higher value = larger amplitude changes",
            "1.0 = 线性映射，值越大振幅变化越明显",
        );
        let waveform_scope_note = self.tr(
            "This mode controls waveform shape only, not channel strength.",
            "该模式只控制波形形状，不会改变通道强度。",
        );
        let by_strength_note = self.tr(
            "By strength: pulse waveform amplitude follows current mapped strength.",
            "按强度映射：波形幅度跟随当前映射强度变化。",
        );
        let always_max_note = self.tr(
            "Always max waveform: pulse waveform uses max amplitude while strength still follows the strength panel.",
            "波形始终最高：波形幅度固定最大，但强度仍由左侧强度面板控制。",
        );
        let v3_note = self.tr(
            "V3 pulse format: 0A0A0A0A + amplitude bytes (00000000..64646464).",
            "V3 波形格式：0A0A0A0A + 幅度字节（00000000..64646464）。",
        );

        ui.group(|ui| {
            ui.label(title);
            egui::ComboBox::from_id_salt("auto_pulse_mode")
                .selected_text(match self.state.auto_pulse_mode {
                    AutoPulseMode::ByStrength => pulse_mode_by_strength,
                    AutoPulseMode::AlwaysMax => pulse_mode_always_max,
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.state.auto_pulse_mode,
                        AutoPulseMode::ByStrength,
                        pulse_mode_by_strength,
                    );
                    ui.selectable_value(
                        &mut self.state.auto_pulse_mode,
                        AutoPulseMode::AlwaysMax,
                        pulse_mode_always_max,
                    );
                });
            ui.small(format!(
                "{}: {}",
                pulse_mode_label,
                match self.state.auto_pulse_mode {
                    AutoPulseMode::ByStrength => pulse_mode_by_strength,
                    AutoPulseMode::AlwaysMax => pulse_mode_always_max,
                }
            ));
            if self.state.auto_pulse_mode == AutoPulseMode::ByStrength {
                ui.add(
                    egui::Slider::new(&mut self.state.waveform_contrast, 1.0..=4.0)
                        .text(contrast_label),
                );
                self.state.waveform_contrast = self.state.normalized_waveform_contrast();
                ui.small(format!(
                    "{contrast_hint}: {:.2}",
                    self.state.waveform_contrast
                ));
            }
            ui.small(waveform_scope_note);
            ui.small(match self.state.auto_pulse_mode {
                AutoPulseMode::ByStrength => by_strength_note,
                AutoPulseMode::AlwaysMax => always_max_note,
            });
            ui.small(v3_note);
        });
    }

    fn draw_speaker_source_panel(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label(self.tr("Audio Source (Speaker Loopback)", "音频源（扬声器回环）"));
            ui.small(self.tr(
                "Capture source is speaker playback loopback, not microphone.",
                "采集源是扬声器播放回环，不是麦克风。",
            ));

            ui.horizontal(|ui| {
                if ui
                    .button(self.tr("Refresh Speakers", "刷新扬声器"))
                    .clicked()
                {
                    self.refresh_output_device_list();
                }

                let active = self
                    .state
                    .selected_output_device
                    .as_deref()
                    .unwrap_or(self.tr("Auto (Default Speaker)", "自动（默认扬声器）"));
                ui.small(format!("{}: {active}", self.tr("Selected", "已选择")));
            });

            let auto_key = "__AUTO__".to_owned();
            let mut selected_key = self
                .state
                .selected_output_device
                .clone()
                .unwrap_or_else(|| auto_key.clone());
            let selected_label = if selected_key == auto_key {
                self.tr("Auto (Default Speaker)", "自动（默认扬声器）")
                    .to_owned()
            } else {
                selected_key.clone()
            };

            egui::ComboBox::from_id_salt("speaker_output_selector")
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut selected_key,
                        auto_key.clone(),
                        self.tr("Auto (Default Speaker)", "自动（默认扬声器）"),
                    );
                    for name in &self.state.available_output_devices {
                        ui.selectable_value(&mut selected_key, name.clone(), name);
                    }
                });

            let next_selection = if selected_key == auto_key {
                None
            } else {
                Some(selected_key)
            };
            if next_selection != self.state.selected_output_device {
                let selected_text = next_selection
                    .as_deref()
                    .unwrap_or(self.tr("default", "默认"))
                    .to_owned();
                self.state.selected_output_device = next_selection;
                self.state.set_protocol_action(format!(
                    "{} {selected_text}",
                    self.tr("speaker source switched to", "扬声器源已切换到")
                ));
            }

            if self.state.available_output_devices.is_empty() {
                ui.small(self.tr("No output speaker device found.", "未找到输出扬声器设备。"));
            } else {
                ui.small(format!(
                    "{}: {}",
                    self.tr("Detected output speakers", "检测到输出扬声器数量"),
                    self.state.available_output_devices.len()
                ));
            }

            if let Some(default_name) = default_output_device_name() {
                ui.small(format!(
                    "{}: {default_name}",
                    self.tr("System default speaker", "系统默认扬声器")
                ));
            }
        });
    }

    fn draw_settings_panel(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(self.tr("Settings", "设置"))
            .id_salt("settings_panel")
            .default_open(true)
            .show(ui, |ui| {
                self.draw_speaker_source_panel(ui);
                ui.separator();
                egui::CollapsingHeader::new(self.tr("Protocol Debug (Manual)", "协议调试（手动）"))
                    .id_salt("settings_protocol_debug_panel")
                    .default_open(false)
                    .show(ui, |ui| {
                        self.draw_protocol_debug_panel(ui);
                    });
            });
    }

    fn draw_protocol_debug_panel(&mut self, ui: &mut egui::Ui) {
        let value_label = self.tr("Value", "数值");
        let pulse_hint = self.tr(
            "Pulse HEX list, e.g. 0A0A0A0A00000000 0A0A0A0A0A0A0A0A",
            "波形 HEX 列表，例如 0A0A0A0A00000000 0A0A0A0A0A0A0A0A",
        );

        let debug_strength_max = self
            .state
            .effective_debug_strength_slider_max(self.state.debug_strength_channel);
        self.state.debug_strength_value = self.state.debug_strength_value.min(debug_strength_max);

        ui.small(self.tr(
            "Send raw control messages to App after bind. Fails will be shown explicitly.",
            "绑定后可向 App 发送原始控制消息，失败会明确显示。",
        ));

        ui.horizontal(|ui| {
            ui.label(self.tr("Strength", "强度"));
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
                    .text(value_label),
            );

            if ui.button(self.tr("Send Strength", "发送强度")).clicked() {
                self.send_debug_strength_message();
            }
        });

        ui.horizontal(|ui| {
            ui.label(self.tr("Clear", "清空"));
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

            if ui.button(self.tr("Send Clear", "发送清空")).clicked() {
                let message = build_clear_message(self.state.debug_clear_channel);
                self.send_manual_protocol_message(message, None);
            }
        });

        ui.horizontal(|ui| {
                ui.label(self.tr("Pulse", "波形"));
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

                if ui.button(self.tr("Load Sample", "加载示例")).clicked() {
                    self.state.debug_pulse_values =
                        "0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A"
                            .to_owned();
                }

                if ui.button(self.tr("Send Pulse", "发送波形")).clicked() {
                    match build_pulse_message(
                        self.state.debug_pulse_channel,
                        &self.state.debug_pulse_values,
                    ) {
                        Ok(message) => {
                            let note = self
                                .state
                                .app_current_strength_for_channel(self.state.debug_pulse_channel)
                                .and_then(|current| {
                                    if current == 0 {
                                        Some(self.tr(
                                            "channel strength is currently 0; pulse can be queued but output may be silent",
                                            "当前通道强度为 0；波形可入队，但实际输出可能无感",
                                        ).to_owned())
                                    } else {
                                        None
                                    }
                                });
                            self.send_manual_protocol_message(message, note);
                        }
                        Err(err) => self.state.set_error(err),
                    }
                }
            });

        ui.add(
            egui::TextEdit::multiline(&mut self.state.debug_pulse_values)
                .hint_text(pulse_hint)
                .desired_rows(3),
        );
        ui.small(self.tr(
            "Pulse HEX format: 16 hex chars each. Split by spaces, comma, semicolon or new line.",
            "波形 HEX 格式：每项 16 位十六进制，用空格/逗号/分号/换行分隔。",
        ));

        if let Some(last) = &self.state.last_protocol_action {
            ui.small(format!("{}: {last}", self.tr("Last action", "最近操作")));
        }
        if let Some(last_app) = &self.state.last_app_message {
            ui.small(format!(
                "{}: {last_app}",
                self.tr("Last app msg", "最近 App 消息")
            ));
        }
    }

    fn draw_band_editor(&mut self, ui: &mut egui::Ui) {
        let language = self.state.language;
        ui.group(|ui| {
            ui.label(self.tr("Band Routing (4 bands)", "频段路由（4 个频段）"));
            for index in 0..BAND_COUNT {
                let band_value = self.state.band_values[index];
                let routing = &mut self.state.band_routing[index];
                Self::draw_band_row(language, ui, index, routing, band_value);
                ui.separator();
            }
            egui::CollapsingHeader::new(self.tr("Band Help", "频段说明"))
                .default_open(false)
                .show(ui, |ui| {
                    let title = self.tr(
                        "Each band tracks a different speaker playback frequency range:",
                        "每个 band 代表扬声器回放中的不同频段：",
                    );
                    ui.small(title);
                    Self::draw_band_help_rows(language, ui);
                });
        });
    }

    fn draw_band_row(
        language: UiLanguage,
        ui: &mut egui::Ui,
        index: usize,
        routing: &mut BandRouting,
        band_value: f32,
    ) {
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut routing.enabled,
                format!("{} {}", tr(language, "Band", "频段"), index + 1),
            );
            ui.add(
                egui::Slider::new(&mut routing.threshold, 0.0..=1.0).text(tr(
                    language,
                    "Trigger",
                    "触发值",
                )),
            );
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

    fn draw_band_help_rows(language: UiLanguage, ui: &mut egui::Ui) {
        let descriptions = [
            (
                "Sub-bass / Bass: kick drum, bass line, heavy low-end.",
                "超低频/低频：底鼓、贝斯线、厚重低频。",
            ),
            (
                "Low-mid: vocal body, snare weight, guitar fundamentals.",
                "中低频：人声厚度、军鼓主体、吉他基音。",
            ),
            (
                "Mid-high presence: vocal clarity, lead instruments, attack.",
                "中高频存在感：人声清晰度、主旋律乐器、攻击感。",
            ),
            (
                "Highs / Air: hi-hat, cymbal sparkle, sibilance details.",
                "高频/空气感：踩镲、镲片亮度、齿音细节。",
            ),
        ];

        for index in 0..BAND_COUNT {
            let (lo, hi) = BAND_RANGES_HZ[index];
            let desc = tr(language, descriptions[index].0, descriptions[index].1);
            ui.small(format!(
                "{} {} ({:.0}-{:.0} Hz): {}",
                tr(language, "Band", "频段"),
                index + 1,
                lo,
                hi,
                desc
            ));
        }
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

    fn refresh_output_device_list(&mut self) {
        match list_output_device_names() {
            Ok(devices) => {
                self.state.available_output_devices = devices;
                if let Some(selected) = self.state.selected_output_device.as_ref() {
                    if !self
                        .state
                        .available_output_devices
                        .iter()
                        .any(|name| name == selected)
                    {
                        self.state.selected_output_device = None;
                    }
                }
                if self.state.selected_output_device.is_none() {
                    if let Some(default_name) = default_output_device_name() {
                        if self
                            .state
                            .available_output_devices
                            .iter()
                            .any(|name| name == &default_name)
                        {
                            self.state.selected_output_device = Some(default_name);
                        }
                    }
                }
            }
            Err(err) => {
                self.state.available_output_devices.clear();
                self.state.set_error(format!(
                    "{}: {err}",
                    self.tr("failed to enumerate speakers", "枚举扬声器失败")
                ));
            }
        }
    }

    fn start_engine(&mut self) {
        self.state.clear_error();
        match self.engine.start(&self.state.websocket_url) {
            Ok(()) => {
                self.state.running = true;
            }
            Err(err) => {
                self.state.running = false;
                self.state
                    .set_error(format!("{}: {err}", self.tr("start failed", "启动失败")));
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
                self.state.set_error(format!(
                    "{}: {err}",
                    self.tr("failed to restart ws server", "重启 WS 服务失败")
                ));
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
                                "{} ({soft_limit}) {} {}",
                                self.tr("clamped to app soft limit", "已按 App 软上限限幅"),
                                self.tr("for channel", "通道"),
                                channel.label()
                            ));
                        }
                    } else {
                        note = Some(
                            self.tr(
                                "app soft limit unknown; no clamp applied",
                                "App 软上限未知；未做限幅",
                            )
                            .to_owned(),
                        );
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
                                "{} {} {} {soft_limit}，{}",
                                self.tr("channel", "通道"),
                                channel.label(),
                                self.tr("already at soft limit", "已到软上限"),
                                self.tr("increase skipped", "已跳过增加"),
                            ));
                            self.state.set_protocol_action(
                                self.tr("send skipped by local guard", "被本地保护逻辑拦截发送")
                                    .to_owned(),
                            );
                            return;
                        }
                        if effective > max_delta {
                            effective = max_delta;
                            note = Some(format!(
                                "{} {max_delta} ({}, {}, {}, {})",
                                self.tr("increase clamped to", "增加量已限幅到"),
                                self.tr("current", "当前"),
                                current,
                                self.tr("soft", "软上限"),
                                soft_limit
                            ));
                        }
                    }
                }
                StrengthControlMode::Decrease => {
                    if let Some(current) = self.state.app_current_strength_for_channel(channel) {
                        if effective > current {
                            effective = current;
                            note = Some(format!(
                                "{} {current}",
                                self.tr(
                                    "decrease clamped to current strength",
                                    "减少量已限幅到当前强度"
                                )
                            ));
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
                let mut action = format!("{} {message_for_status}", self.tr("sent", "已发送"));
                if let Some(note) = note {
                    action.push_str(" (");
                    action.push_str(&note);
                    action.push(')');
                }
                self.state.set_protocol_action(action);
            }
            Err(err) => {
                self.state
                    .set_error(format!("{}: {err}", self.tr("send failed", "发送失败")));
                self.state.set_protocol_action(format!(
                    "{}: {message_for_status}",
                    self.tr("send failed", "发送失败")
                ));
            }
        }
    }

    fn sync_engine_snapshot(&mut self) {
        let snapshot = self.engine.snapshot();
        self.state.app_connected = snapshot.app_connected;
        self.state.app_bound = snapshot.app_bound;
        self.state.app_id = snapshot.app_id;
        self.state.output_strengths = snapshot.output_strengths;
        self.state.band_values = snapshot.latest_band_values;
        self.state.last_app_message = snapshot.last_app_message;
        self.state.last_server_info = snapshot.last_server_info;
        self.state.audio_capture_running = snapshot.audio_capture_running;
        self.state.audio_input_device = snapshot.audio_input_device;
        self.push_strength_history(self.state.output_strengths);

        if let Some(report) = snapshot.latest_strength {
            self.state.app_strength_report = Some(report);
            if self.state.auto_limit_with_app_soft_limit {
                let max_a = self
                    .state
                    .effective_strength_slider_max_for_channel(DglabChannel::A);
                self.state.strength_range_a.max = self.state.strength_range_a.max.min(max_a);
                self.state.strength_range_a.min = self
                    .state
                    .strength_range_a
                    .min
                    .min(self.state.strength_range_a.max);

                let max_b = self
                    .state
                    .effective_strength_slider_max_for_channel(DglabChannel::B);
                self.state.strength_range_b.max = self.state.strength_range_b.max.min(max_b);
                self.state.strength_range_b.min = self
                    .state
                    .strength_range_b
                    .min
                    .min(self.state.strength_range_b.max);

                let debug_max = self
                    .state
                    .effective_debug_strength_slider_max(self.state.debug_strength_channel);
                self.state.debug_strength_value = self.state.debug_strength_value.min(debug_max);
            }
        }
    }

    fn push_strength_history(&mut self, output_strengths: [u16; 2]) {
        let now = self.strength_history_started_at.elapsed().as_secs_f64();
        if let Some((last_time, last_a, last_b)) = self.strength_history.back_mut() {
            if now - *last_time < STRENGTH_HISTORY_SAMPLE_INTERVAL_SECONDS {
                *last_a = output_strengths[0];
                *last_b = output_strengths[1];
                return;
            }
        }
        self.strength_history
            .push_back((now, output_strengths[0], output_strengths[1]));

        while self.strength_history.len() > STRENGTH_HISTORY_MAX_POINTS {
            let _ = self.strength_history.pop_front();
        }

        while let Some((first_time, _, _)) = self.strength_history.front() {
            if now - *first_time <= STRENGTH_HISTORY_WINDOW_SECONDS {
                break;
            }
            let _ = self.strength_history.pop_front();
        }
    }

    fn draw_strength_history_plot(&self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            if self.strength_history.is_empty() {
                ui.small(self.tr("No strength data yet.", "暂无强度数据。"));
                return;
            }

            let now = self.strength_history_started_at.elapsed().as_secs_f64();
            let points_a: Vec<[f64; 2]> = self
                .strength_history
                .iter()
                .map(|(t, a, _)| [*t - now, *a as f64])
                .collect();
            let points_b: Vec<[f64; 2]> = self
                .strength_history
                .iter()
                .map(|(t, _, b)| [*t - now, *b as f64])
                .collect();
            let soft_max = self
                .state
                .app_strength_report
                .map(|r| r.a_soft_limit.max(r.b_soft_limit).max(1) as f64)
                .unwrap_or(200.0);

            let channel_a_name = self.tr("Channel A", "A 通道");
            let channel_b_name = self.tr("Channel B", "B 通道");
            let line_a = Line::new(PlotPoints::from(points_a))
                .name(channel_a_name)
                .color(egui::Color32::from_rgb(64, 160, 255));
            let line_b = Line::new(PlotPoints::from(points_b))
                .name(channel_b_name)
                .color(egui::Color32::from_rgb(255, 128, 64));

            Plot::new("output_strength_trend_plot")
                .height(180.0)
                .include_x(-STRENGTH_HISTORY_WINDOW_SECONDS)
                .include_x(0.0)
                .include_y(0.0)
                .include_y(soft_max)
                .allow_scroll(false)
                .allow_zoom(false)
                .legend(Legend::default())
                .show(ui, |plot_ui| {
                    plot_ui.line(line_a);
                    plot_ui.line(line_b);
                });
            ui.small(format!(
                "{}: {}",
                self.tr("Y max (App soft max)", "纵轴最大值（App 软上限）"),
                soft_max as u16
            ));

            ui.small(format!(
                "{}: A={} B={}",
                self.tr("Current output", "当前输出"),
                self.state.output_strengths[0],
                self.state.output_strengths[1]
            ));
        });
    }

    fn repaint_interval_ms(&self) -> u64 {
        if self.state.app_bound || self.state.audio_capture_running {
            REPAINT_ACTIVE_MS
        } else if self.engine.is_running() || self.state.running {
            REPAINT_RUNNING_MS
        } else {
            REPAINT_IDLE_MS
        }
    }

    fn sync_engine_settings(&self) {
        self.engine.update_settings(PipelineSettings {
            band_routing: self.state.band_routing,
            strength_ranges: [self.state.strength_range_a, self.state.strength_range_b],
            pulse_items_per_message: 1,
            auto_pulse_mode: self.state.auto_pulse_mode,
            waveform_contrast: self.state.normalized_waveform_contrast(),
            respect_app_soft_limit: self.state.auto_limit_with_app_soft_limit,
            smooth_strength_enabled: self.state.smooth_strength_enabled,
            smooth_strength_factor: self.state.normalized_smooth_strength_factor(),
            preferred_output_device_name: self.state.selected_output_device.clone(),
        });
    }

    fn persist_settings_if_changed(&mut self) {
        let current = PersistedSettings::from_state(&self.state);
        if current == self.last_persisted_settings {
            return;
        }

        if let Err(err) = settings::save_settings(&current) {
            tracing::warn!("failed to persist GUI settings: {err}");
        }
        self.last_persisted_settings = current;
    }
}

impl eframe::App for DgLinkGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.state.language != self.last_title_language {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                self.state.language.app_title().to_owned(),
            ));
            self.last_title_language = self.state.language;
        }
        self.sync_engine_snapshot();
        self.sync_engine_settings();
        self.refresh_qr_texture_if_needed(ctx);
        let collapse_pairing_now = self.state.app_bound && !self.prev_app_bound;

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.draw_top_bar(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.heading(self.tr("DG-Lab Audio Link", "DG-Lab 音频联动"));
                    ui.label(self.tr(
                        "Windows speaker output -> 4-band analysis -> DGLab A/B waveform output",
                        "Windows 扬声器输出 -> 4 频段分析 -> DGLab A/B 波形输出",
                    ));
                    ui.separator();
                    egui::CollapsingHeader::new(
                        self.tr("DGLab 3.0 Pairing QR", "DGLab 3.0 配对二维码"),
                    )
                    .id_salt("pairing_qr_panel")
                    .default_open(true)
                    .open(if collapse_pairing_now {
                        Some(false)
                    } else {
                        None
                    })
                    .show(ui, |ui| {
                        self.draw_pairing_panel(ui);
                    });
                    ui.separator();
                    ui.columns(2, |columns| {
                        self.draw_strength_range(&mut columns[0]);
                        self.draw_waveform_panel(&mut columns[1]);
                    });
                    ui.separator();
                    egui::CollapsingHeader::new(
                        self.tr("Output Strength Trend (A/B)", "输出强度曲线（A/B）"),
                    )
                    .id_salt("output_strength_trend_panel")
                    .default_open(true)
                    .show(ui, |ui| {
                        self.draw_strength_history_plot(ui);
                    });
                    ui.separator();
                    self.draw_band_editor(ui);
                    ui.separator();
                    self.draw_settings_panel(ui);
                });
        });

        if self.engine.is_running() && !self.state.running {
            self.state.running = true;
        } else if !self.engine.is_running() && self.state.running {
            self.state.running = false;
        }
        self.prev_app_bound = self.state.app_bound;
        self.persist_settings_if_changed();

        ctx.request_repaint_after(std::time::Duration::from_millis(self.repaint_interval_ms()));
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
