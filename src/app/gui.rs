use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
    time::Instant,
};

use dglab_socket_protocol::{
    pairing,
    protocol::{
        StrengthControlMode, build_clear_message, build_pulse_message, build_strength_message,
    },
};
use eframe::egui;
use egui_plot::{Legend, Line, Plot, PlotPoints};
use qrcodegen::{QrCode, QrCodeEcc};
use tokio::runtime::Runtime;

use crate::{
    app::{
        i18n::{UiLanguage, tr},
        logs::{GuiLogBuffer, GuiLogEntry, GuiLogLevel, GuiLogReloadHandle},
        scenes::{
            FACTORY_SCENE_PRESETS, FactorySceneId, SavedScene, USER_SCENE_SLOT_COUNT,
            factory_scene_preset,
        },
        settings::{self, PersistedSettings},
        state::AppState,
    },
    audio::capture::{default_output_device_name, list_output_device_names},
    pipeline::engine::{PipelineEngine, PipelineSettings},
    types::{
        AutoPulseMode, BAND_COUNT, BandDriveMode, BandRouting, DglabChannel, WaveformPattern,
        WaveformPatternMode, band_profile,
    },
};

pub struct DgLinkGuiApp {
    state: AppState,
    engine: PipelineEngine,
    log_buffer: GuiLogBuffer,
    log_reload_handle: GuiLogReloadHandle,
    log_level: GuiLogLevel,
    qr_texture: Option<egui::TextureHandle>,
    qr_error: Option<String>,
    last_qr_payload: String,
    prev_app_bound: bool,
    last_title_language: UiLanguage,
    last_persisted_settings: PersistedSettings,
    selected_factory_scene: FactorySceneId,
    scene_name_drafts: [String; USER_SCENE_SLOT_COUNT],
    show_log_panel: bool,
    log_auto_scroll: bool,
    selected_log_ids: BTreeSet<u64>,
    log_selection_anchor: Option<u64>,
    strength_history_started_at: Instant,
    strength_history: VecDeque<(f64, u16, u16)>,
}

