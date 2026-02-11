use crate::net::Connection;
use eframe::egui;
use shared::{ClientMessage, ServerMessage};
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc as tokio_mpsc;

/// Chat message for display.
struct ChatMsg {
    from: String,
    text: String,
}

/// Result of a connection attempt, sent back to the UI thread.
enum ConnectResult {
    Ok {
        room: String,
        username: String,
        users: Vec<String>,
        client_tx: tokio_mpsc::UnboundedSender<ClientMessage>,
        bridge_rx: std_mpsc::Receiver<ServerMessage>,
    },
    Err(String),
}

pub struct App {
    state: Screen,
    runtime: tokio::runtime::Runtime,
    /// Receives connection results from async tasks
    connect_rx: std_mpsc::Receiver<ConnectResult>,
    connect_tx: std_mpsc::Sender<ConnectResult>,
}

enum Screen {
    Login {
        server_addr: String,
        username: String,
        room: String,
        password: String,
        error: Option<String>,
        connecting: bool,
        /// Set to true once connect has been dispatched, prevents double-fire
        dispatched: bool,
    },
    Connected {
        room: String,
        username: String,
        users: Vec<String>,
        messages: Vec<ChatMsg>,
        input: String,
        client_tx: tokio_mpsc::UnboundedSender<ClientMessage>,
        bridge_rx: std_mpsc::Receiver<ServerMessage>,
    },
}

impl App {
    pub fn new(_cc: &eframe::CreationContext) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .expect("failed to create tokio runtime");

        let (connect_tx, connect_rx) = std_mpsc::channel();

