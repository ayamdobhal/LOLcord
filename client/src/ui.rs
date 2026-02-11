use crate::audio::{self, AudioState};
use crate::devices::{self, DeviceInfo};
use crate::hotkeys;
use crate::net::Connection;
use device_query::Keycode;
use eframe::egui;
use shared::{ClientMessage, ServerMessage};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc as std_mpsc,
    Arc, Mutex,
};
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
        user_id: u16,
        room_id: u16,
        voice_port: u16,
        server_addr: String,
        client_tx: tokio_mpsc::UnboundedSender<ClientMessage>,
        bridge_rx: std_mpsc::Receiver<ServerMessage>,
    },
    Err(String),
}

pub struct App {
    state: Screen,
    runtime: tokio::runtime::Runtime,
    connect_rx: std_mpsc::Receiver<ConnectResult>,
    connect_tx: std_mpsc::Sender<ConnectResult>,
    /// Stash selected device indices before async connect
    pending_devices: Option<(Option<usize>, Option<usize>)>,
}

enum Screen {
    Login {
        server_addr: String,
        username: String,
        room: String,
        password: String,
        error: Option<String>,
        connecting: bool,
        dispatched: bool,
        input_devices: Vec<DeviceInfo>,
        output_devices: Vec<DeviceInfo>,
        selected_input: usize,
        selected_output: usize,
    },
    Connected {
        room: String,
        username: String,
        users: Vec<String>,
        messages: Vec<ChatMsg>,
        input: String,
        client_tx: tokio_mpsc::UnboundedSender<ClientMessage>,
        bridge_rx: std_mpsc::Receiver<ServerMessage>,
        audio: Option<Arc<AudioState>>,
        _streams: Option<(cpal::Stream, cpal::Stream)>,
        /// Global hotkey thread alive flag
        hotkey_running: Option<Arc<AtomicBool>>,
        /// Current PTT key (shared with hotkey thread)
        ptt_key: Arc<Mutex<Keycode>>,
        /// Index into key_names for the UI dropdown
        ptt_key_idx: usize,
        /// Open mic vs PTT mode
        use_open_mic: bool,
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

        let input_devices = devices::list_input_devices();
        let output_devices = devices::list_output_devices();

        Self {
            state: Screen::Login {
                server_addr: format!("127.0.0.1:{}", shared::DEFAULT_PORT),
                username: String::new(),
                room: String::from("general"),
                password: String::new(),
                error: None,
                connecting: false,
                dispatched: false,
                input_devices,
                output_devices,
                selected_input: 0,
                selected_output: 0,
            },
            runtime,
            connect_rx,
            connect_tx,
            pending_devices: None,
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
                    let first = conn.server_rx.recv().await;
                    match first {
                        Some(ServerMessage::RoomState { room: r, users, user_id, room_id, voice_port }) => {
                            let (bridge_tx, bridge_rx) = std_mpsc::channel();
                            let ctx2 = ctx.clone();

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
                                user_id,
                                room_id,
                                voice_port,
                                server_addr: addr,
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
                ConnectResult::Ok { room, username, mut users, user_id, room_id, voice_port, server_addr, client_tx, bridge_rx } => {
                    if !users.contains(&username) {
                        users.push(username.clone());
                    }

                    // Get selected devices from login state (saved before transition)
                    let (in_idx, out_idx) = self.pending_devices.take().unwrap_or((None, None));

                    // Start audio engine
                    let (audio, streams) = match audio::start_audio(in_idx, out_idx) {
                        Ok((state, input_stream, output_stream)) => {
                            let audio_clone = state.clone();
                            let voice_addr_str = format!(
                                "{}:{}",
                                server_addr.split(':').next().unwrap_or("127.0.0.1"),
                                voice_port
                            );
                            self.runtime.spawn(async move {
                                if let Ok(addr) = voice_addr_str.parse() {
                                    crate::voice::run(audio_clone, addr, room_id, user_id).await;
                                }
                            });
                            (Some(state), Some((input_stream, output_stream)))
                        }
                        Err(e) => {
                            tracing::error!("audio engine failed: {e:#}, voice disabled");
                            eprintln!("AUDIO ERROR: {e:#}");
                            (None, None)
                        }
                    };

                    // Start global hotkey polling thread
                    let ptt_key = Arc::new(Mutex::new(Keycode::V));
                    let hotkey_running = Arc::new(AtomicBool::new(true));
                    if let Some(ref a) = audio {
                        hotkeys::spawn_global_hotkey_thread(
                            a.clone(),
                            ptt_key.clone(),
                            hotkey_running.clone(),
                        );
                    }

                    self.state = Screen::Connected {
                        room,
                        username,
                        users,
                        messages: Vec::new(),
                        input: String::new(),
                        client_tx,
                        bridge_rx,
                        audio,
                        _streams: streams,
                        hotkey_running: Some(hotkey_running),
                        ptt_key,
                        ptt_key_idx: 0, // "V" is index 0 in key_names()
                        use_open_mic: true, // default: open mic
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
                input_devices,
                output_devices,
                selected_input,
                selected_output,
                ..
            } => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(30.0);
                        ui.heading("voicechat");
                        ui.add_space(16.0);

                        ui.set_max_width(350.0);

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

                                ui.label("Microphone:");
                                egui::ComboBox::from_id_salt("input_device")
                                    .width(220.0)
                                    .selected_text(
                                        input_devices
                                            .get(*selected_input)
                                            .map(|d| d.name.as_str())
                                            .unwrap_or("(none)"),
                                    )
                                    .show_ui(ui, |ui| {
                                        for (i, dev) in input_devices.iter().enumerate() {
                                            ui.selectable_value(selected_input, i, &dev.name);
                                        }
                                    });
                                ui.end_row();

                                ui.label("Speaker:");
                                egui::ComboBox::from_id_salt("output_device")
                                    .width(220.0)
                                    .selected_text(
                                        output_devices
                                            .get(*selected_output)
                                            .map(|d| d.name.as_str())
                                            .unwrap_or("(none)"),
                                    )
                                    .show_ui(ui, |ui| {
                                        for (i, dev) in output_devices.iter().enumerate() {
                                            ui.selectable_value(selected_output, i, &dev.name);
                                        }
                                    });
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
                if let Screen::Login { connecting: true, dispatched: false, server_addr, username, room, password, error, selected_input, selected_output, input_devices, output_devices, .. } = &self.state {
                    if error.is_none() {
                        let addr = server_addr.clone();
                        let user = username.trim().to_string();
                        let rm = room.trim().to_string();
                        let pw = password.clone();
                        // Stash device selections
                        let in_idx = input_devices.get(*selected_input).and_then(|d| Some(d.index)).flatten();
                        let out_idx = output_devices.get(*selected_output).and_then(|d| Some(d.index)).flatten();
                        self.pending_devices = Some((in_idx, out_idx));
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
                audio,
                ptt_key,
                ptt_key_idx,
                use_open_mic,
                ..
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

                // Sync open_mic state to audio engine
                if let Some(ref audio) = audio {
                    audio.open_mic.store(*use_open_mic, Ordering::Relaxed);
                }

                // Users panel (left)
                egui::SidePanel::left("users_panel")
                    .default_width(160.0)
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

                        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                            if let Some(ref audio) = audio {
                                let is_muted = audio.muted.load(Ordering::Relaxed);
                                let is_deaf = audio.deafened.load(Ordering::Relaxed);
                                let is_ptt = audio.ptt_active.load(Ordering::Relaxed);
                                let is_open = audio.open_mic.load(Ordering::Relaxed);

                                // Transmit indicator
                                if is_open && !is_muted && !is_deaf {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(80, 220, 80),
                                        "🎙 OPEN MIC",
                                    );
                                } else if is_ptt && !is_muted && !is_deaf {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(80, 220, 80),
                                        "🎙 TRANSMITTING",
                                    );
                                }

                                ui.separator();

                                // Voice mode toggle
                                if ui
                                    .selectable_label(*use_open_mic, "🎤 Open Mic")
                                    .clicked()
                                {
                                    *use_open_mic = true;
                                }
                                if ui
                                    .selectable_label(!*use_open_mic, "📻 Push to Talk")
                                    .clicked()
                                {
                                    *use_open_mic = false;
                                }

                                // PTT key selector (only show when PTT mode)
                                if !*use_open_mic {
                                    let names = hotkeys::key_names();
                                    egui::ComboBox::from_id_salt("ptt_key")
                                        .width(120.0)
                                        .selected_text(format!(
                                            "PTT: {}",
                                            names.get(*ptt_key_idx).unwrap_or(&"?")
                                        ))
                                        .show_ui(ui, |ui| {
                                            for (i, name) in names.iter().enumerate() {
                                                if ui
                                                    .selectable_value(ptt_key_idx, i, *name)
                                                    .clicked()
                                                {
                                                    if let Some(kc) =
                                                        hotkeys::keycode_from_name(name)
                                                    {
                                                        if let Ok(mut k) = ptt_key.lock() {
                                                            *k = kc;
                                                        }
                                                    }
                                                }
                                            }
                                        });
                                }

                                ui.separator();

                                if ui.selectable_label(is_muted, "🔇 Mute").clicked() {
                                    audio.muted.store(!is_muted, Ordering::Relaxed);
                                }
                                if ui.selectable_label(is_deaf, "🔇 Deafen").clicked() {
                                    audio.deafened.store(!is_deaf, Ordering::Relaxed);
                                }
                            } else {
                                ui.colored_label(egui::Color32::YELLOW, "⚠ No audio");
                            }
                        });
                    });

                // Status bar (very bottom)
                egui::TopBottomPanel::bottom("status_bar")
                    .exact_height(24.0)
                    .show(ctx, |ui| {
                        ui.horizontal_centered(|ui| {
                            let voice_status = if let Some(ref audio) = audio {
                                if audio.deafened.load(Ordering::Relaxed) {
                                    "🔇 Deafened"
                                } else if audio.muted.load(Ordering::Relaxed) {
                                    "🔇 Muted"
                                } else if audio.open_mic.load(Ordering::Relaxed) {
                                    "🎙 Open Mic"
                                } else if audio.ptt_active.load(Ordering::Relaxed) {
                                    "🎙 PTT Active"
                                } else {
                                    "🎤 PTT Ready"
                                }
                            } else {
                                "⚠ No Audio"
                            };
                            ui.label(format!(
                                "{} {} · Room: {} · {} online",
                                voice_status,
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

                            let enter_pressed = response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let send = ui.button("→").clicked() || enter_pressed;

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
