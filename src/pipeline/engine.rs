use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow};
use dglab_socket_protocol::{
    pairing,
    protocol::{
        MAX_JSON_CHARS, SocketPacket, StrengthControlMode, StrengthReport, build_clear_message,
        build_pulse_message_from_items, build_strength_message, parse_strength_report,
    },
    server::{DglabWsServer, DglabWsServerConfig, DglabWsServerControl, DglabWsServerEvent, DglabWsServerStatus},
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, watch},
    task::JoinHandle,
};

use crate::{
    audio::capture::{LoopbackCapture, LoopbackCaptureConfig},
    domain::{
        BAND_COUNT,
        types::{AutoPulseMode, BandRouting, DglabChannel, StrengthRange},
    },
    signal::mapper::{aggregate_channel_strengths, compute_band_outputs},
};

const DEFAULT_SEND_INTERVAL_MS: u64 = 100;

#[derive(Debug, Clone)]
pub struct PipelineSettings {
    pub band_routing: [BandRouting; BAND_COUNT],
    pub strength_ranges: [StrengthRange; 2],
    pub pulse_items_per_message: usize,
    pub auto_pulse_mode: AutoPulseMode,
    pub waveform_contrast: f32,
    pub respect_app_soft_limit: bool,
    pub smooth_strength_enabled: bool,
    pub smooth_strength_factor: f32,
    pub preferred_output_device_name: Option<String>,
}

impl Default for PipelineSettings {
    fn default() -> Self {
        Self {
            band_routing: [BandRouting::default(); BAND_COUNT],
            strength_ranges: [StrengthRange::new(10, 160), StrengthRange::new(10, 160)],
            pulse_items_per_message: 1,
            auto_pulse_mode: AutoPulseMode::ByStrength,
            waveform_contrast: 1.8,
            respect_app_soft_limit: true,
            smooth_strength_enabled: true,
            smooth_strength_factor: 0.70,
            preferred_output_device_name: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineSnapshot {
    pub app_connected: bool,
    pub app_bound: bool,
    pub app_id: Option<String>,
    pub latest_strength: Option<StrengthReport>,
    pub output_strengths: [u16; 2],
    pub latest_band_values: [f32; BAND_COUNT],
    pub audio_capture_running: bool,
    pub audio_input_device: Option<String>,
    pub last_app_message: Option<String>,
    pub last_server_info: Option<String>,
}

impl Default for EngineSnapshot {
    fn default() -> Self {
        Self {
            app_connected: false,
            app_bound: false,
            app_id: None,
            latest_strength: None,
            output_strengths: [0; 2],
            latest_band_values: [0.0; BAND_COUNT],
            audio_capture_running: false,
            audio_input_device: None,
            last_app_message: None,
            last_server_info: None,
        }
    }
}

#[derive(Debug)]
pub struct PipelineEngine {
    runtime: Arc<Runtime>,
    worker: Option<JoinHandle<()>>,
    server_control: Option<DglabWsServerControl>,
    server_status_rx: Option<watch::Receiver<DglabWsServerStatus>>,
    snapshot: Arc<Mutex<EngineSnapshot>>,
    settings: Arc<Mutex<PipelineSettings>>,
}

impl PipelineEngine {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            worker: None,
            server_control: None,
            server_status_rx: None,
            snapshot: Arc::new(Mutex::new(EngineSnapshot::default())),
            settings: Arc::new(Mutex::new(PipelineSettings::default())),
        }
    }

    pub fn is_running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    pub fn update_settings(&self, settings: PipelineSettings) {
        if let Ok(mut current) = self.settings.lock() {
            *current = settings;
        }
    }

    pub fn start(&mut self, ws_url: &str) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }

        self.worker = None;
        self.server_control = None;
        self.server_status_rx = None;

        let parsed = pairing::parse_control_ws_url(ws_url)
            .ok_or_else(|| anyhow!("invalid ws url. expected ws://<host>:<port>/<session-id>"))?;
        let bind_addr = format!("0.0.0.0:{}", parsed.port);
        let controller_id = parsed.session_id;

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<DglabWsServerEvent>();
        let server = DglabWsServer::new(
            DglabWsServerConfig::new(bind_addr.clone(), controller_id.clone()),
            event_tx,
        );
        self.server_status_rx = Some(server.subscribe_status());
        self.server_control = Some(server.control());
        let control_for_worker = self
            .server_control
            .clone()
            .ok_or_else(|| anyhow!("server control is unavailable"))?;

