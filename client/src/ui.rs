use crate::audio::{self, AudioState};
use crate::devices::{self, DeviceInfo};
use crate::hotkeys::{self, PttBind};
use crate::jitter::JitterBuffer;
use crate::net::Connection;
use crate::settings::Settings;
use eframe::egui;
use shared::{ClientMessage, ServerMessage};
use std::collections::HashMap;
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
        user_ids: HashMap<String, u16>,
        password: Option<String>,
        is_reconnect: bool,
    },
    Err(String),
}

#[derive(Clone)]
struct ReconnectParams {
    server_addr: String,
    username: String,
    room: String,
    password: String,
    input_device_idx: Option<usize>,
    output_device_idx: Option<usize>,
}

#[derive(Clone, PartialEq)]
enum ReconnectState {
    Connected,
    Reconnecting {
        attempt: u32,
        next_try: std::time::Instant,
        started: std::time::Instant,
    },
    GaveUp,
}

pub struct App {
    state: Screen,
    runtime: tokio::runtime::Runtime,
    connect_rx: std_mpsc::Receiver<ConnectResult>,
    connect_tx: std_mpsc::Sender<ConnectResult>,
    /// Stash selected device indices before async connect
    pending_devices: Option<(Option<usize>, Option<usize>)>,
    /// Whether a reconnect attempt is in-flight
    reconnect_pending: bool,
    /// Deferred reconnect action (set in match, executed after)
    deferred_reconnect: Option<ReconnectParams>,
    /// Deferred go-to-login
    deferred_go_login: Option<ReconnectParams>,
    /// System tray icon (kept alive)
    _tray: Option<tray_icon::TrayIcon>,
    /// Tray command receiver
    tray_rx: Option<std_mpsc::Receiver<crate::tray::TrayCommand>>,
    /// Logo texture for login screen
    logo_texture: Option<egui::TextureHandle>,
    /// Persisted settings
    settings: Settings,
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
        /// Current PTT binding (shared with hotkey thread)
        ptt_bind: Arc<Mutex<PttBind>>,
        /// Display name of current PTT binding
        ptt_bind_name: String,
        /// Whether we're listening for a key press to rebind PTT
        listening_for_ptt: bool,
        /// Open mic vs PTT mode
        use_open_mic: bool,
        /// Per-user volume multipliers shared with voice thread
        user_volumes: Arc<Mutex<HashMap<u16, f32>>>,
        /// User ID mapping: username -> user_id (populated from voice packets)
        user_id_map: HashMap<String, u16>,
        /// Our own user_id
        our_user_id: u16,
        /// Jitter buffer (shared with voice thread) for quality stats
        jitter: Arc<Mutex<JitterBuffer>>,
        /// Cached per-user quality stats
        quality_stats: HashMap<u16, f32>,
        /// Last time quality stats were updated
        quality_update: std::time::Instant,
        /// Upload quality stats (shared with voice thread)
        upload_stats: Arc<crate::voice::UploadStats>,
        /// Cached upload loss %
        upload_loss: f32,
        /// E2E encryption key (derived from room password, if set)
        encryption_key: Option<[u8; 32]>,
        /// Original connection params for reconnect
        connect_params: Option<ReconnectParams>,
        /// Reconnection state
        reconnect_state: ReconnectState,
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

        let (tray, tray_rx) = match crate::tray::create_tray() {
            Some((t, r)) => (Some(t), Some(r)),
            None => (None, None),
        };

        // Load persisted settings
        let settings = Settings::load();

        let selected_input = settings.selected_input_device.as_ref()
            .and_then(|name| input_devices.iter().position(|d| d.name == *name))
            .unwrap_or(0);
        let selected_output = settings.selected_output_device.as_ref()
            .and_then(|name| output_devices.iter().position(|d| d.name == *name))
            .unwrap_or(0);

