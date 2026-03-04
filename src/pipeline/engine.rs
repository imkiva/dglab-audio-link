use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use tokio::{runtime::Runtime, sync::watch, task::JoinHandle};

use crate::dglab::{
    pairing,
    protocol::{MAX_JSON_CHARS, SocketPacket, StrengthReport, parse_strength_report},
    server::{DglabWsServer, DglabWsServerConfig, DglabWsServerControl, DglabWsServerEvent},
};

#[derive(Debug, Clone, Default)]
pub struct EngineSnapshot {
    pub app_connected: bool,
    pub app_bound: bool,
    pub app_id: Option<String>,
    pub latest_strength: Option<StrengthReport>,
    pub last_app_message: Option<String>,
    pub last_server_info: Option<String>,
}

#[derive(Debug)]
pub struct PipelineEngine {
    runtime: Arc<Runtime>,
    worker: Option<JoinHandle<()>>,
    server_control: Option<DglabWsServerControl>,
    server_status_rx: Option<watch::Receiver<crate::dglab::server::DglabWsServerStatus>>,
    snapshot: Arc<Mutex<EngineSnapshot>>,
}

impl PipelineEngine {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            worker: None,
            server_control: None,
            server_status_rx: None,
            snapshot: Arc::new(Mutex::new(EngineSnapshot::default())),
        }
    }

    pub fn is_running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
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
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<DglabWsServerEvent>();
        let server = DglabWsServer::new(
            DglabWsServerConfig::new(bind_addr.clone(), controller_id.clone()),
            event_tx,
        );
        self.server_status_rx = Some(server.subscribe_status());
        self.server_control = Some(server.control());

        self.set_snapshot(|snapshot| {
            snapshot.app_connected = false;
            snapshot.app_bound = false;
            snapshot.app_id = None;
            snapshot.latest_strength = None;
            snapshot.last_app_message = None;
            snapshot.last_server_info = Some(format!(
                "ws server starting on {bind_addr}, session={controller_id}"
            ));
        });

        let snapshot = Arc::clone(&self.snapshot);
        self.worker = Some(self.runtime.spawn(async move {
            tracing::info!(
                "dglab ws server worker start: bind={bind_addr}, controller_id={controller_id}"
            );

            let snapshot_for_events = Arc::clone(&snapshot);
            let event_task = tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    match event {
                        DglabWsServerEvent::Connected {
                            app_id,
                            requested_path,
                            peer_addr,
                        } => {
                            tracing::info!(
                                "app connected: app_id={app_id}, path={requested_path}, peer={peer_addr}"
                            );
                            update_snapshot(&snapshot_for_events, |state| {
                                state.app_connected = true;
                                state.app_bound = false;
                                state.app_id = Some(app_id.clone());
                                state.last_server_info = Some(format!("app connected: {peer_addr}"));
                            });
                        }
                        DglabWsServerEvent::Bound { app_id } => {
                            tracing::info!("app bound success: app_id={app_id}");
                            update_snapshot(&snapshot_for_events, |state| {
                                state.app_connected = true;
                                state.app_bound = true;
                                state.app_id = Some(app_id.clone());
                                state.last_server_info = Some("app bound (200)".to_owned());
                            });
                        }
                        DglabWsServerEvent::AppMessage { app_id, message } => {
                            tracing::debug!("app -> program ({app_id}): {message}");
                            update_snapshot(&snapshot_for_events, |state| {
                                state.last_app_message = Some(message.clone());
                                if let Some(report) = parse_strength_report(&message) {
                                    state.latest_strength = Some(report);
                                    state.last_server_info = Some(format!(
                                        "strength sync A:{} B:{} softA:{} softB:{}",
                                        report.a_strength,
                                        report.b_strength,
                                        report.a_soft_limit,
                                        report.b_soft_limit
                                    ));
                                }
                            });
                        }
                        DglabWsServerEvent::Disconnected { app_id } => {
                            tracing::info!("app disconnected: app_id={app_id}");
                            update_snapshot(&snapshot_for_events, |state| {
                                state.app_connected = false;
                                state.app_bound = false;
                                state.app_id = None;
                                state.last_server_info = Some("app disconnected".to_owned());
                            });
                        }
                    }
                }
            });

            if let Err(err) = server.run().await {
                tracing::error!("dglab ws server stopped with error: {err:?}");
                update_snapshot(&snapshot, |state| {
                    state.last_server_info = Some(format!("ws server error: {err}"));
                });
            }
            event_task.abort();
            tracing::info!("dglab ws server worker stop");
            update_snapshot(&snapshot, |state| {
                state.app_connected = false;
                state.app_bound = false;
                state.app_id = None;
                state.last_server_info = Some("ws server stopped".to_owned());
            });
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