        self.set_snapshot(|snapshot| {
            snapshot.app_connected = false;
            snapshot.app_bound = false;
            snapshot.app_id = None;
            snapshot.latest_strength = None;
            snapshot.output_strengths = [0; 2];
            snapshot.latest_band_values = [0.0; BAND_COUNT];
            snapshot.audio_capture_running = false;
            snapshot.audio_input_device = None;
            snapshot.last_app_message = None;
            snapshot.last_server_info = Some(format!(
                "ws server starting on {bind_addr}, session={controller_id}"
            ));
        });

        let snapshot = Arc::clone(&self.snapshot);
        let settings = Arc::clone(&self.settings);

        self.worker = Some(self.runtime.spawn(async move {
            tracing::info!(
                "pipeline worker start: bind={bind_addr}, controller_id={controller_id}"
            );

            let mut app_bound = false;
            let mut channel_active = [false; 2];
            let mut last_strength = [0_u16; 2];
            let mut smoothed_strength = [0_u16; 2];
            let mut latest_bands = [0.0_f32; BAND_COUNT];
            let mut pending_peak_bands = [0.0_f32; BAND_COUNT];
            let mut latest_soft_limits = [200_u16; 2];
            let mut active_output_preference = settings
                .lock()
                .ok()
                .and_then(|s| s.preferred_output_device_name.clone());

            let (band_tx, mut band_rx) = mpsc::unbounded_channel::<[f32; BAND_COUNT]>();
            let mut capture = LoopbackCapture::new(LoopbackCaptureConfig {
                preferred_output_device_name: active_output_preference.clone(),
                ..LoopbackCaptureConfig::default()
            });
            if let Err(err) = capture.start(band_tx.clone()) {
                tracing::warn!("audio capture failed to start: {err}");
                update_snapshot(&snapshot, |state| {
                    state.audio_capture_running = false;
                    state.audio_input_device = None;
                    state.last_server_info = Some(format!("audio capture unavailable: {err}"));
                });
            } else {
                let device_name = capture.selected_device_name().map(str::to_owned);
                update_snapshot(&snapshot, |state| {
                    state.audio_capture_running = true;
                    state.audio_input_device = device_name.clone();
                    state.last_server_info = Some(format!(
                        "audio capture started on {} (speaker: {})",
                        device_name.as_deref().unwrap_or("<unknown>"),
                        active_output_preference
                            .as_deref()
                            .unwrap_or("default")
                    ));
                });
            }

            let mut server_task = tokio::spawn(async move { server.run().await });
            let mut ticker = tokio::time::interval(Duration::from_millis(DEFAULT_SEND_INTERVAL_MS));

            loop {
                tokio::select! {
                    server_result = &mut server_task => {
                        match server_result {
                            Ok(Ok(())) => tracing::info!("ws server task ended normally"),
                            Ok(Err(err)) => {
                                tracing::error!("ws server task failed: {err:?}");
                                update_snapshot(&snapshot, |state| {
                                    state.last_server_info = Some(format!("ws server error: {err}"));
                                });
                            }
                            Err(join_err) => {
                                tracing::error!("ws server task join error: {join_err}");
                                update_snapshot(&snapshot, |state| {
                                    state.last_server_info = Some(format!("ws server join error: {join_err}"));
                                });
                            }
                        }
                        break;
                    }
                    maybe_event = event_rx.recv() => {
                        match maybe_event {
                            Some(DglabWsServerEvent::Connected { app_id, requested_path, peer_addr }) => {
                                tracing::info!("app connected: app_id={app_id}, path={requested_path}, peer={peer_addr}");
                                app_bound = false;
                                update_snapshot(&snapshot, |state| {
                                    state.app_connected = true;
                                    state.app_bound = false;
                                    state.app_id = Some(app_id.clone());
                                    state.output_strengths = [0; 2];
                                    state.last_server_info = Some(format!("app connected: {peer_addr}"));
                                });
                            }
                            Some(DglabWsServerEvent::Bound { app_id }) => {
                                tracing::info!("app bound success: app_id={app_id}");
                                app_bound = true;
                                update_snapshot(&snapshot, |state| {
                                    state.app_connected = true;
                                    state.app_bound = true;
                                    state.app_id = Some(app_id.clone());
                                    state.last_server_info = Some("app bound (200)".to_owned());
                                });
                            }
                            Some(DglabWsServerEvent::AppMessage { app_id, message }) => {
                                tracing::debug!("app -> program ({app_id}): {message}");
                                update_snapshot(&snapshot, |state| {
                                    state.last_app_message = Some(message.clone());
                                    if let Some(report) = parse_strength_report(&message) {
                                        latest_soft_limits = [report.a_soft_limit, report.b_soft_limit];
                                        state.latest_strength = Some(report);
                                        state.last_server_info = Some(format!(
                                            "strength sync A:{} B:{} softA:{} softB:{}",
                                            report.a_strength,
                                            report.b_strength,
                                            report.a_soft_limit,
                                            report.b_soft_limit
                                        ));
                                    } else if message.trim().to_ascii_lowercase().starts_with("strength-") {
                                        state.last_server_info = Some(format!(
                                            "received non-standard strength report: {}",
                                            message.trim()
                                        ));
                                    }
                                });
                            }
                            Some(DglabWsServerEvent::Disconnected { app_id }) => {
                                tracing::info!("app disconnected: app_id={app_id}");
                                app_bound = false;
                                channel_active = [false; 2];
                                last_strength = [0; 2];
                                smoothed_strength = [0; 2];
                                update_snapshot(&snapshot, |state| {
                                    state.app_connected = false;
                                    state.app_bound = false;
                                    state.app_id = None;
                                    state.output_strengths = [0; 2];
                                    state.last_server_info = Some("app disconnected".to_owned());
                                });
                            }
                            None => {
                                tracing::warn!("server event channel closed");
                                break;
                            }
                        }
                    }
                    maybe_bands = band_rx.recv() => {
                        if let Some(bands) = maybe_bands {
                            latest_bands = bands;
                            for idx in 0..BAND_COUNT {
                                pending_peak_bands[idx] = pending_peak_bands[idx].max(bands[idx].clamp(0.0, 1.0));
                            }
                            update_snapshot(&snapshot, |state| {
                                state.latest_band_values = bands;
                            });
                        } else {
                            tracing::warn!("audio band channel closed");
                            update_snapshot(&snapshot, |state| {
                                state.audio_capture_running = false;
                                state.last_server_info = Some("audio band stream closed".to_owned());
                            });
                        }
                    }
                    _ = ticker.tick() => {
                        let local_settings = settings.lock().map(|s| s.clone()).unwrap_or_default();

                        if local_settings.preferred_output_device_name != active_output_preference {
                            let requested_output = local_settings.preferred_output_device_name.clone();
                            let _ = capture.stop();
                            capture = LoopbackCapture::new(LoopbackCaptureConfig {
                                preferred_output_device_name: requested_output.clone(),
                                ..LoopbackCaptureConfig::default()
                            });
                            if let Err(err) = capture.start(band_tx.clone()) {
                                tracing::warn!("audio capture switch failed: {err}");
                                update_snapshot(&snapshot, |state| {
                                    state.audio_capture_running = false;
                                    state.audio_input_device = None;
                                    state.last_server_info = Some(format!("audio capture switch failed: {err}"));
                                });
                            } else {
                                let device_name = capture.selected_device_name().map(str::to_owned);
                                latest_bands = [0.0; BAND_COUNT];
                                pending_peak_bands = [0.0; BAND_COUNT];
                                update_snapshot(&snapshot, |state| {
                                    state.audio_capture_running = true;
                                    state.audio_input_device = device_name.clone();
                                    state.latest_band_values = [0.0; BAND_COUNT];
                                    state.last_server_info = Some(format!(
                                        "audio capture switched to {} (speaker: {})",
                                        device_name.as_deref().unwrap_or("<unknown>"),
                                        requested_output.as_deref().unwrap_or("default")
                                    ));
                                });
                            }
                            active_output_preference = requested_output;
                        }

                        if !app_bound {
                            continue;
                        }

                        let sampled_bands =
                            merge_bands_with_pending_peaks(latest_bands, &mut pending_peak_bands);
                        let outputs = compute_band_outputs(
                            sampled_bands,
                            local_settings.band_routing,
                            local_settings.strength_ranges,
                        );
                        let mut channel_strengths = aggregate_channel_strengths(outputs);
                        if local_settings.respect_app_soft_limit {
                            channel_strengths[0] = channel_strengths[0].min(latest_soft_limits[0]);
                            channel_strengths[1] = channel_strengths[1].min(latest_soft_limits[1]);
                        }
                        let smooth_factor = local_settings.smooth_strength_factor.clamp(0.0, 1.0);
                        for idx in 0..2 {
                            let target = channel_strengths[idx];
                            smoothed_strength[idx] = if local_settings.smooth_strength_enabled {
                                smooth_strength_step(smoothed_strength[idx], target, smooth_factor)
                            } else {
                                target
                            };
                            channel_strengths[idx] = smoothed_strength[idx];
                        }

                        for (idx, channel) in [DglabChannel::A, DglabChannel::B].into_iter().enumerate() {
                            let strength = channel_strengths[idx];
                            if strength > 0 {
                                if strength != last_strength[idx] {
                                    let msg = build_strength_message(channel, StrengthControlMode::SetValue, strength);
                                    if let Err(err) = control_for_worker.send_app_message(msg) {
                                        tracing::warn!("auto strength send failed: {err}");
                                        update_snapshot(&snapshot, |state| {
                                            state.last_server_info = Some(format!("auto strength send failed: {err}"));
                                        });
                                    } else {
                                        last_strength[idx] = strength;
                                    }
                                }

                                let configured_max = local_settings.strength_ranges[idx].normalized().max;
                                let mapping_max = if local_settings.respect_app_soft_limit {
                                    configured_max.min(latest_soft_limits[idx])
                                } else {
                                    configured_max
                                };
                                let pulse_items = build_pulse_items_for_strength(
                                    strength,
                                    mapping_max,
                                    local_settings.pulse_items_per_message.max(1).min(8),
                                    local_settings.auto_pulse_mode,
                                    local_settings.waveform_contrast,
                                );
                                match build_pulse_message_from_items(channel, &pulse_items) {
                                    Ok(pulse_msg) => {
                                        if let Err(err) = control_for_worker.send_app_message(pulse_msg) {
                                            tracing::warn!("auto pulse send failed: {err}");
                                            update_snapshot(&snapshot, |state| {
                                                state.last_server_info = Some(format!("auto pulse send failed: {err}"));
                                            });
                                        } else {
                                            channel_active[idx] = true;
                                        }
                                    }
                                    Err(err) => {
                                        tracing::warn!("auto pulse build failed: {err}");
                                        update_snapshot(&snapshot, |state| {
                                            state.last_server_info = Some(format!("auto pulse build failed: {err}"));
                                        });
                                    }
                                }
                            } else if channel_active[idx] {
                                let clear = build_clear_message(channel);
                                let zero = build_strength_message(channel, StrengthControlMode::SetValue, 0);
                                if let Err(err) = control_for_worker.send_app_message(clear) {
                                    tracing::warn!("auto clear send failed: {err}");
                                }
                                if let Err(err) = control_for_worker.send_app_message(zero) {
                                    tracing::warn!("auto strength zero send failed: {err}");
                                }
                                channel_active[idx] = false;
                                last_strength[idx] = 0;
                            }
                        }

                        update_snapshot(&snapshot, |state| {
                            state.output_strengths = channel_strengths;
                        });
                    }
                }
            }

            server_task.abort();
            let _ = capture.stop();
            update_snapshot(&snapshot, |state| {
                state.app_connected = false;
                state.app_bound = false;
                state.app_id = None;
                state.output_strengths = [0; 2];
                state.audio_capture_running = false;
                state.audio_input_device = None;
                state.last_server_info = Some("pipeline worker stopped".to_owned());
            });
            tracing::info!("pipeline worker stop");
        }));