        Self {
            state: Screen::Login {
                server_addr: settings.server_addr.clone().unwrap_or_else(|| format!("127.0.0.1:{}", shared::DEFAULT_PORT)),
                username: settings.username.clone().unwrap_or_default(),
                room: settings.room.clone().unwrap_or_else(|| "general".into()),
                password: String::new(),
                error: None,
                connecting: false,
                dispatched: false,
                input_devices,
                output_devices,
                selected_input,
                selected_output,
            },
            runtime,
            connect_rx,
            connect_tx,
            pending_devices: None,
            _tray: tray,
            tray_rx,
            reconnect_pending: false,
            deferred_reconnect: None,
            deferred_go_login: None,
            logo_texture: None,
            settings,
        }
    }

    fn get_logo_texture(&mut self, ctx: &egui::Context) -> &egui::TextureHandle {
        self.logo_texture.get_or_insert_with(|| {
            let bytes = include_bytes!("../assets/logo.png");
            let img = image::load_from_memory(bytes).expect("failed to load logo");
            let mut rgba = img.into_rgba8();
            let (w, h) = (rgba.width() as usize, rgba.height() as usize);

            // Remove pinkish gradient background by sampling corner colors
            // and making pixels transparent if they're close to the background
            {
                // Sample the 4 corner pixels as background reference (image is RGB, alpha=255)
                let corners = [
                    (0usize, 0usize),
                    (w - 1, 0),
                    (0, h - 1),
                    (w - 1, h - 1),
                ];
                let ref_colors: Vec<[f32; 3]> = corners.iter().map(|&(x, y)| {
                    let p = rgba.get_pixel(x as u32, y as u32);
                    [p[0] as f32, p[1] as f32, p[2] as f32]
                }).collect();

                // For each pixel, compute min color distance to any corner reference
                // If close enough, make transparent (with soft edge)
                let threshold = 55.0f32; // distance below which we consider it background
                let soft_edge = 20.0f32; // fade zone

                for y in 0..h {
                    for x in 0..w {
                        let p = rgba.get_pixel(x as u32, y as u32);
                        let pc = [p[0] as f32, p[1] as f32, p[2] as f32];

                        let min_dist = ref_colors.iter().map(|rc| {
                            let dr = pc[0] - rc[0];
                            let dg = pc[1] - rc[1];
                            let db = pc[2] - rc[2];
                            (dr * dr + dg * dg + db * db).sqrt()
                        }).fold(f32::MAX, f32::min);

                        if min_dist < threshold + soft_edge {
                            let alpha = if min_dist < threshold {
                                0u8
                            } else {
                                ((min_dist - threshold) / soft_edge * 255.0) as u8
                            };
                            rgba.get_pixel_mut(x as u32, y as u32)[3] = alpha;
                        }
                    }
                }
            }

            let size = [w, h];
            let pixels = rgba.into_raw();
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
            ctx.load_texture("logo", color_image, egui::TextureOptions::LINEAR)
        })
    }

    fn initiate_connect(&self, ctx: &egui::Context, addr: String, username: String, room: String, password: String, raw_password: Option<String>, is_reconnect: bool) {
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
                        Some(ServerMessage::RoomState { room: r, users, user_id, room_id, voice_port, user_ids }) => {
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
                                user_ids,
                                password: raw_password,
                                is_reconnect,
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

impl App {
    fn save_settings_from_state(&mut self) {
        match &self.state {
            Screen::Login { server_addr, username, room, input_devices, output_devices, selected_input, selected_output, .. } => {
                self.settings.server_addr = Some(server_addr.clone());
                self.settings.username = Some(username.clone());
                self.settings.room = Some(room.clone());
                self.settings.selected_input_device = input_devices.get(*selected_input).map(|d| d.name.clone());
                self.settings.selected_output_device = output_devices.get(*selected_output).map(|d| d.name.clone());
            }
            Screen::Connected { use_open_mic, ptt_bind, audio, .. } => {
                self.settings.use_open_mic = Some(*use_open_mic);
                if let Ok(b) = ptt_bind.lock() {
                    self.settings.ptt_bind = Some(b.to_setting_string());
                }
                if let Some(ref a) = audio {
                    self.settings.noise_suppression = Some(a.noise_suppression.load(Ordering::Relaxed));
                    let ng_thresh = a.noise_gate_threshold.load(Ordering::Relaxed) as f32 / 10000.0;
                    self.settings.noise_gate_threshold = Some(ng_thresh);
                    let vad_thresh = a.vad_threshold.load(Ordering::Relaxed) as f32 / 10000.0;
                    let sens = 1.0 - (vad_thresh - 0.001) / 0.099;
                    self.settings.vad_sensitivity = Some(sens.clamp(0.0, 1.0));
                }
            }
        }
        self.settings.save();
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings_from_state();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Minimize to tray on close (if tray is available and connected)
        if self._tray.is_some() {
            if let Screen::Connected { .. } = &self.state {
                if ctx.input(|i| i.viewport().close_requested()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
            }
        }

        // Handle tray commands
        if let Some(ref tray_rx) = self.tray_rx {
            while let Ok(cmd) = tray_rx.try_recv() {
                match cmd {
                    crate::tray::TrayCommand::Show => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    crate::tray::TrayCommand::ToggleMute => {
                        if let Screen::Connected { ref audio, .. } = self.state {
                            if let Some(ref a) = audio {
                                let cur = a.muted.load(Ordering::Relaxed);
                                a.muted.store(!cur, Ordering::Relaxed);
                            }
                        }
                    }
                    crate::tray::TrayCommand::ToggleDeafen => {
                        if let Screen::Connected { ref audio, .. } = self.state {
                            if let Some(ref a) = audio {
                                let cur = a.deafened.load(Ordering::Relaxed);
                                a.deafened.store(!cur, Ordering::Relaxed);
                            }
                        }
                    }
                    crate::tray::TrayCommand::Quit => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }

        // Check for connection results
        if let Ok(result) = self.connect_rx.try_recv() {
            match result {
                ConnectResult::Ok { room, username, mut users, user_id, room_id, voice_port, server_addr, client_tx, bridge_rx, user_ids: initial_user_ids, password, is_reconnect } => {
                    if !users.contains(&username) {
                        users.push(username.clone());
                    }

                    // Derive E2E encryption key from password
                    let encryption_key = password.as_ref()
                        .filter(|p| !p.is_empty())
                        .map(|p| crate::crypto::derive_key(p));

                    // Preserve messages from old state if reconnecting
                    let old_messages = if is_reconnect {
                        if let Screen::Connected { messages, .. } = &mut self.state {
                            let mut msgs = Vec::new();
                            std::mem::swap(&mut msgs, messages);
                            msgs
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    };

                    self.reconnect_pending = false;

                    // Get selected devices from login state (saved before transition)
                    let (in_idx, out_idx) = self.pending_devices.take().unwrap_or((None, None));

                    // Create jitter buffer, volume map, and upload stats
                    let jitter = Arc::new(Mutex::new(JitterBuffer::new()));
                    let user_volumes: Arc<Mutex<HashMap<u16, f32>>> = Arc::new(Mutex::new(HashMap::new()));
                    let upload_stats = Arc::new(crate::voice::UploadStats::new());

                    // Start audio engine
                    let (audio, streams) = match audio::start_audio(in_idx, out_idx) {
                        Ok((state, input_stream, output_stream)) => {
                            let audio_clone = state.clone();
                            let jitter_clone = jitter.clone();
                            let volumes_clone = user_volumes.clone();
                            let voice_addr_str = format!(
                                "{}:{}",
                                server_addr.split(':').next().unwrap_or("127.0.0.1"),
                                voice_port
                            );
                            let upload_stats_clone = upload_stats.clone();
                            self.runtime.spawn(async move {
                                if let Ok(addr) = voice_addr_str.parse() {
                                    crate::voice::run(audio_clone, addr, room_id, user_id, jitter_clone, volumes_clone, encryption_key, upload_stats_clone).await;
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
                    let initial_bind = self.settings.ptt_bind.as_ref()
                        .and_then(|s| PttBind::from_setting_string(s))
                        .unwrap_or(PttBind::Key(device_query::Keycode::V));
                    let ptt_bind_name = initial_bind.display_name();
                    let ptt_bind = Arc::new(Mutex::new(initial_bind));
                    let hotkey_running = Arc::new(AtomicBool::new(true));
                    if let Some(ref a) = audio {
                        hotkeys::spawn_global_hotkey_thread(
                            a.clone(),
                            ptt_bind.clone(),
                            hotkey_running.clone(),
                        );
                    }

                    // Apply persisted audio settings
                    if let Some(ref a) = audio {
                        if let Some(ns) = self.settings.noise_suppression {
                            a.noise_suppression.store(ns, Ordering::Relaxed);
                        }
                        if let Some(ng) = self.settings.noise_gate_threshold {
                            a.noise_gate_threshold.store((ng * 10000.0) as u32, Ordering::Relaxed);
                        }
                        if let Some(vad) = self.settings.vad_sensitivity {
                            // sens 0..1 maps to threshold 0.001 + (1-sens)*0.099
                            let thresh = 0.001 + (1.0 - vad) * 0.099;
                            a.vad_threshold.store((thresh * 10000.0) as u32, Ordering::Relaxed);
                        }
                    }

                    // Build reconnect params
                    let rp = ReconnectParams {
                        server_addr: server_addr.clone(),
                        username: username.clone(),
                        room: room.clone(),
                        password: password.as_ref().cloned().unwrap_or_default(),
                        input_device_idx: in_idx,
                        output_device_idx: out_idx,
                    };

                    let mut initial_messages = old_messages;
                    if is_reconnect {
                        initial_messages.push(ChatMsg {
                            from: "system".into(),
                            text: "Reconnected!".into(),
                        });
                    }

                    self.state = Screen::Connected {
                        room,
                        username,
                        users,
                        messages: initial_messages,
                        input: String::new(),
                        client_tx,
                        bridge_rx,
                        audio,
                        _streams: streams,
                        hotkey_running: Some(hotkey_running),
                        ptt_bind,
                        ptt_bind_name: ptt_bind_name,
                        listening_for_ptt: false,
                        use_open_mic: self.settings.use_open_mic.unwrap_or(true),
                        user_volumes,
                        user_id_map: initial_user_ids,
                        our_user_id: user_id,
                        upload_stats,
                        upload_loss: 0.0,
                        encryption_key,
                        connect_params: Some(rp),
                        reconnect_state: ReconnectState::Connected,
                        jitter: jitter.clone(),
                        quality_stats: HashMap::new(),
                        quality_update: std::time::Instant::now(),
                    };
                }
                ConnectResult::Err(msg) => {
                    if self.reconnect_pending {
                        // Reconnect attempt failed — will retry via state machine
                        self.reconnect_pending = false;
                        tracing::warn!("reconnect failed: {msg}");
                    } else if let Screen::Login { error, connecting, dispatched, .. } = &mut self.state {
                        *error = Some(msg);
                        *connecting = false;
                        *dispatched = false;
                    }
                }
            }
        }

        // Pre-load logo texture to avoid borrow conflicts
        let logo_tid = self.get_logo_texture(ctx).id();

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
                        ui.add_space(10.0);
                        let logo_size = egui::vec2(120.0, 120.0);
                        ui.image(egui::load::SizedTexture::new(logo_tid, logo_size));
                        ui.add_space(8.0);

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

                // Trigger connect outside the borrow (only once); save settings
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
                        let raw_pw = if pw.is_empty() { None } else { Some(pw.clone()) };
                        self.save_settings_from_state();
                        self.initiate_connect(ctx, addr, user, rm, pw, raw_pw, false);
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
                ptt_bind,
                ptt_bind_name,
                listening_for_ptt,
                use_open_mic,
                user_volumes,
                user_id_map,
                our_user_id,
                upload_stats,
                upload_loss,
                encryption_key,
                reconnect_state,
                connect_params,
                jitter,
                quality_stats,
                quality_update,
                ..
            } => {
                // Poll incoming messages + detect disconnection
                let mut channel_disconnected = false;
                loop {
                    match bridge_rx.try_recv() {
                        Ok(msg) => match msg {
                            ServerMessage::Chat { from, text, .. } => {
                                let text = if let Some(ref key) = encryption_key {
                                    use base64::Engine;
                                    match base64::engine::general_purpose::STANDARD.decode(&text) {
                                        Ok(encrypted) => {
                                            match crate::crypto::decrypt(key, &encrypted) {
                                                Ok(plaintext) => String::from_utf8_lossy(&plaintext).to_string(),
                                                Err(_) => text,
                                            }
                                        }
                                        Err(_) => text,
                                    }
                                } else {
                                    text
                                };
                                messages.push(ChatMsg { from, text });
                            }
                            ServerMessage::UserJoined { username: u, user_id: uid } => {
                                if !users.contains(&u) {
                                    users.push(u.clone());
                                }
                                if let Some(uid) = uid {
                                    user_id_map.insert(u.clone(), uid);
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
                        },
                        Err(std_mpsc::TryRecvError::Empty) => break,
                        Err(std_mpsc::TryRecvError::Disconnected) => {
                            channel_disconnected = true;
                            break;
                        }
                    }
                }

                // Handle disconnection → trigger reconnect
                if channel_disconnected && *reconnect_state == ReconnectState::Connected {
                    if let Some(_params) = connect_params.as_ref() {
                        let now = std::time::Instant::now();
                        *reconnect_state = ReconnectState::Reconnecting {
                            attempt: 0,
                            next_try: now,
                            started: now,
                        };
                        messages.push(ChatMsg {
                            from: "system".into(),
                            text: "Connection lost. Reconnecting...".into(),
                        });
                    }
                }

                // Reconnect state machine — collect action to execute after match
                let mut reconnect_action: Option<ReconnectParams> = None;
                let mut should_go_login = false;
                if let ReconnectState::Reconnecting { attempt, next_try, started } = reconnect_state {
                    let now = std::time::Instant::now();
                    if now.duration_since(*started).as_secs() > 120 {
                        *reconnect_state = ReconnectState::GaveUp;
                        messages.push(ChatMsg {
                            from: "system".into(),
                            text: "Reconnection failed. Returning to login...".into(),
                        });
                        should_go_login = true;
                    } else if now >= *next_try && !self.reconnect_pending {
                        if let Some(params) = connect_params.clone() {
                            *attempt += 1;
                            let delay_secs = std::cmp::min(30, 1u64 << (*attempt).min(5));
                            *next_try = now + std::time::Duration::from_secs(delay_secs);
                            reconnect_action = Some(params);
                        }
                    }
                }
                if should_go_login {
                    self.deferred_go_login = connect_params.clone();
                }
                if let Some(params) = reconnect_action {
                    self.deferred_reconnect = Some(params);
                }

                // Sync open_mic state to audio engine
                if let Some(ref audio) = audio {
                    audio.open_mic.store(*use_open_mic, Ordering::Relaxed);
                }

                // Update connection quality stats every 3 seconds
                if quality_update.elapsed() >= std::time::Duration::from_secs(3) {
                    if let Ok(mut jb) = jitter.lock() {
                        *quality_stats = jb.get_quality_stats();
                        jb.reset_quality_stats();
                    }
                    *upload_loss = upload_stats.take_loss_percent();
                    *quality_update = std::time::Instant::now();
                }

                // Request repaint for live indicators (VAD, PTT)
                ctx.request_repaint_after(std::time::Duration::from_millis(100));

                // Users panel (left)
                egui::SidePanel::left("users_panel")
                    .default_width(160.0)
                    .show(ctx, |ui| {
                        ui.heading(format!("🔊 {room}"));
                        ui.separator();
                        for user in users.iter() {
                            if user == username.as_str() {
                                ui.label(format!("🎤 {user} (you)"));
                            } else {
                                let quality_icon = user_id_map.get(user)
                                    .and_then(|uid| quality_stats.get(uid))
                                    .map(|&loss| {
                                        if loss < 2.0 { "🟢" }
                                        else if loss < 10.0 { "🟡" }
                                        else { "🔴" }
                                    })
                                    .unwrap_or("🔊");
                                ui.label(format!("{quality_icon} {user}"));
                                // Per-user volume slider
                                if let Some(&uid) = user_id_map.get(user) {
                                    let mut vol = user_volumes
                                        .lock()
                                        .ok()
                                        .and_then(|v| v.get(&uid).copied())
                                        .unwrap_or(1.0);
                                    let vol_pct = (vol * 100.0) as u32;
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().slider_width = 80.0;
                                        if ui.add(egui::Slider::new(&mut vol, 0.0..=2.0)
                                            .text(format!("{vol_pct}%"))
                                            .show_value(false)
                                        ).changed() {
                                            if let Ok(mut vols) = user_volumes.lock() {
                                                vols.insert(uid, vol);
                                            }
                                        }
                                    });
                                }
                            }
                        }

                        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                            if let Some(ref audio) = audio {
                                let is_muted = audio.muted.load(Ordering::Relaxed);
                                let is_deaf = audio.deafened.load(Ordering::Relaxed);
                                let is_ptt = audio.ptt_active.load(Ordering::Relaxed);
                                let is_open = audio.open_mic.load(Ordering::Relaxed);

                                let is_vad = audio.vad_active.load(Ordering::Relaxed);

                                // Transmit indicator
                                if is_open && is_vad && !is_muted && !is_deaf {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(80, 220, 80),
                                        "🎙 TRANSMITTING",
                                    );
                                } else if is_open && !is_muted && !is_deaf {
                                    ui.colored_label(
                                        egui::Color32::GRAY,
                                        "🎙 Open Mic (silent)",
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

                                // VAD sensitivity (only in open mic mode)
                                if *use_open_mic {
                                    let mut thresh = audio.vad_threshold.load(Ordering::Relaxed) as f32 / 10000.0;
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().slider_width = 80.0;
                                        ui.label("Sensitivity:");
                                        // Invert: low threshold = high sensitivity
                                        let mut sens = 1.0 - (thresh - 0.001) / 0.099;
                                        if ui.add(egui::Slider::new(&mut sens, 0.0..=1.0).show_value(false)).changed() {
                                            thresh = 0.001 + (1.0 - sens) * 0.099;
                                            audio.vad_threshold.store((thresh * 10000.0) as u32, Ordering::Relaxed);
                                        }
                                    });
                                }

                                // PTT key selector (only show when PTT mode)
                                if !*use_open_mic {
                                    if *listening_for_ptt {
                                        // Actively listening for a key/mouse press
                                        let btn = ui.button("⏳ Press any key/button... (Esc to cancel)");
                                        if btn.clicked() {
                                            *listening_for_ptt = false;
                                        }
                                        // Poll for input via device_query
                                        if let Some(bind) = hotkeys::capture_any_input() {
                                            if matches!(bind, PttBind::Key(device_query::Keycode::Escape)) {
                                                *listening_for_ptt = false;
                                            } else {
                                                *ptt_bind_name = bind.display_name();
                                                if let Ok(mut b) = ptt_bind.lock() {
                                                    *b = bind;
                                                }
                                                *listening_for_ptt = false;
                                            }
                                        }
                                        // Keep repainting while listening
                                        ctx.request_repaint();
                                    } else {
                                        if ui.button(format!("🎤 PTT: {}", ptt_bind_name)).clicked() {
                                            *listening_for_ptt = true;
                                        }
                                    }
                                }

                                ui.separator();

                                // Noise suppression toggle
                                let ns_on = audio.noise_suppression.load(Ordering::Relaxed);
                                if ui.selectable_label(ns_on, "🔉 Noise Suppression").clicked() {
                                    audio.noise_suppression.store(!ns_on, Ordering::Relaxed);
                                }

                                // Noise gate toggle + threshold slider
                                let ng_on = audio.noise_gate_enabled.load(Ordering::Relaxed);
                                if ui.selectable_label(ng_on, "🚪 Noise Gate").clicked() {
                                    audio.noise_gate_enabled.store(!ng_on, Ordering::Relaxed);
                                }
                                if ng_on {
                                    let mut thresh = audio.noise_gate_threshold.load(Ordering::Relaxed) as f32 / 10000.0;
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().slider_width = 80.0;
                                        ui.label("Gate:");
                                        if ui.add(egui::Slider::new(&mut thresh, 0.001..=0.05).show_value(false)).changed() {
                                            audio.noise_gate_threshold.store((thresh * 10000.0) as u32, Ordering::Relaxed);
                                        }
                                    });
                                }

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
                            let reconnect_status = match reconnect_state {
                                ReconnectState::Reconnecting { attempt, .. } => {
                                    Some(format!("⟳ Reconnecting (attempt {attempt})..."))
                                }
                                _ => None,
                            };
                            if let Some(ref rs) = reconnect_status {
                                ui.colored_label(egui::Color32::YELLOW, rs);
                            } else {
                                let upload_icon = if *upload_loss < 2.0 { "🟢" }
                                    else if *upload_loss < 10.0 { "🟡" }
                                    else { "🔴" };
                                ui.label(format!(
                                    "{} {} · Room: {} · {} online · Upload {}",
                                    voice_status,
                                    username,
                                    room,
                                    users.len(),
                                    upload_icon,
                                ));
                            }
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
                                // Encrypt chat text if encryption is active
                                let wire_text = if let Some(ref key) = encryption_key {
                                    use base64::Engine;
                                    match crate::crypto::encrypt(key, text.as_bytes()) {
                                        Ok(encrypted) => base64::engine::general_purpose::STANDARD.encode(&encrypted),
                                        Err(_) => text.clone(),
                                    }
                                } else {
                                    text.clone()
                                };
                                let _ = client_tx.send(ClientMessage::Chat {
                                    text: wire_text,
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

        // Execute deferred reconnect actions (outside the state match to avoid borrow issues)
        if let Some(params) = self.deferred_go_login.take() {
            self.save_settings_from_state();
            let input_devices = devices::list_input_devices();
            let output_devices = devices::list_output_devices();
            self.state = Screen::Login {
                server_addr: params.server_addr,
                username: params.username,
                room: params.room,
                password: params.password,
                error: Some("Connection lost".into()),
                connecting: false,
                dispatched: false,
                input_devices,
                output_devices,
                selected_input: 0,
                selected_output: 0,
            };
        } else if let Some(params) = self.deferred_reconnect.take() {
            let pw = params.password.clone();
            let raw_pw = if pw.is_empty() { None } else { Some(pw.clone()) };
            self.pending_devices = Some((params.input_device_idx, params.output_device_idx));
            self.reconnect_pending = true;
            self.initiate_connect(
                ctx,
                params.server_addr,
                params.username,
                params.room,
                pw,
                raw_pw,
                true,
            );
        }
    }
}
