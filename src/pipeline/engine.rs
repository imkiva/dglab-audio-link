use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::{runtime::Runtime, task::JoinHandle};

use crate::dglab::{
    pairing,
    server::{DglabWsServer, DglabWsServerConfig, DglabWsServerControl, DglabWsServerEvent},
};

#[derive(Debug)]
pub struct PipelineEngine {
    runtime: Arc<Runtime>,
    worker: Option<JoinHandle<()>>,
    server_control: Option<DglabWsServerControl>,
}

impl PipelineEngine {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            worker: None,
            server_control: None,
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

        let parsed = pairing::parse_control_ws_url(ws_url)
            .ok_or_else(|| anyhow!("invalid ws url. expected ws://<host>:<port>/<session-id>"))?;
        let bind_addr = format!("0.0.0.0:{}", parsed.port);
        let controller_id = parsed.session_id;
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<DglabWsServerEvent>();
        let server = DglabWsServer::new(
            DglabWsServerConfig::new(bind_addr.clone(), controller_id.clone()),
            event_tx,
        );
        self.server_control = Some(server.control());

        self.worker = Some(self.runtime.spawn(async move {
            tracing::info!("dglab ws server worker start: bind={bind_addr}, controller_id={controller_id}");

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
                        }
                        DglabWsServerEvent::Bound { app_id } => {
                            tracing::info!("app bound success: app_id={app_id}");
                        }
                        DglabWsServerEvent::AppMessage { app_id, message } => {
                            tracing::debug!("app -> program ({app_id}): {message}");
                        }
                        DglabWsServerEvent::Disconnected { app_id } => {
                            tracing::info!("app disconnected: app_id={app_id}");
                        }
                    }
                }
            });

            if let Err(err) = server.run().await {
                tracing::error!("dglab ws server stopped with error: {err:?}");
            }
            event_task.abort();
            tracing::info!("dglab ws server worker stop");
        }));

        Ok(())
    }

    pub fn restart(&mut self, ws_url: &str) -> Result<()> {
        self.stop();
        self.start(ws_url)
    }

    pub fn send_app_message(&self, message: impl Into<String>) -> Result<()> {
        match &self.server_control {
            Some(control) => control.send_app_message(message),
            None => Err(anyhow!("ws server is not running")),
        }
    }

    pub fn stop(&mut self) {
        self.server_control = None;
        if let Some(handle) = self.worker.take() {
            handle.abort();
            tracing::info!("pipeline worker stopped");
        }
    }
}

impl Drop for PipelineEngine {
    fn drop(&mut self) {
        self.stop();
    }
}