        Ok(())
    }

    pub fn restart(&mut self, ws_url: &str) -> Result<()> {
        self.stop();
        self.start(ws_url)
    }

    pub fn send_app_message(&self, message: impl Into<String>) -> Result<()> {
        if !self.is_running() {
            return Err(anyhow!("ws server is not running"));
        }

        let message = message.into();
        let status_rx = self
            .server_status_rx
            .as_ref()
            .ok_or_else(|| anyhow!("ws server status channel unavailable"))?;
        let status = status_rx.borrow().clone();
        let app_id = status
            .app_id
            .clone()
            .ok_or_else(|| anyhow!("app is not connected yet"))?;

        if !status.app_bound {
            return Err(anyhow!(
                "app not bound yet. scan QR and wait for bind=200 first"
            ));
        }

        let packet = SocketPacket::msg(status.controller_id, app_id, message.clone());
        let packet_text = serde_json::to_string(&packet)?;
        let packet_len = packet_text.chars().count();
        if packet_len > MAX_JSON_CHARS {
            return Err(anyhow!(
                "message too long after JSON wrapping: {packet_len} > {MAX_JSON_CHARS}"
            ));
        }

        match &self.server_control {
            Some(control) => control.send_app_message(message),
            None => Err(anyhow!("ws server command channel unavailable")),
        }
    }

    pub fn stop(&mut self) {
        self.server_control = None;
        self.server_status_rx = None;
        if let Some(handle) = self.worker.take() {
            handle.abort();
            tracing::info!("pipeline worker stopped");
        }

        self.set_snapshot(|snapshot| {
            snapshot.app_connected = false;
            snapshot.app_bound = false;
            snapshot.app_id = None;
            snapshot.output_strengths = [0; 2];
            snapshot.audio_capture_running = false;
            snapshot.audio_input_device = None;
            snapshot.last_server_info = Some("ws server stopped".to_owned());
        });
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        self.snapshot
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    fn set_snapshot(&self, mut updater: impl FnMut(&mut EngineSnapshot)) {
        if let Ok(mut state) = self.snapshot.lock() {
            updater(&mut state);
        }
    }
}

