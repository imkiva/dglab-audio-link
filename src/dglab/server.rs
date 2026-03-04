use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
    time::MissedTickBehavior,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};
use uuid::Uuid;

use crate::dglab::protocol::{
    CODE_BIND_TARGET_NOT_FOUND, CODE_INVALID_JSON, CODE_MESSAGE_TOO_LONG, CODE_NOT_BOUND, CODE_OK,
    CODE_QR_CLIENT_ID_INVALID, MAX_JSON_CHARS, MESSAGE_DGLAB, PacketType, SocketPacket,
};

#[derive(Debug, Clone)]
pub struct DglabWsServerConfig {
    pub bind_addr: String,
    pub controller_id: String,
    pub max_json_chars: usize,
}

impl DglabWsServerConfig {
    pub fn new(bind_addr: impl Into<String>, controller_id: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            controller_id: controller_id.into(),
            max_json_chars: MAX_JSON_CHARS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DglabWsServerStatus {
    pub listening: bool,
    pub bind_addr: String,
    pub controller_id: String,
    pub app_id: Option<String>,
    pub app_bound: bool,
    pub last_info: String,
}

impl DglabWsServerStatus {
    fn new(config: &DglabWsServerConfig) -> Self {
        Self {
            listening: false,
            bind_addr: config.bind_addr.clone(),
            controller_id: config.controller_id.clone(),
            app_id: None,
            app_bound: false,
            last_info: "init".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum DglabWsServerEvent {
    Connected {
        app_id: String,
        requested_path: String,
        peer_addr: SocketAddr,
    },
    Bound {
        app_id: String,
    },
    AppMessage {
        app_id: String,
        message: String,
    },
    Disconnected {
        app_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct DglabWsServerControl {
    command_tx: mpsc::UnboundedSender<ServerCommand>,
}

impl DglabWsServerControl {
    pub fn send_app_message(&self, message: impl Into<String>) -> Result<()> {
        self.command_tx
            .send(ServerCommand::SendAppMessage(message.into()))
            .map_err(|_| anyhow::anyhow!("websocket server command channel is closed"))
    }
}

#[derive(Debug)]
enum ServerCommand {
    SendAppMessage(String),
}

#[derive(Debug)]
pub struct DglabWsServer {
    pub config: DglabWsServerConfig,
    command_tx: mpsc::UnboundedSender<ServerCommand>,
    command_rx: mpsc::UnboundedReceiver<ServerCommand>,
    status_tx: watch::Sender<DglabWsServerStatus>,
    event_tx: mpsc::UnboundedSender<DglabWsServerEvent>,
}

impl DglabWsServer {
    pub fn new(
        config: DglabWsServerConfig,
        event_tx: mpsc::UnboundedSender<DglabWsServerEvent>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (status_tx, _status_rx) = watch::channel(DglabWsServerStatus::new(&config));

        Self {
            config,
            command_tx,
            command_rx,
            status_tx,
            event_tx,
        }
    }

    pub fn control(&self) -> DglabWsServerControl {
        DglabWsServerControl {
            command_tx: self.command_tx.clone(),
        }
    }

    pub fn subscribe_status(&self) -> watch::Receiver<DglabWsServerStatus> {
        self.status_tx.subscribe()
    }

    pub async fn run(mut self) -> Result<()> {
        let listener = TcpListener::bind(&self.config.bind_addr)
            .await
            .with_context(|| {
                format!(
                    "failed to bind websocket server at {}",
                    self.config.bind_addr
                )
            })?;

        self.update_status(|status| {
            status.listening = true;
            status.last_info = "listening".to_owned();
        });

        tracing::info!(
            "dglab websocket server listening on {} (controller_id={})",
            self.config.bind_addr,
            self.config.controller_id
        );

        loop {
            let (stream, peer_addr) = listener.accept().await?;
            if let Err(err) = self.serve_connection(stream, peer_addr).await {
                tracing::error!("connection handling error: {err:?}");
                self.update_status(|status| {
                    status.last_info = format!("connection error: {err}");
                });
            }
        }
    }

    fn update_status(&self, mut f: impl FnMut(&mut DglabWsServerStatus)) {
        let mut status = self.status_tx.borrow().clone();
        f(&mut status);
        let _ = self.status_tx.send(status);
    }

    async fn serve_connection(&mut self, stream: TcpStream, peer_addr: SocketAddr) -> Result<()> {
        let requested_path = Arc::new(Mutex::new(String::new()));
        let requested_path_for_cb = Arc::clone(&requested_path);

        let ws_stream = accept_hdr_async(stream, move |req: &Request, response: Response| {
            if let Ok(mut slot) = requested_path_for_cb.lock() {
                *slot = req.uri().path().to_owned();
            }
            Ok(response)
        })
        .await?;

        let requested_path = requested_path.lock().map(|v| v.clone()).unwrap_or_default();
        let requested_session_id = requested_path.trim_matches('/').to_owned();
        let app_id = Uuid::new_v4().to_string();

        let _ = self.event_tx.send(DglabWsServerEvent::Connected {
            app_id: app_id.clone(),
            requested_path: requested_path.clone(),
            peer_addr,
        });

        self.update_status(|status| {
            status.app_id = Some(app_id.clone());
            status.app_bound = false;
            status.last_info = format!("connected from {peer_addr}");
        });

        let (mut ws_write, mut ws_read) = ws_stream.split();

        Self::send_packet(
            &mut ws_write,
            &SocketPacket::bind_assign_current_id(app_id.clone()),
        )
        .await?;

        if requested_session_id != self.config.controller_id {
            tracing::warn!(
                "invalid session id in path (path={requested_path}, expected={})",
                self.config.controller_id
            );
            Self::send_packet(
                &mut ws_write,
                &SocketPacket::bind_result(
                    self.config.controller_id.clone(),
                    app_id.clone(),
                    CODE_QR_CLIENT_ID_INVALID,
                ),
            )
            .await?;
            let _ = ws_write
                .send(Message::Close(None))
                .await
                .map_err(|err| tracing::warn!("close on invalid session failed: {err}"));
            self.update_status(|status| {
                status.app_id = None;
                status.app_bound = false;
                status.last_info = "invalid session id (210)".to_owned();
            });
            return Ok(());
        }

        let mut bound = false;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(60));
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let packet = SocketPacket::heartbeat(
                        app_id.clone(),
                        self.config.controller_id.clone(),
                        CODE_OK,
                    );
                    if let Err(err) = Self::send_packet(&mut ws_write, &packet).await {
                        tracing::warn!("heartbeat send failed: {err}");
                        break;
                    }
                }
                command = self.command_rx.recv() => {
                    match command {
                        Some(ServerCommand::SendAppMessage(message)) => {
                            if !bound {
                                tracing::warn!("drop outbound message because app is not bound");
                                continue;
                            }

                            let packet = SocketPacket::msg(
                                self.config.controller_id.clone(),
                                app_id.clone(),
                                message,
                            );
                            if let Err(err) = Self::send_packet(&mut ws_write, &packet).await {
                                tracing::warn!("send outbound message failed: {err}");
                                break;
                            }
                        }
                        None => break,
                    }
                }
                next_msg = ws_read.next() => {
                    let next_msg = match next_msg {
                        Some(Ok(message)) => message,
                        Some(Err(err)) => {
                            tracing::warn!("read frame error: {err}");
                            break;
                        }
                        None => break,
                    };

                    match next_msg {
                        Message::Text(text) => {
                            let should_keep = self.handle_text_frame(&mut ws_write, &app_id, &text, &mut bound).await?;
                            if !should_keep {
                                break;
                            }
                        }
                        Message::Ping(payload) => {
                            ws_write.send(Message::Pong(payload)).await?;
                        }
                        Message::Close(_) => {
                            break;
                        }
                        Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
                    }
                }
            }
        }

        let _ = self.event_tx.send(DglabWsServerEvent::Disconnected {
            app_id: app_id.clone(),
        });
        self.update_status(|status| {
            status.app_id = None;
            status.app_bound = false;
            status.last_info = "disconnected".to_owned();
        });
        tracing::info!("app disconnected: {app_id}");

        Ok(())
    }

    async fn handle_text_frame(
        &mut self,
        ws_write: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<TcpStream>,
            Message,
        >,
        app_id: &str,
        text: &str,
        bound: &mut bool,
    ) -> Result<bool> {
        if text.chars().count() > self.config.max_json_chars {
            Self::send_packet(
                ws_write,
                &SocketPacket::error(
                    app_id.to_owned(),
                    self.config.controller_id.clone(),
                    CODE_MESSAGE_TOO_LONG,
                ),
            )
            .await?;
            return Ok(true);
        }

        let packet: SocketPacket = match serde_json::from_str(text) {
            Ok(packet) => packet,
            Err(_) => {
                Self::send_packet(
                    ws_write,
                    &SocketPacket::error(
                        app_id.to_owned(),
                        self.config.controller_id.clone(),
                        CODE_INVALID_JSON,
                    ),
                )
                .await?;
                return Ok(true);
            }
        };

        if !packet.has_required_non_empty_values() {
            Self::send_packet(
                ws_write,
                &SocketPacket::error(
                    app_id.to_owned(),
                    self.config.controller_id.clone(),
                    CODE_INVALID_JSON,
                ),
            )
            .await?;
            return Ok(true);
        }

        match packet.kind() {
            Some(PacketType::Bind) => {
                if packet.message != MESSAGE_DGLAB {
                    Self::send_packet(
                        ws_write,
                        &SocketPacket::bind_result(
                            self.config.controller_id.clone(),
                            app_id.to_owned(),
                            CODE_QR_CLIENT_ID_INVALID,
                        ),
                    )
                    .await?;
                    return Ok(true);
                }

                if !self.is_controller_app_pair(&packet.client_id, &packet.target_id, app_id) {
                    tracing::warn!(
                        "bind pair mismatch: clientId={}, targetId={}, expected controllerId={} with appId={}",
                        packet.client_id,
                        packet.target_id,
                        self.config.controller_id,
                        app_id
                    );
                    Self::send_packet(
                        ws_write,
                        &SocketPacket::bind_result(
                            self.config.controller_id.clone(),
                            app_id.to_owned(),
                            CODE_BIND_TARGET_NOT_FOUND,
                        ),
                    )
                    .await?;
                    return Ok(true);
                }

                *bound = true;
                self.update_status(|status| {
                    status.app_bound = true;
                    status.last_info = "bound (200)".to_owned();
                });
                let _ = self.event_tx.send(DglabWsServerEvent::Bound {
                    app_id: app_id.to_owned(),
                });

                Self::send_packet(
                    ws_write,
                    &SocketPacket::bind_result(
                        self.config.controller_id.clone(),
                        app_id.to_owned(),
                        CODE_OK,
                    ),
                )
                .await?;
                Ok(true)
            }
            Some(PacketType::Heartbeat) => {
                Self::send_packet(
                    ws_write,
                    &SocketPacket::heartbeat(
                        app_id.to_owned(),
                        self.config.controller_id.clone(),
                        CODE_OK,
                    ),
                )
                .await?;
                Ok(true)
            }
            Some(PacketType::Msg) => {
                if !*bound {
                    tracing::warn!("received msg before bind=200, reject with 402");
                    Self::send_packet(
                        ws_write,
                        &SocketPacket::bind_result(
                            self.config.controller_id.clone(),
                            app_id.to_owned(),
                            CODE_NOT_BOUND,
                        ),
                    )
                    .await?;
                    return Ok(true);
                }

                if !self.is_controller_app_pair(&packet.client_id, &packet.target_id, app_id) {
                    tracing::warn!(
                        "msg pair mismatch: clientId={}, targetId={}, expected controllerId={} with appId={}",
                        packet.client_id,
                        packet.target_id,
                        self.config.controller_id,
                        app_id
                    );
                    Self::send_packet(
                        ws_write,
                        &SocketPacket::bind_result(
                            self.config.controller_id.clone(),
                            app_id.to_owned(),
                            CODE_NOT_BOUND,
                        ),
                    )
                    .await?;
                    return Ok(true);
                }

                let message = packet.message.clone();
                let _ = self.event_tx.send(DglabWsServerEvent::AppMessage {
                    app_id: app_id.to_owned(),
                    message: message.clone(),
                });
                tracing::debug!("app message: {message}");
                Ok(true)
            }
            Some(PacketType::Break) => {
                tracing::info!("app sent break, closing connection");
                Ok(false)
            }
            Some(PacketType::Error) => {
                tracing::warn!("app sent error packet: {:?}", packet);
                Ok(true)
            }
            None => {
                Self::send_packet(
                    ws_write,
                    &SocketPacket::error(
                        app_id.to_owned(),
                        self.config.controller_id.clone(),
                        CODE_INVALID_JSON,
                    ),
                )
                .await?;
                Ok(true)
            }
        }
    }

    async fn send_packet(
        ws_write: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<TcpStream>,
            Message,
        >,
        packet: &SocketPacket,
    ) -> Result<()> {
        let text = serde_json::to_string(packet)?;
        ws_write.send(Message::Text(text.into())).await?;
        Ok(())
    }

    fn is_controller_app_pair(&self, client_id: &str, target_id: &str, app_id: &str) -> bool {
        (client_id == self.config.controller_id && target_id == app_id)
            || (client_id == app_id && target_id == self.config.controller_id)
    }
}