        Self {
            state: Screen::Login {
                server_addr: format!("127.0.0.1:{}", shared::DEFAULT_PORT),
                username: String::new(),
                room: String::from("general"),
                password: String::new(),
                error: None,
                connecting: false,
                dispatched: false,
            },
            runtime,
            connect_rx,
            connect_tx,
        }
    }

    fn initiate_connect(&self, ctx: &egui::Context, addr: String, username: String, room: String, password: String) {
        let join_msg = ClientMessage::Join {
            username: username.clone(),
            room: room.clone(),
            password: if password.is_empty() { None } else { Some(password) },
        };

        let result_tx = self.connect_tx.clone();
        let ctx = ctx.clone();

        self.runtime.spawn(async move {
            match Connection::connect(&addr, join_msg).await {
                Ok(mut conn) => {
                    // Read the first message — should be RoomState or Error
                    let first = conn.server_rx.recv().await;
                    match first {
                        Some(ServerMessage::RoomState { room: r, users }) => {
                            let (bridge_tx, bridge_rx) = std_mpsc::channel();
                            let ctx2 = ctx.clone();

                            // Spawn bridge: forward remaining server msgs to std channel
                            tokio::spawn(async move {
                                while let Some(msg) = conn.server_rx.recv().await {
                                    if bridge_tx.send(msg).is_err() {
                                        break;
                                    }
                                    ctx2.request_repaint();
                                }
                            });

                            let _ = result_tx.send(ConnectResult::Ok {
                                room: r,
                                username,
                                users,
                                client_tx: conn.client_tx,
                                bridge_rx,
                            });
                        }
                        Some(ServerMessage::Error { message }) => {
                            let _ = result_tx.send(ConnectResult::Err(message));
                        }
                        _ => {
                            let _ = result_tx.send(ConnectResult::Err("unexpected server response".into()));
                        }
                    }
                }
                Err(e) => {
                    let _ = result_tx.send(ConnectResult::Err(e.to_string()));
                }
            }
            ctx.request_repaint();
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check for connection results
        if let Ok(result) = self.connect_rx.try_recv() {
            match result {
                ConnectResult::Ok { room, username, users, client_tx, bridge_rx } => {
                    self.state = Screen::Connected {
                        room,
                        username,
                        users,
                        messages: Vec::new(),
                        input: String::new(),
                        client_tx,
                        bridge_rx,
                    };
                }
                ConnectResult::Err(msg) => {
                    if let Screen::Login { error, connecting, dispatched, .. } = &mut self.state {
                        *error = Some(msg);
                        *connecting = false;
                        *dispatched = false;
                    }
                }
            }
        }

        match &mut self.state {
            Screen::Login {
                server_addr,
                username,
                room,
                password,
                error,
                connecting,
                ..
            } => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.heading("voicechat");
                        ui.add_space(20.0);

                        ui.set_max_width(300.0);

                        egui::Grid::new("login_grid")
                            .num_columns(2)
                            .spacing([10.0, 8.0])
                            .show(ui, |ui| {
                                ui.label("Server:");
                                ui.text_edit_singleline(server_addr);
                                ui.end_row();

                                ui.label("Username:");
                                ui.text_edit_singleline(username);
                                ui.end_row();

                                ui.label("Room:");
                                ui.text_edit_singleline(room);
                                ui.end_row();

                                ui.label("Password:");
                                ui.add(egui::TextEdit::singleline(password).password(true));
                                ui.end_row();
                            });

                        ui.add_space(12.0);

                        if *connecting {
                            ui.spinner();
                        } else {
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if ui.button("Connect").clicked() || enter {
                                if username.trim().is_empty() {
                                    *error = Some("username required".into());
                                } else {
                                    *connecting = true;
                                    *error = None;
                                }
                            }
                        }

                        if let Some(err) = error {
                            ui.add_space(8.0);
                            ui.colored_label(egui::Color32::RED, err.as_str());
                        }
                    });
                });

                // Trigger connect outside the borrow (only once)
                if let Screen::Login { connecting: true, dispatched: false, server_addr, username, room, password, error, .. } = &self.state {
                    if error.is_none() {
                        let addr = server_addr.clone();
                        let user = username.trim().to_string();
                        let rm = room.trim().to_string();
                        let pw = password.clone();
                        self.initiate_connect(ctx, addr, user, rm, pw);
                        if let Screen::Login { dispatched, .. } = &mut self.state {
                            *dispatched = true;
                        }
                    }
                }
            }
            Screen::Connected {
                room,
                username,
                users,
                messages,
                input,
                client_tx,
                bridge_rx,
            } => {
                // Poll incoming messages
                while let Ok(msg) = bridge_rx.try_recv() {
                    match msg {
                        ServerMessage::Chat { from, text, .. } => {
                            messages.push(ChatMsg { from, text });
                        }
                        ServerMessage::UserJoined { username: u } => {
                            if !users.contains(&u) {
                                users.push(u.clone());
                            }
                            messages.push(ChatMsg {
                                from: "system".into(),
                                text: format!("{u} joined"),
                            });
                        }
                        ServerMessage::UserLeft { username: u } => {
                            users.retain(|x| x != &u);
                            messages.push(ChatMsg {
                                from: "system".into(),
                                text: format!("{u} left"),
                            });
                        }
                        _ => {}
                    }
                }

                // Users panel (left)
                egui::SidePanel::left("users_panel")
                    .default_width(150.0)
                    .show(ctx, |ui| {
                        ui.heading(format!("🔊 {room}"));
                        ui.separator();
                        for user in users.iter() {
                            let label = if user == username.as_str() {
                                format!("🎤 {user} (you)")
                            } else {
                                format!("🔊 {user}")
                            };
                            ui.label(label);
                        }
                    });

                // Status bar (very bottom)
                egui::TopBottomPanel::bottom("status_bar")
                    .exact_height(24.0)
                    .show(ctx, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.label(format!(
                                "🎤 {} · Room: {} · {} online",
                                username,
                                room,
                                users.len()
                            ));
                        });
                    });

                // Input bar (above status)
                egui::TopBottomPanel::bottom("input_panel")
                    .exact_height(32.0)
                    .show(ctx, |ui| {
                        ui.horizontal_centered(|ui| {
                            let response = ui.add_sized(
                                [ui.available_width() - 35.0, 22.0],
                                egui::TextEdit::singleline(input)
                                    .hint_text("type a message..."),
                            );

                            let send = ui.button("→").clicked()
                                || (response.has_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)));

                            if send && !input.trim().is_empty() {
                                let text = input.trim().to_string();
                                let _ = client_tx.send(ClientMessage::Chat {
                                    text: text.clone(),
                                });
                                messages.push(ChatMsg {
                                    from: username.clone(),
                                    text,
                                });
                                input.clear();
                                response.request_focus();
                            }
                        });
                    });

                // Chat area (center)
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for msg in messages.iter() {
                                if msg.from == "system" {
                                    ui.colored_label(egui::Color32::GRAY, &msg.text);
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.strong(format!("{}:", msg.from));
                                        ui.label(&msg.text);
                                    });
                                }
                            }
                        });
                });
            }
        }
    }
}