impl Drop for PipelineEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn update_snapshot(
    snapshot: &Arc<Mutex<EngineSnapshot>>,
    mut updater: impl FnMut(&mut EngineSnapshot),
) {
    if let Ok(mut state) = snapshot.lock() {
        updater(&mut state);
    }
}

fn merge_bands_with_pending_peaks(
    latest: [f32; BAND_COUNT],
    pending_peaks: &mut [f32; BAND_COUNT],
) -> [f32; BAND_COUNT] {
    let mut merged = [0.0_f32; BAND_COUNT];
    for idx in 0..BAND_COUNT {
        merged[idx] = latest[idx].max(pending_peaks[idx]).clamp(0.0, 1.0);
        pending_peaks[idx] = 0.0;
    }
    merged
}

fn build_pulse_items_for_strength(
    strength: u16,
    mapping_max_strength: u16,
    count: usize,
    mode: AutoPulseMode,
    waveform_contrast: f32,
) -> Vec<String> {
    const CONTINUOUS_FREQ_HEX: &str = "0A0A0A0A";
    const MAX_WAVE_STRENGTH: u8 = 100;

    let wave_strength_hex = |wave_strength: u8| format!("{wave_strength:02X}").repeat(4);

    let item = match mode {
        AutoPulseMode::ByStrength => {
            let mapping_max_strength = mapping_max_strength.clamp(1, 200);
            let contrast = waveform_contrast.clamp(1.0, 4.0);
            let normalized =
                (strength.min(mapping_max_strength) as f32 / mapping_max_strength as f32)
                    .clamp(0.0, 1.0);
            let boosted = ((normalized - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
            let wave_strength = if strength == 0 {
                0
            } else {
                ((boosted * MAX_WAVE_STRENGTH as f32).round() as u8).clamp(1, MAX_WAVE_STRENGTH)
            };
            format!("{CONTINUOUS_FREQ_HEX}{}", wave_strength_hex(wave_strength))
        }
        AutoPulseMode::AlwaysMax => format!(
            "{CONTINUOUS_FREQ_HEX}{}",
            wave_strength_hex(MAX_WAVE_STRENGTH)
        ),
    };
    vec![item; count.max(1)]
}

fn smooth_strength_step(current: u16, target: u16, smoothness: f32) -> u16 {
    let smoothness = smoothness.clamp(0.0, 1.0);
    if smoothness <= 0.0 {
        return target;
    }

    let response = (1.0 - smoothness).clamp(0.0, 1.0);
    let max_step = ((response.powf(2.2) * 200.0).round() as u16).clamp(1, 200);

    if current < target {
        current.saturating_add(max_step).min(target)
    } else if current > target {
        current.saturating_sub(max_step).max(target)
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_pulse_items_for_strength, merge_bands_with_pending_peaks, smooth_strength_step,
    };
    use crate::domain::types::AutoPulseMode;

    #[test]
    fn builds_strength_based_pulse_items() {
        let items = build_pulse_items_for_strength(100, 200, 4, AutoPulseMode::ByStrength, 1.0);
        assert_eq!(
            items,
            vec![
                "0A0A0A0A32323232".to_owned(),
                "0A0A0A0A32323232".to_owned(),
                "0A0A0A0A32323232".to_owned(),
                "0A0A0A0A32323232".to_owned(),
            ]
        );
    }

    #[test]
    fn builds_always_max_pulse_items() {
        let items = build_pulse_items_for_strength(1, 200, 3, AutoPulseMode::AlwaysMax, 1.0);
        assert_eq!(
            items,
            vec![
                "0A0A0A0A64646464".to_owned(),
                "0A0A0A0A64646464".to_owned(),
                "0A0A0A0A64646464".to_owned(),
            ]
        );
    }

    #[test]
    fn strength_based_pulse_never_contains_gap_pattern_when_strength_is_positive() {
        let items = build_pulse_items_for_strength(1, 200, 1, AutoPulseMode::ByStrength, 1.0);
        assert_eq!(items[0], "0A0A0A0A01010101");
    }

    #[test]
    fn strength_based_pulse_uses_valid_v3_ranges() {
        let zero = build_pulse_items_for_strength(0, 200, 1, AutoPulseMode::ByStrength, 1.0);
        let max = build_pulse_items_for_strength(200, 200, 1, AutoPulseMode::ByStrength, 1.0);
        assert_eq!(zero[0], "0A0A0A0A00000000");
        assert_eq!(max[0], "0A0A0A0A64646464");
    }

    #[test]
    fn strength_based_pulse_reaches_max_at_mapping_cap() {
        let items = build_pulse_items_for_strength(80, 80, 1, AutoPulseMode::ByStrength, 1.0);
        assert_eq!(items[0], "0A0A0A0A64646464");
    }

    #[test]
    fn strength_based_pulse_clamps_to_mapping_cap() {
        let items = build_pulse_items_for_strength(160, 80, 1, AutoPulseMode::ByStrength, 1.0);
        assert_eq!(items[0], "0A0A0A0A64646464");
    }

    #[test]
    fn waveform_contrast_boosts_dynamic_range() {
        let linear = build_pulse_items_for_strength(120, 200, 1, AutoPulseMode::ByStrength, 1.0);
        let boosted = build_pulse_items_for_strength(120, 200, 1, AutoPulseMode::ByStrength, 1.8);
        assert_eq!(linear[0], "0A0A0A0A3C3C3C3C");
        assert_eq!(boosted[0], "0A0A0A0A44444444");
    }

    #[test]
    fn merges_pending_peaks_and_clears_cache() {
        let latest = [0.2, 0.4, 0.1, 0.3];
        let mut pending = [0.5, 0.1, 0.7, 0.0];
        let merged = merge_bands_with_pending_peaks(latest, &mut pending);
        assert_eq!(merged, [0.5, 0.4, 0.7, 0.3]);
        assert_eq!(pending, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn smooth_step_uses_target_when_smoothing_is_zero() {
        assert_eq!(smooth_strength_step(20, 100, 0.0), 100);
    }

    #[test]
    fn smooth_step_moves_with_rate_limit_and_can_reach_target_exactly() {
        assert_eq!(smooth_strength_step(20, 100, 0.70), 34);
        assert_eq!(smooth_strength_step(92, 100, 0.70), 100);
        assert_eq!(smooth_strength_step(100, 20, 0.70), 86);
    }

    #[test]
    fn smooth_step_with_max_smoothing_moves_by_one_step() {
        assert_eq!(smooth_strength_step(20, 100, 1.0), 21);
        assert_eq!(smooth_strength_step(100, 20, 1.0), 99);
    }
}