const STRENGTH_HISTORY_MAX_POINTS: usize = 300;
const STRENGTH_HISTORY_WINDOW_SECONDS: f64 = 20.0;
const STRENGTH_HISTORY_SAMPLE_INTERVAL_SECONDS: f64 = 0.25;
const REPAINT_ACTIVE_MS: u64 = 120;
const REPAINT_RUNNING_MS: u64 = 250;
const REPAINT_IDLE_MS: u64 = 600;
const BASE_WINDOW_WIDTH: f32 = 980.0;
const LOG_PANEL_WIDTH_HINT: f32 = 540.0;

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
    pub fn new(
        runtime: Arc<Runtime>,
        language: UiLanguage,
        log_buffer: GuiLogBuffer,
        log_reload_handle: GuiLogReloadHandle,
        initial_log_level: GuiLogLevel,
    ) -> Self {
        let mut state = AppState::default();
        state.language = language;
        match settings::load_settings() {
            Ok(Some(saved)) => saved.apply_to_state(&mut state),
            Ok(None) => {}
            Err(err) => tracing::warn!("failed to load persisted GUI settings: {err}"),
        }
        let initial_language = state.language;
        let last_persisted_settings = PersistedSettings::from_state(&state);
        let scene_name_drafts = std::array::from_fn(|index| {
            state
                .saved_scenes
                .get(index)
                .and_then(|slot| slot.as_ref())
                .map(|scene| scene.name.clone())
                .unwrap_or_default()
        });

        let mut app = Self {
            state,
            engine: PipelineEngine::new(runtime),
            log_buffer,
            log_reload_handle,
            log_level: initial_log_level,
            qr_texture: None,
            qr_error: None,
            last_qr_payload: String::new(),
            prev_app_bound: false,
            last_title_language: initial_language,
            last_persisted_settings,
            selected_factory_scene: FactorySceneId::BalancedMotion,
            scene_name_drafts,
            show_log_panel: false,
            log_auto_scroll: true,
            selected_log_ids: BTreeSet::new(),
            log_selection_anchor: None,
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

    fn default_scene_slot_name(&self, slot_index: usize) -> String {
        format!(
            "{} {}",
            self.tr("Scene", "场景"),
            slot_index.saturating_add(1)
        )
    }

    fn save_scene_to_slot(&mut self, slot_index: usize) {
        if slot_index >= USER_SCENE_SLOT_COUNT {
            return;
        }

        let draft_name = self.scene_name_drafts[slot_index].trim();
        let name = if draft_name.is_empty() {
            self.default_scene_slot_name(slot_index)
        } else {
            draft_name.to_owned()
        };

        self.scene_name_drafts[slot_index] = name.clone();
        self.state.saved_scenes[slot_index] = Some(SavedScene {
            name: name.clone(),
            config: self.state.capture_scene_config(),
        });
        self.state.clear_error();
        tracing::info!(
            "saved current setup to scene slot {} ({name})",
            slot_index + 1
        );
    }

    fn load_scene_from_slot(&mut self, slot_index: usize) {
        let Some(saved) = self
            .state
            .saved_scenes
            .get(slot_index)
            .and_then(|slot| slot.clone())
        else {
            return;
        };

        self.state.apply_scene_config(&saved.config);
        self.scene_name_drafts[slot_index] = saved.name.clone();
        self.state.clear_error();
        tracing::info!("loaded scene slot {} ({})", slot_index + 1, saved.name);
    }

    fn clear_scene_slot(&mut self, slot_index: usize) {
        if slot_index >= USER_SCENE_SLOT_COUNT {
            return;
        }

        self.state.saved_scenes[slot_index] = None;
        self.scene_name_drafts[slot_index].clear();
        self.state.clear_error();
        tracing::info!("cleared scene slot {}", slot_index + 1);
    }

    fn apply_factory_scene(&mut self, id: FactorySceneId) {
        let preset = factory_scene_preset(id);
        self.state.apply_factory_scene_config(&preset.config);
        self.state.clear_error();
        tracing::info!("applied factory preset `{}`", preset.name_en);
    }

    fn set_log_level(&mut self, level: GuiLogLevel) {
        if self.log_level == level {
            return;
        }

        match self
            .log_reload_handle
            .modify(|filter| *filter = tracing_subscriber::EnvFilter::new(level.directive()))
        {
            Ok(()) => {
                self.log_level = level;
                self.state.clear_error();
                tracing::info!("GUI log level changed to {}", level.directive());
            }
            Err(err) => {
                self.state
                    .set_error(format!("failed to change log level: {err}"));
            }
        }
    }

    fn set_log_panel_visibility(&mut self, ctx: &egui::Context, show: bool) {
        if self.show_log_panel == show {
            return;
        }

        let current_size = ctx
            .input(|i| i.viewport().inner_rect.map(|rect| rect.size()))
            .unwrap_or_else(|| ctx.screen_rect().size());

        let next_width = if show {
            current_size.x + LOG_PANEL_WIDTH_HINT
        } else {
            (current_size.x - LOG_PANEL_WIDTH_HINT).max(BASE_WINDOW_WIDTH)
        };

        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            next_width,
            current_size.y,
        )));
        self.show_log_panel = show;
    }

    fn log_entries_text(entries: &[GuiLogEntry]) -> String {
        entries
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn log_entries_text_from_refs(entries: &[&GuiLogEntry]) -> String {
        entries
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn selected_log_entries<'a>(&self, entries: &'a [GuiLogEntry]) -> Vec<&'a GuiLogEntry> {
        entries
            .iter()
            .filter(|entry| self.selected_log_ids.contains(&entry.id))
            .collect()
    }

    fn retain_visible_log_selection(&mut self, entries: &[GuiLogEntry]) {
        let visible_ids = entries
            .iter()
            .map(|entry| entry.id)
            .collect::<BTreeSet<_>>();
        self.selected_log_ids.retain(|id| visible_ids.contains(id));
        if self
            .log_selection_anchor
            .is_some_and(|id| !visible_ids.contains(&id))
        {
            self.log_selection_anchor = None;
        }
    }

    fn toggle_log_selection(
        &mut self,
        entries: &[GuiLogEntry],
        clicked_id: u64,
        modifiers: egui::Modifiers,
    ) {
        let clicked_index = match entries.iter().position(|entry| entry.id == clicked_id) {
            Some(index) => index,
            None => return,
        };

        if modifiers.shift {
            if let Some(anchor_id) = self.log_selection_anchor {
                if let Some(anchor_index) = entries.iter().position(|entry| entry.id == anchor_id) {
                    let (start, end) = if anchor_index <= clicked_index {
                        (anchor_index, clicked_index)
                    } else {
                        (clicked_index, anchor_index)
                    };
                    self.selected_log_ids.clear();
                    for entry in &entries[start..=end] {
                        self.selected_log_ids.insert(entry.id);
                    }
                    return;
                }
            }
        }

        if modifiers.command || modifiers.ctrl {
            if !self.selected_log_ids.insert(clicked_id) {
                self.selected_log_ids.remove(&clicked_id);
            }
            self.log_selection_anchor = Some(clicked_id);
            return;
        }

        self.selected_log_ids.clear();
        self.selected_log_ids.insert(clicked_id);
        self.log_selection_anchor = Some(clicked_id);
    }

    fn copy_logs_text(&self, ctx: &egui::Context, text: String) {
        ctx.copy_text(text);
    }

    fn export_logs_to_file(&mut self, text: &str) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Text", &["txt"])
            .set_file_name("dglab-audio-link-log.txt")
            .save_file()
        {
            if let Err(err) = std::fs::write(&path, text) {
                self.state.set_error(format!(
                    "failed to write log file `{}`: {err}",
                    path.display()
                ));
            } else {
                self.state.clear_error();
                self.state
                    .set_protocol_action(format!("log exported to {}", path.display()));
            }
        }
    }

    fn emit_test_logs(&self) {
        tracing::error!(
            "test error: socket payload rejected {{code=405, size=2048, sample='[\"0A0A...\"]'}}"
        );
        tracing::warn!("test warn: requested speaker `LG ULTRAGEAR` fallback to default endpoint");
        tracing::info!("test info: app connected targetId=7e04d0a7-b6c0-4fa1-b255-5055c47b3374");
        tracing::debug!(
            "test debug: pulse items=[0A0A0A0A00000000,0A0A0A0A64646464] band=[0.17,0.52,0.88,0.11]"
        );
        tracing::trace!(
            "test trace: fft bins=1024 offset=37.25 smooth=0.70 selection='copy all' token=abc123_xyz"
        );
    }

    fn log_level_row_fill(
        level: GuiLogLevel,
        selected: bool,
        visuals: &egui::Visuals,
    ) -> egui::Color32 {
        let selected_fill = egui::Color32::from_rgb(236, 242, 250);
        let base = match level {
            GuiLogLevel::Error => egui::Color32::from_rgb(255, 228, 228),
            GuiLogLevel::Warn => egui::Color32::from_rgb(255, 242, 214),
            GuiLogLevel::Info => egui::Color32::TRANSPARENT,
            GuiLogLevel::Debug => egui::Color32::from_rgb(229, 246, 237),
            GuiLogLevel::Trace => egui::Color32::from_rgb(238, 238, 238),
        };

        if selected {
            if level == GuiLogLevel::Info {
                return selected_fill;
            }
            let highlight = visuals.selection.bg_fill;
            egui::Color32::from_rgba_unmultiplied(
                ((u16::from(base.r()) + u16::from(highlight.r())) / 2) as u8,
                ((u16::from(base.g()) + u16::from(highlight.g())) / 2) as u8,
                ((u16::from(base.b()) + u16::from(highlight.b())) / 2) as u8,
                255,
            )
        } else {
            base
        }
    }

    fn draw_top_bar(&mut self, ui: &mut egui::Ui) {
        let log_level_label = self.tr("Log level:", "日志级别：");
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

            let mut selected_log_level = self.log_level;
            ui.label(log_level_label);
            egui::ComboBox::from_id_salt("gui_log_level")
                .selected_text(self.log_level.directive())
                .show_ui(ui, |ui| {
                    for level in GuiLogLevel::all() {
                        ui.selectable_value(&mut selected_log_level, level, level.directive());
                    }
                });
            if selected_log_level != self.log_level {
                self.set_log_level(selected_log_level);
            }
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

            let logs_button = if self.show_log_panel {
                self.tr("Hide Logs", "隐藏日志")
            } else {
                self.tr("Show Logs", "显示日志")
            };
            if ui.button(logs_button).clicked() {
                self.set_log_panel_visibility(ui.ctx(), !self.show_log_panel);
            }
        });

        if let Some(err) = &self.state.last_error {
            ui.colored_label(egui::Color32::from_rgb(200, 40, 40), err);
        }
    }

    fn draw_log_panel(&mut self, ui: &mut egui::Ui) {
        let entries = self.log_buffer.snapshot();
        self.retain_visible_log_selection(&entries);
        let logs_label = self.tr("Logs", "日志");
        let clear_label = self.tr("Clear", "清空");
        let clear_selection_label = self.tr("Clear Selection", "清空选择");
        let select_all_label = self.tr("Select All", "全选");
        let copy_selected_label = self.tr("Copy Selected", "复制已选");
        let copy_all_label = self.tr("Copy All", "复制全部");
        let save_selected_label = self.tr("Save Selected", "导出已选");
        let save_all_label = self.tr("Save All", "导出全部");
        let generate_test_logs_label = self.tr("Generate Test Logs", "生成测试日志");
        let copy_item_label = self.tr("Copy", "复制");
        let auto_scroll_label = self.tr("Always scroll to bottom", "始终滚动到底部");
        let collapse_label = self.tr("Collapse", "收起");
        let lines_label = self.tr("Lines", "行数");
        let mut force_scroll_to_bottom = false;
        let mut always_scroll = self.log_auto_scroll;
        let selected_entries = self.selected_log_entries(&entries);
        let selected_text = Self::log_entries_text_from_refs(&selected_entries);
        let all_text = Self::log_entries_text(&entries);
        let has_selection = !selected_entries.is_empty();
        let mut copy_single_text: Option<String> = None;

        ui.horizontal(|ui| {
            ui.heading(logs_label);
            if ui.button(clear_label).clicked() {
                self.log_buffer.clear();
                self.selected_log_ids.clear();
                self.log_selection_anchor = None;
            }
            if ui
                .add_enabled(has_selection, egui::Button::new(clear_selection_label))
                .clicked()
            {
                self.selected_log_ids.clear();
                self.log_selection_anchor = None;
            }
            if ui.button(select_all_label).clicked() {
                self.selected_log_ids = entries.iter().map(|entry| entry.id).collect();
                self.log_selection_anchor = entries.last().map(|entry| entry.id);
            }
            if ui.button(generate_test_logs_label).clicked() {
                self.emit_test_logs();
            }
        });

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(has_selection, egui::Button::new(copy_selected_label))
                .clicked()
            {
                self.copy_logs_text(ui.ctx(), selected_text.clone());
            }
            if ui.button(copy_all_label).clicked() {
                self.copy_logs_text(ui.ctx(), all_text.clone());
            }
            if ui
                .add_enabled(has_selection, egui::Button::new(save_selected_label))
                .clicked()
            {
                self.export_logs_to_file(&selected_text);
            }
            if ui.button(save_all_label).clicked() {
                self.export_logs_to_file(&all_text);
            }
            let auto_scroll_response = ui.checkbox(&mut always_scroll, auto_scroll_label);
            if auto_scroll_response.changed() && always_scroll {
                force_scroll_to_bottom = true;
            }
            if ui.button(collapse_label).clicked() {
                self.set_log_panel_visibility(ui.ctx(), false);
            }
        });
        ui.separator();
        ui.small(format!(
            "{lines_label}: {} | {}: {}",
            entries.len(),
            self.tr("Selected", "已选"),
            selected_entries.len()
        ));

        let scroll_output = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.log_auto_scroll || force_scroll_to_bottom)
            .show(ui, |ui| {
                ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                ui.spacing_mut().item_spacing.y = 2.0;
                for entry in &entries {
                    let selected = self.selected_log_ids.contains(&entry.id);
                    let fill = Self::log_level_row_fill(entry.level, selected, ui.visuals());
                    let row_width = ui.available_width();
                    let inner_width = (row_width - 12.0).max(0.0);
                    let frame = egui::Frame::none()
                        .fill(fill)
                        .stroke(if selected {
                            egui::Stroke::new(1.0, ui.visuals().selection.stroke.color)
                        } else {
                            egui::Stroke::new(
                                1.0,
                                ui.visuals().widgets.noninteractive.bg_stroke.color,
                            )
                        })
                        .inner_margin(egui::Margin::symmetric(6.0, 4.0));

                    let response = frame
                        .show(ui, |ui| {
                            ui.set_min_width(inner_width);
                            ui.set_max_width(inner_width);
                            let level_text =
                                format!("[{}]", entry.level.directive().to_ascii_uppercase());
                            let dark_text = egui::Color32::from_rgb(34, 34, 34);
                            ui.horizontal_top(|ui| {
                                ui.add_sized(
                                    [64.0, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new(level_text)
                                            .monospace()
                                            .strong()
                                            .color(dark_text),
                                    ),
                                );
                                ui.scope(|ui| {
                                    ui.style_mut().visuals.override_text_color = Some(dark_text);
                                    ui.vertical(|ui| {
                                        if !entry.timestamp.is_empty() || !entry.target.is_empty() {
                                            ui.horizontal_wrapped(|ui| {
                                                if !entry.timestamp.is_empty() {
                                                    ui.label(
                                                        egui::RichText::new(&entry.timestamp)
                                                            .monospace()
                                                            .small()
                                                            .color(egui::Color32::from_rgb(
                                                                90, 90, 90,
                                                            )),
                                                    );
                                                }
                                                if !entry.target.is_empty() {
                                                    ui.label(
                                                        egui::RichText::new(&entry.target)
                                                            .monospace()
                                                            .small()
                                                            .color(egui::Color32::from_rgb(
                                                                70, 90, 110,
                                                            )),
                                                    );
                                                }
                                            });
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&entry.message)
                                                    .monospace()
                                                    .color(dark_text),
                                            )
                                            .wrap(),
                                        );
                                    });
                                });
                            });
                        })
                        .response
                        .interact(egui::Sense::click());

                    if response.clicked() {
                        let modifiers = ui.input(|i| i.modifiers);
                        self.toggle_log_selection(&entries, entry.id, modifiers);
                    }

                    response.context_menu(|ui| {
                        if ui.button(copy_item_label).clicked() {
                            copy_single_text = Some(entry.text.clone());
                            ui.close_menu();
                        }
                    });
                }
                if force_scroll_to_bottom {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                }
            });

        if let Some(text) = copy_single_text {
            self.copy_logs_text(ui.ctx(), text);
        }

        let max_scroll_y =
            (scroll_output.content_size.y - scroll_output.inner_rect.height()).max(0.0);
        let at_bottom =
            max_scroll_y <= 1.0 || scroll_output.state.offset.y >= (max_scroll_y - 2.0).max(0.0);
        if force_scroll_to_bottom {
            self.log_auto_scroll = true;
            ui.ctx().request_repaint();
        } else {
            self.log_auto_scroll = at_bottom;
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
        let pattern_mode_label = self.tr("Pattern mode", "波形模式");
        let fixed_pattern_mode = self.tr("Fixed", "固定");
        let auto_pattern_mode = self.tr("Auto morph", "自动形变");
        let pattern_label = self.tr("Waveform pattern", "波形样式");
        let smooth_label = self.tr("Smooth", "平滑");
        let punch_label = self.tr("Punch", "重击");
        let tide_label = self.tr("Tide", "潮汐");
        let ripple_label = self.tr("Ripple", "涟漪");
        let shimmer_label = self.tr("Shimmer", "闪烁");
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
        let auto_pattern_note = self.tr(
            "Auto morph picks a waveform from current energy/onset characteristics and morphs from smooth toward that shape.",
            "自动形变会根据当前能量/瞬态特征选择波形，并从平滑形态连续过渡过去。",
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
            egui::ComboBox::from_id_salt("waveform_pattern_mode")
                .selected_text(match self.state.waveform_pattern_mode {
                    WaveformPatternMode::Fixed => fixed_pattern_mode,
                    WaveformPatternMode::AutoMorph => auto_pattern_mode,
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.state.waveform_pattern_mode,
                        WaveformPatternMode::Fixed,
                        fixed_pattern_mode,
                    );
                    ui.selectable_value(
                        &mut self.state.waveform_pattern_mode,
                        WaveformPatternMode::AutoMorph,
                        auto_pattern_mode,
                    );
                });
            ui.small(format!(
                "{}: {}",
                pattern_mode_label,
                match self.state.waveform_pattern_mode {
                    WaveformPatternMode::Fixed => fixed_pattern_mode,
                    WaveformPatternMode::AutoMorph => auto_pattern_mode,
                }
            ));
            if self.state.waveform_pattern_mode == WaveformPatternMode::Fixed {
                egui::ComboBox::from_id_salt("waveform_pattern")
                    .selected_text(match self.state.waveform_pattern {
                        WaveformPattern::Smooth => smooth_label,
                        WaveformPattern::Punch => punch_label,
                        WaveformPattern::Tide => tide_label,
                        WaveformPattern::Ripple => ripple_label,
                        WaveformPattern::Shimmer => shimmer_label,
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.state.waveform_pattern,
                            WaveformPattern::Smooth,
                            smooth_label,
                        );
                        ui.selectable_value(
                            &mut self.state.waveform_pattern,
                            WaveformPattern::Punch,
                            punch_label,
                        );
                        ui.selectable_value(
                            &mut self.state.waveform_pattern,
                            WaveformPattern::Tide,
                            tide_label,
                        );
                        ui.selectable_value(
                            &mut self.state.waveform_pattern,
                            WaveformPattern::Ripple,
                            ripple_label,
                        );
                        ui.selectable_value(
                            &mut self.state.waveform_pattern,
                            WaveformPattern::Shimmer,
                            shimmer_label,
                        );
                    });
                ui.small(format!(
                    "{}: {}",
                    pattern_label,
                    match self.state.waveform_pattern {
                        WaveformPattern::Smooth => smooth_label,
                        WaveformPattern::Punch => punch_label,
                        WaveformPattern::Tide => tide_label,
                        WaveformPattern::Ripple => ripple_label,
                        WaveformPattern::Shimmer => shimmer_label,
                    }
                ));
            } else {
                ui.small(auto_pattern_note);
            }
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

    fn draw_scene_panel(&mut self, ui: &mut egui::Ui) {
        let language = self.state.language;
        let title = self.tr("Scenes & Presets", "场景与预设");
        let factory_title = self.tr("Factory preset", "内置预设");
        let factory_apply = self.tr("Apply preset", "应用预设");
        let saved_title = self.tr("Saved scenes", "已保存场景");
        let save_label = self.tr("Save", "保存");
        let load_label = self.tr("Load", "载入");
        let clear_label = self.tr("Clear", "清空");
        let empty_slot_label = self.tr("Empty slot", "空槽位");
        let preset_note = self.tr(
            "Presets are starting points. They change style settings but keep your current A/B strength ranges.",
            "预设只是起点。它们会改变风格参数，但会保留你当前的 A/B 强度范围。",
        );
        let drive_mode_label = self.tr("Drive", "驱动");
        let pulse_mode_label = self.tr("Pulse", "波形");
        let smoothing_label = self.tr("Smooth", "平滑");

        ui.group(|ui| {
            ui.label(title);
            ui.small(preset_note);
            ui.separator();

            let preset = factory_scene_preset(self.selected_factory_scene);
            ui.horizontal_wrapped(|ui| {
                ui.label(factory_title);
                egui::ComboBox::from_id_salt("factory_scene_preset")
                    .selected_text(tr(language, preset.name_en, preset.name_zh))
                    .show_ui(ui, |ui| {
                        for preset in FACTORY_SCENE_PRESETS {
                            ui.selectable_value(
                                &mut self.selected_factory_scene,
                                preset.id,
                                tr(language, preset.name_en, preset.name_zh),
                            );
                        }
                    });
                if ui.button(factory_apply).clicked() {
                    self.apply_factory_scene(self.selected_factory_scene);
                }
            });
            let preset = factory_scene_preset(self.selected_factory_scene);
            ui.small(self.tr(preset.summary_en, preset.summary_zh));

            ui.separator();
            ui.label(saved_title);
            for slot_index in 0..USER_SCENE_SLOT_COUNT {
                let saved_scene = self
                    .state
                    .saved_scenes
                    .get(slot_index)
                    .and_then(|slot| slot.clone());
                let has_scene = saved_scene.is_some();
                let fallback_name = self.default_scene_slot_name(slot_index);

                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{} {}", self.tr("Slot", "槽位"), slot_index + 1));
                    ui.add_sized(
                        [180.0, 0.0],
                        egui::TextEdit::singleline(&mut self.scene_name_drafts[slot_index])
                            .hint_text(fallback_name),
                    );
                    if ui.button(save_label).clicked() {
                        self.save_scene_to_slot(slot_index);
                    }
                    if ui
                        .add_enabled(has_scene, egui::Button::new(load_label))
                        .clicked()
                    {
                        self.load_scene_from_slot(slot_index);
                    }
                    if ui
                        .add_enabled(has_scene, egui::Button::new(clear_label))
                        .clicked()
                    {
                        self.clear_scene_slot(slot_index);
                    }
                });

                if let Some(scene) = saved_scene.as_ref() {
                    let config = scene.config;
                    let drive_mode = match config.band_drive_mode {
                        BandDriveMode::Energy => self.tr("Energy", "能量"),
                        BandDriveMode::Onset => self.tr("Onset", "瞬态"),
                    };
                    let pulse_mode = match config.auto_pulse_mode {
                        AutoPulseMode::ByStrength => self.tr("By strength", "按强度映射"),
                        AutoPulseMode::AlwaysMax => self.tr("Always max", "始终最高"),
                    };
                    ui.small(format!(
                        "{}: {} | {}: {} | A {}-{} / B {}-{} | {}: {}",
                        drive_mode_label,
                        drive_mode,
                        pulse_mode_label,
                        pulse_mode,
                        config.strength_range_a.min,
                        config.strength_range_a.max,
                        config.strength_range_b.min,
                        config.strength_range_b.max,
                        smoothing_label,
                        if config.smooth_strength_enabled {
                            format!("{:.2}", config.smooth_strength_factor)
                        } else {
                            self.tr("off", "关闭").to_owned()
                        }
                    ));
                } else {
                    ui.small(empty_slot_label);
                }

                if slot_index + 1 < USER_SCENE_SLOT_COUNT {
                    ui.separator();
                }
            }
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
        let drive_mode_label = self.tr("Band drive mode", "频段驱动模式");
        let energy_label = self.tr("Energy", "能量");
        let onset_label = self.tr("Onset", "瞬态");
        let onset_note = self.tr(
            "Onset emphasizes sudden hits and beat attacks instead of sustained loudness.",
            "瞬态模式更强调鼓点/攻击瞬间，而不是持续响度。",
        );
        let band_role_note = self.tr(
            "These 4 bands are tuned for musical roles, not evenly spaced engineering ranges.",
            "这 4 个 band 按音乐角色调过，不是平均切开的工程频段。",
        );
        ui.group(|ui| {
            ui.label(self.tr("Band Routing (4 roles)", "频段路由（4 个角色）"));
            egui::ComboBox::from_id_salt("band_drive_mode")
                .selected_text(match self.state.band_drive_mode {
                    BandDriveMode::Energy => energy_label,
                    BandDriveMode::Onset => onset_label,
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.state.band_drive_mode,
                        BandDriveMode::Energy,
                        energy_label,
                    );
                    ui.selectable_value(
                        &mut self.state.band_drive_mode,
                        BandDriveMode::Onset,
                        onset_label,
                    );
                });
            ui.small(format!(
                "{}: {}",
                drive_mode_label,
                match self.state.band_drive_mode {
                    BandDriveMode::Energy => energy_label,
                    BandDriveMode::Onset => onset_label,
                }
            ));
            if self.state.band_drive_mode == BandDriveMode::Onset {
                ui.small(onset_note);
            }
            ui.small(band_role_note);
            ui.separator();
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
        let profile = band_profile(index);
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut routing.enabled,
                format!(
                    "{} {} · {}",
                    tr(language, "Band", "频段"),
                    index + 1,
                    tr(language, profile.short_name_en, profile.short_name_zh),
                ),
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
        let (lo, hi) = profile.range_hz();
        ui.small(format!(
            "{lo:.0}-{hi:.0} Hz · {} · {} {:.2}",
            tr(language, profile.detail_en, profile.detail_zh),
            tr(language, "Suggested trigger", "建议触发值"),
            profile.recommended_threshold,
        ));

        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::Slider::new(&mut routing.attack_ms, 0..=1_000).text(tr(
                    language,
                    "Attack ms",
                    "起音 ms",
                )),
            );
            ui.add(egui::Slider::new(&mut routing.hold_ms, 0..=1_000).text(tr(
                language,
                "Hold ms",
                "保持 ms",
            )));
            ui.add(
                egui::Slider::new(&mut routing.release_ms, 0..=2_000).text(tr(
                    language,
                    "Release ms",
                    "释音 ms",
                )),
            );
        });
    }

    fn draw_band_help_rows(language: UiLanguage, ui: &mut egui::Ui) {
        for index in 0..BAND_COUNT {
            let profile = band_profile(index);
            let (lo, hi) = profile.range_hz();
            ui.small(format!(
                "{} {} · {} ({:.0}-{:.0} Hz): {}",
                tr(language, "Band", "频段"),
                index + 1,
                tr(language, profile.short_name_en, profile.short_name_zh),
                lo,
                hi,
                tr(language, profile.detail_en, profile.detail_zh),
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
        self.sync_engine_settings();
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
        self.sync_engine_settings();
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
            band_drive_mode: self.state.band_drive_mode,
            waveform_pattern_mode: self.state.waveform_pattern_mode,
            waveform_pattern: self.state.waveform_pattern,
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

        if self.show_log_panel {
            egui::SidePanel::right("log_panel")
                .default_width(LOG_PANEL_WIDTH_HINT)
                .min_width(360.0)
                .resizable(true)
                .show(ctx, |ui| {
                    self.draw_log_panel(ui);
                });
        }

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
                    self.draw_scene_panel(ui);
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
