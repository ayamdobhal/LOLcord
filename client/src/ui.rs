use crate::audio::{self, AudioState};
use crate::devices::{self, DeviceInfo};
use crate::hotkeys::{self, PttBind};
use crate::jitter::JitterBuffer;
use crate::net::Connection;
use crate::settings::Settings;
use iced::widget::{
    button, column, container, image, row, scrollable, text, text_input, 
    slider, Space,
};
use iced::{
    executor, time, Alignment, Application, Command, Element, Length, Subscription, Theme
};
use shared::{ClientMessage, ServerMessage};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc as std_mpsc,
    Arc, Mutex,
};
use tokio::sync::mpsc as tokio_mpsc;
use std::time::{Duration, Instant};

/// Chat message for display.
#[derive(Debug, Clone)]
struct ChatMsg {
    from: String,
    text: String,
}

/// Messages that can be sent to the Iced application
#[derive(Debug, Clone)]
pub enum Message {
    // Login screen messages
    ServerAddrChanged(String),
    UsernameChanged(String),
    RoomChanged(String),
    PasswordChanged(String),
    InputDeviceSelected(usize),
    OutputDeviceSelected(usize),
    ConnectPressed,
    
    // Connection result
    ConnectionResult(ConnectResult),
    
    // Connected screen messages
    MessageInputChanged(String),
    SendMessage,
    
    // Audio controls
    ToggleMute,
    ToggleDeafen,
    ToggleOpenMic,
    VadSensitivityChanged(f32),
    PttBindPressed,
    PttBindCaptured(Option<PttBind>),
    ToggleNoiseSupression,
    ToggleNoiseGate,
    ToggleLoopback,
    NoiseGateThresholdChanged(f32),
    UserVolumeChanged(u16, f32),
    
    // Server messages
    ServerMessage(ServerMessage),
    
    // System messages
    Tick,
    
    // Tray commands
    TrayCommand(crate::tray::TrayCommand),
    
    // Settings
    SettingsPressed,
    SettingsClosePressed,
}

/// Result of a connection attempt, sent back to the UI
#[derive(Debug, Clone)]
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
        bridge_rx: Arc<Mutex<Option<std_mpsc::Receiver<ServerMessage>>>>,
        user_ids: HashMap<String, u16>,
        password: Option<String>,
        is_reconnect: bool,
    },
    Err(String),
}

#[derive(Clone, PartialEq)]
enum ReconnectState {
    Connected,
    Reconnecting {
        attempt: u32,
        next_try: Instant,
        started: Instant,
    },
    GaveUp,
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

pub struct App {
    state: Screen,
    runtime: tokio::runtime::Runtime,
    connect_rx: std_mpsc::Receiver<ConnectResult>,
    connect_tx: std_mpsc::Sender<ConnectResult>,
    pending_devices: Option<(Option<usize>, Option<usize>)>,
    reconnect_pending: bool,
    tray_state: Option<crate::tray::TrayState>,
    
    // Icon handles for Iced
    logo_handle: Option<iced::widget::image::Handle>,
    mic_on_handle: Option<iced::widget::image::Handle>,
    mic_off_handle: Option<iced::widget::image::Handle>,
    deaf_off_handle: Option<iced::widget::image::Handle>,
    deaf_on_handle: Option<iced::widget::image::Handle>,
    send_handle: Option<iced::widget::image::Handle>,
    
    settings: Settings,
    show_settings: bool,
}

enum Screen {
    Login {
        server_addr: String,
        username: String,
        room: String,
        password: String,
        error: Option<String>,
        connecting: bool,
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
        bridge_rx: Arc<Mutex<Option<std_mpsc::Receiver<ServerMessage>>>>,
        audio: Option<Arc<AudioState>>,
        _streams: Option<(cpal::Stream, cpal::Stream)>,
        hotkey_running: Option<Arc<AtomicBool>>,
        ptt_bind: Arc<Mutex<PttBind>>,
        ptt_bind_name: String,
        listening_for_ptt: bool,
        use_open_mic: bool,
        user_volumes: Arc<Mutex<HashMap<u16, f32>>>,
        user_id_map: HashMap<String, u16>,
        our_user_id: u16,
        jitter: Arc<Mutex<JitterBuffer>>,
        quality_stats: HashMap<u16, f32>,
        quality_update: Instant,
        upload_stats: Arc<crate::voice::UploadStats>,
        upload_loss: f32,
        latency_ms: Option<u32>,
        last_ping: Instant,
        encryption_key: Option<[u8; 32]>,
        connect_params: Option<ReconnectParams>,
        reconnect_state: ReconnectState,
    },
}

impl Application for App {
    type Executor = executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: Self::Flags) -> (Self, Command<Self::Message>) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .expect("failed to create tokio runtime");

        let (connect_tx, connect_rx) = std_mpsc::channel();
        let input_devices = devices::list_input_devices();
        let output_devices = devices::list_output_devices();
        let tray_state = crate::tray::create_tray();
        let settings = Settings::load();

        let selected_input = settings.selected_input_device.as_ref()
            .and_then(|name| input_devices.iter().position(|d| d.name == *name))
            .unwrap_or(0);
        let selected_output = settings.selected_output_device.as_ref()
            .and_then(|name| output_devices.iter().position(|d| d.name == *name))
            .unwrap_or(0);

        let app = Self {
            state: Screen::Login {
                server_addr: settings.server_addr.clone().unwrap_or_else(|| format!("127.0.0.1:{}", shared::DEFAULT_PORT)),
                username: settings.username.clone().unwrap_or_default(),
                room: settings.room.clone().unwrap_or_else(|| "general".into()),
                password: String::new(),
                error: None,
                connecting: false,
                input_devices,
                output_devices,
                selected_input,
                selected_output,
            },
            runtime,
            connect_rx,
            connect_tx,
            pending_devices: None,
            reconnect_pending: false,
            tray_state,
            logo_handle: None,
            mic_on_handle: None,
            mic_off_handle: None,
            deaf_off_handle: None,
            deaf_on_handle: None,
            send_handle: None,
            settings,
            show_settings: false,
        };

        (app, Command::none())
    }

    fn title(&self) -> String {
        "LOLcord (Iced)".to_string()
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        // Poll server messages
        let mut pending_messages = Vec::new();
        if let Screen::Connected { bridge_rx, .. } = &mut self.state {
            if let Ok(mut receiver_guard) = bridge_rx.lock() {
                if let Some(ref mut receiver) = receiver_guard.as_mut() {
                    while let Ok(msg) = receiver.try_recv() {
                        pending_messages.push(msg);
                    }
                }
            }
        }
        
        // Process collected messages
        for msg in pending_messages {
            self.update(Message::ServerMessage(msg));
        }

        // Poll tray commands
        let mut tray_commands = Vec::new();
        if let Some(ref tray_state) = self.tray_state {
            while let Ok(cmd) = tray_state.rx.try_recv() {
                tray_commands.push(cmd);
            }
        }
        
        // Process collected tray commands
        for cmd in tray_commands {
            let _ = self.update(Message::TrayCommand(cmd));
        }

        match message {
            Message::ServerAddrChanged(addr) => {
                if let Screen::Login { server_addr, .. } = &mut self.state {
                    *server_addr = addr;
                }
            }
            Message::UsernameChanged(name) => {
                if let Screen::Login { username, .. } = &mut self.state {
                    *username = name;
                }
            }
            Message::RoomChanged(room) => {
                if let Screen::Login { room: r, .. } = &mut self.state {
                    *r = room;
                }
            }
            Message::PasswordChanged(pass) => {
                if let Screen::Login { password, .. } = &mut self.state {
                    *password = pass;
                }
            }
            Message::InputDeviceSelected(idx) => {
                if let Screen::Login { selected_input, .. } = &mut self.state {
                    *selected_input = idx;
                }
            }
            Message::OutputDeviceSelected(idx) => {
                if let Screen::Login { selected_output, .. } = &mut self.state {
                    *selected_output = idx;
                }
            }
            Message::ConnectPressed => {
                if let Screen::Login { 
                    connecting, 
                    username, 
                    server_addr, 
                    room, 
                    password, 
                    error,
                    selected_input,
                    selected_output,
                    input_devices,
                    output_devices,
                    .. 
                } = &mut self.state {
                    let user_trimmed = username.trim().to_string();
                    if user_trimmed.is_empty() {
                        *error = Some("username required".into());
                    } else {
                        *connecting = true;
                        *error = None;
                        
                        // Store selected devices
                        let in_idx = input_devices.get(*selected_input).and_then(|d| d.index);
                        let out_idx = output_devices.get(*selected_output).and_then(|d| d.index);
                        self.pending_devices = Some((in_idx, out_idx));
                        
                        // Initiate connection
                        let addr = server_addr.clone();
                        let user = user_trimmed;
                        let rm = room.trim().to_string();
                        let pw = password.clone();
                        let raw_pw = if pw.is_empty() { None } else { Some(pw.clone()) };
                        
                        return Command::perform(
                            Self::connect(addr, user, rm, pw, raw_pw, false),
                            Message::ConnectionResult
                        );
                    }
                }
                
                // Save settings after handling connect logic
                self.save_settings_from_state();
            }
            Message::ConnectionResult(result) => {
                match result {
                    ConnectResult::Ok { 
                        room, 
                        username, 
                        mut users, 
                        user_id, 
                        room_id, 
                        voice_port, 
                        server_addr, 
                        client_tx, 
                        bridge_rx, 
                        user_ids: initial_user_ids, 
                        password, 
                        is_reconnect 
                    } => {
                        if !users.contains(&username) {
                            users.push(username.clone());
                        }

                        // Derive E2E encryption key from password
                        let encryption_key = password.as_ref()
                            .filter(|p| !p.is_empty())
                            .map(|p| crate::crypto::derive_key(p));

                        self.reconnect_pending = false;

                        // Get selected devices from pending state
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

                        let initial_messages = if is_reconnect {
                            vec![ChatMsg {
                                from: "system".into(),
                                text: "Reconnected!".into(),
                            }]
                        } else {
                            Vec::new()
                        };

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
                            ptt_bind_name,
                            listening_for_ptt: false,
                            use_open_mic: self.settings.use_open_mic.unwrap_or(true),
                            user_volumes,
                            user_id_map: initial_user_ids,
                            our_user_id: user_id,
                            upload_stats,
                            upload_loss: 0.0,
                            latency_ms: None,
                            last_ping: Instant::now(),
                            encryption_key,
                            connect_params: Some(rp),
                            reconnect_state: ReconnectState::Connected,
                            jitter: jitter.clone(),
                            quality_stats: HashMap::new(),
                            quality_update: Instant::now(),
                        };
                    }
                    ConnectResult::Err(msg) => {
                        if self.reconnect_pending {
                            self.reconnect_pending = false;
                            tracing::warn!("reconnect failed: {msg}");
                        } else if let Screen::Login { error, connecting, .. } = &mut self.state {
                            *error = Some(msg);
                            *connecting = false;
                        }
                    }
                }
            }
            Message::MessageInputChanged(text) => {
                if let Screen::Connected { input, .. } = &mut self.state {
                    *input = text;
                }
            }
            Message::SendMessage => {
                if let Screen::Connected { 
                    input, 
                    client_tx, 
                    messages, 
                    username,
                    encryption_key,
                    .. 
                } = &mut self.state {
                    if !input.trim().is_empty() {
                        let text = input.trim().to_string();
                        let wire_text = if let Some(ref key) = encryption_key {
                            use base64::Engine;
                            match crate::crypto::encrypt(key, text.as_bytes()) {
                                Ok(encrypted) => base64::engine::general_purpose::STANDARD.encode(&encrypted),
                                Err(_) => text.clone(),
                            }
                        } else {
                            text.clone()
                        };
                        let _ = client_tx.send(ClientMessage::Chat { text: wire_text });
                        messages.push(ChatMsg {
                            from: username.clone(),
                            text,
                        });
                        input.clear();
                    }
                }
            }
            Message::ToggleMute => {
                if let Screen::Connected { audio, .. } = &mut self.state {
                    if let Some(ref a) = audio {
                        let current = a.muted.load(Ordering::Relaxed);
                        a.muted.store(!current, Ordering::Relaxed);
                    }
                }
            }
            Message::ToggleDeafen => {
                if let Screen::Connected { audio, .. } = &mut self.state {
                    if let Some(ref a) = audio {
                        let current = a.deafened.load(Ordering::Relaxed);
                        let new_deaf = !current;
                        a.deafened.store(new_deaf, Ordering::Relaxed);
                        // Deafen implies mute
                        if new_deaf {
                            a.muted.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
            Message::ToggleOpenMic => {
                if let Screen::Connected { use_open_mic, audio, .. } = &mut self.state {
                    *use_open_mic = !*use_open_mic;
                    if let Some(ref a) = audio {
                        a.open_mic.store(*use_open_mic, Ordering::Relaxed);
                    }
                }
            }
            Message::ServerMessage(msg) => {
                if let Screen::Connected { 
                    messages, 
                    users, 
                    user_id_map, 
                    latency_ms,
                    encryption_key,
                    .. 
                } = &mut self.state {
                    match msg {
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
                        ServerMessage::Pong { ts } => {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64;
                            *latency_ms = Some((now.saturating_sub(ts)) as u32);
                        }
                        _ => {}
                    }
                }
            }
            Message::Tick => {
                // Poll for PTT capture when listening
                if let Screen::Connected { listening_for_ptt: true, .. } = &self.state {
                    if let Some(bind) = hotkeys::capture_any_input() {
                        return Command::perform(
                            async move { bind },
                            |bind| Message::PttBindCaptured(Some(bind))
                        );
                    }
                }

                // Handle periodic tasks like ping, quality stats updates, reconnection logic
                if let Screen::Connected { 
                    client_tx, 
                    last_ping, 
                    upload_stats,
                    upload_loss,
                    jitter,
                    quality_stats,
                    quality_update,
                    reconnect_state,
                    connect_params,
                    messages,
                    .. 
                } = &mut self.state {
                    
                    // Send ping every 2 seconds
                    if last_ping.elapsed() >= Duration::from_secs(2) {
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64;
                        let _ = client_tx.send(ClientMessage::Ping { ts });
                        *last_ping = Instant::now();
                    }

                    // Update connection quality stats every 3 seconds
                    if quality_update.elapsed() >= Duration::from_secs(3) {
                        if let Ok(mut jb) = jitter.lock() {
                            *quality_stats = jb.get_quality_stats();
                            jb.reset_quality_stats();
                        }
                        *upload_loss = upload_stats.take_loss_percent();
                        *quality_update = Instant::now();
                    }

                    // Handle reconnection logic
                    if let ReconnectState::Reconnecting { attempt, next_try, started } = reconnect_state {
                        let now = Instant::now();
                        if now.duration_since(*started).as_secs() > 120 {
                            *reconnect_state = ReconnectState::GaveUp;
                            messages.push(ChatMsg {
                                from: "system".into(),
                                text: "Reconnection failed. Returning to login...".into(),
                            });
                            // TODO: Return to login screen
                        } else if now >= *next_try && !self.reconnect_pending {
                            if let Some(params) = connect_params.clone() {
                                *attempt += 1;
                                let delay_secs = std::cmp::min(30, 1u64 << (*attempt).min(5));
                                *next_try = now + Duration::from_secs(delay_secs);
                                
                                // Trigger reconnection
                                self.reconnect_pending = true;
                                let pw = params.password.clone();
                                let raw_pw = if pw.is_empty() { None } else { Some(pw.clone()) };
                                return Command::perform(
                                    Self::connect(
                                        params.server_addr,
                                        params.username,
                                        params.room,
                                        pw,
                                        raw_pw,
                                        true
                                    ),
                                    Message::ConnectionResult
                                );
                            }
                        }
                    }
                }
            }
            Message::TrayCommand(cmd) => {
                match cmd {
                    crate::tray::TrayCommand::Show => {
                        // TODO: Implement window show/focus
                    }
                    crate::tray::TrayCommand::ToggleMute => {
                        if let Screen::Connected { audio, .. } = &mut self.state {
                            if let Some(ref a) = audio {
                                let current = a.muted.load(Ordering::Relaxed);
                                a.muted.store(!current, Ordering::Relaxed);
                            }
                        }
                    }
                    crate::tray::TrayCommand::ToggleDeafen => {
                        if let Screen::Connected { audio, .. } = &mut self.state {
                            if let Some(ref a) = audio {
                                let current = a.deafened.load(Ordering::Relaxed);
                                let new_deaf = !current;
                                a.deafened.store(new_deaf, Ordering::Relaxed);
                                if new_deaf {
                                    a.muted.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    crate::tray::TrayCommand::Quit => {
                        self.save_settings_from_state();
                        std::process::exit(0);
                    }
                }
            }
            Message::VadSensitivityChanged(sens) => {
                if let Screen::Connected { audio, .. } = &mut self.state {
                    if let Some(ref a) = audio {
                        let thresh = 0.001 + (1.0 - sens) * 0.099;
                        a.vad_threshold.store((thresh * 10000.0) as u32, Ordering::Relaxed);
                    }
                }
            }
            Message::NoiseGateThresholdChanged(thresh) => {
                if let Screen::Connected { audio, .. } = &mut self.state {
                    if let Some(ref a) = audio {
                        a.noise_gate_threshold.store((thresh * 10000.0) as u32, Ordering::Relaxed);
                    }
                }
            }
            Message::ToggleNoiseSupression => {
                if let Screen::Connected { audio, .. } = &mut self.state {
                    if let Some(ref a) = audio {
                        let current = a.noise_suppression.load(Ordering::Relaxed);
                        a.noise_suppression.store(!current, Ordering::Relaxed);
                    }
                }
            }
            Message::ToggleNoiseGate => {
                if let Screen::Connected { audio, .. } = &mut self.state {
                    if let Some(ref a) = audio {
                        let current = a.noise_gate_enabled.load(Ordering::Relaxed);
                        a.noise_gate_enabled.store(!current, Ordering::Relaxed);
                    }
                }
            }
            Message::ToggleLoopback => {
                if let Screen::Connected { audio, .. } = &mut self.state {
                    if let Some(ref a) = audio {
                        let current = a.loopback.load(Ordering::Relaxed);
                        let new_val = !current;
                        a.loopback.store(new_val, Ordering::Relaxed);
                        if new_val {
                            // Loopback on: mute + deafen (private mic test)
                            a.muted.store(true, Ordering::Relaxed);
                            a.deafened.store(true, Ordering::Relaxed);
                        } else {
                            // Loopback off: unmute + undeafen
                            a.muted.store(false, Ordering::Relaxed);
                            a.deafened.store(false, Ordering::Relaxed);
                        }
                    }
                }
            }
            Message::UserVolumeChanged(user_id, volume) => {
                if let Screen::Connected { user_volumes, .. } = &mut self.state {
                    if let Ok(mut vols) = user_volumes.lock() {
                        vols.insert(user_id, volume);
                    }
                }
            }
            Message::PttBindPressed => {
                if let Screen::Connected { listening_for_ptt, .. } = &mut self.state {
                    *listening_for_ptt = !*listening_for_ptt;
                }
            }
            Message::PttBindCaptured(bind) => {
                if let Screen::Connected { ptt_bind, ptt_bind_name, listening_for_ptt, .. } = &mut self.state {
                    if let Some(bind) = bind {
                        *ptt_bind_name = bind.display_name();
                        if let Ok(mut b) = ptt_bind.lock() {
                            *b = bind;
                        }
                    }
                    *listening_for_ptt = false;
                }
            }
            Message::SettingsPressed => {
                self.show_settings = true;
            }
            Message::SettingsClosePressed => {
                self.show_settings = false;
            }
        }

        Command::none()
    }

    fn view(&self) -> Element<Self::Message> {
        match &self.state {
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
                self.view_login(
                    server_addr, 
                    username, 
                    room, 
                    password, 
                    error.as_deref(), 
                    *connecting,
                    input_devices,
                    output_devices,
                    *selected_input,
                    *selected_output,
                )
            }
            Screen::Connected { .. } => {
                self.view_connected()
            }
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        // For now, just use the periodic tick to handle all polling
        time::every(Duration::from_millis(100)).map(|_| Message::Tick)
    }

    fn theme(&self) -> Self::Theme {
        Theme::Dark
    }
}

impl App {
    async fn connect(
        addr: String,
        username: String,
        room: String,
        password: String,
        raw_password: Option<String>,
        is_reconnect: bool,
    ) -> ConnectResult {
        let join_msg = ClientMessage::Join {
            username: username.clone(),
            room: room.clone(),
            password: if password.is_empty() { None } else { Some(password) },
        };

        match Connection::connect(&addr, join_msg).await {
            Ok(mut conn) => {
                let first = conn.server_rx.recv().await;
                match first {
                    Some(ServerMessage::RoomState { room: r, users, user_id, room_id, voice_port, user_ids }) => {
                        let (bridge_tx, raw_bridge_rx) = std_mpsc::channel();
                        let bridge_rx = Arc::new(Mutex::new(Some(raw_bridge_rx)));
                        
                        tokio::spawn(async move {
                            while let Some(msg) = conn.server_rx.recv().await {
                                if bridge_tx.send(msg).is_err() {
                                    break;
                                }
                            }
                        });

                        ConnectResult::Ok {
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
                        }
                    }
                    Some(ServerMessage::Error { message }) => {
                        ConnectResult::Err(message)
                    }
                    _ => {
                        ConnectResult::Err("unexpected server response".into())
                    }
                }
            }
            Err(e) => ConnectResult::Err(e.to_string()),
        }
    }

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

    fn view_login(
        &self,
        server_addr: &str,
        username: &str,
        room: &str,
        password: &str,
        error: Option<&str>,
        connecting: bool,
        input_devices: &[DeviceInfo],
        output_devices: &[DeviceInfo],
        selected_input: usize,
        selected_output: usize,
    ) -> Element<Message> {
        let logo = if let Some(ref handle) = self.logo_handle {
            image(handle.clone()).width(120).height(120)
        } else {
            // Load logo on first render
            let bytes = include_bytes!("../assets/logo.png");
            let handle = iced::widget::image::Handle::from_memory(bytes);
            image(handle.clone()).width(120).height(120)
        };

        let mut content = column![
            Space::with_height(20),
            container(logo).center_x(),
            Space::with_height(20),
        ];

        // Form fields
        let form = column![
            row![
                text("Server:").width(80),
                text_input("127.0.0.1:3000", server_addr)
                    .on_input(Message::ServerAddrChanged)
                    .width(250)
            ].spacing(10),
            
            row![
                text("Username:").width(80),
                text_input("username", username)
                    .on_input(Message::UsernameChanged)
                    .on_submit(Message::ConnectPressed)
                    .width(250)
            ].spacing(10),
            
            row![
                text("Room:").width(80),
                text_input("general", room)
                    .on_input(Message::RoomChanged)
                    .on_submit(Message::ConnectPressed)
                    .width(250)
            ].spacing(10),
            
            row![
                text("Password:").width(80),
                text_input("password", password)
                    .on_input(Message::PasswordChanged)
                    .on_submit(Message::ConnectPressed)
                    .secure(true)
                    .width(250)
            ].spacing(10),
            
            row![
                text("Microphone:").width(80),
                text(input_devices.get(selected_input).map(|d| d.name.as_str()).unwrap_or("Unknown"))
                    .width(250)
            ].spacing(10),
            
            row![
                text("Speaker:").width(80),
                text(output_devices.get(selected_output).map(|d| d.name.as_str()).unwrap_or("Unknown"))
                    .width(250)
            ].spacing(10),
        ].spacing(10);

        content = content.push(form);
        content = content.push(Space::with_height(20));

        // Connect button
        if connecting {
            content = content.push(text("Connecting..."));
        } else {
            content = content.push(
                button("Connect")
                    .on_press(Message::ConnectPressed)
                    .padding(10)
            );
        }

        // Error message
        if let Some(err) = error {
            content = content.push(Space::with_height(10));
            content = content.push(
                text(err)
            );
        }

        container(content)
            .center_x()
            .center_y()
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_connected(&self) -> Element<Message> {
        if let Screen::Connected {
            room,
            username,
            users,
            messages,
            input,
            audio,
            use_open_mic,
            user_volumes,
            user_id_map,
            quality_stats,
            upload_loss,
            latency_ms,
            reconnect_state,
            ptt_bind_name,
            listening_for_ptt,
            ..
        } = &self.state {
            
            let voice_status = if let Some(ref a) = audio {
                if a.deafened.load(Ordering::Relaxed) {
                    "DEAFENED"
                } else if a.muted.load(Ordering::Relaxed) {
                    "MUTED"
                } else if a.open_mic.load(Ordering::Relaxed) {
                    "Open Mic"
                } else if a.ptt_active.load(Ordering::Relaxed) {
                    "PTT Active"
                } else {
                    "PTT Ready"
                }
            } else {
                "No Audio"
            };

            let latency_str = match latency_ms {
                Some(ms) => format!("{ms}ms"),
                None => "...".to_string(),
            };

            let reconnect_status = match reconnect_state {
                ReconnectState::Reconnecting { attempt, .. } => {
                    Some(format!("Reconnecting (attempt {attempt})..."))
                }
                _ => None,
            };

            // Top bar
            let logo = image(image::Handle::from_memory(include_bytes!("../assets/logo.png").to_vec()))
                .width(28).height(28);
            let top_bar = container(
                row![
                    logo,
                    text("LOLcord").size(18),
                    Space::with_width(Length::Fill),
                    text(format!("# {}", room)).size(16),
                    Space::with_width(Length::Fill),
                    button("Settings").on_press(Message::SettingsPressed)
                ]
                .spacing(8)
                .padding(10)
                .align_items(Alignment::Center)
            ).style(iced::theme::Container::default());

            // Left sidebar - Users panel
            let mut user_list = column![]
                .spacing(5)
                .padding(10);

            for user in users.iter() {
                if user == username {
                    user_list = user_list.push(text(format!("{user} (you)")));
                } else {
                    let quality_color = user_id_map.get(user)
                        .and_then(|uid| quality_stats.get(uid))
                        .map(|&loss| {
                            if loss < 2.0 { iced::Color::from_rgb(0.0, 1.0, 0.0) }
                            else if loss < 10.0 { iced::Color::from_rgb(1.0, 1.0, 0.0) }
                            else { iced::Color::from_rgb(1.0, 0.0, 0.0) }
                        });

                    let mut user_row = row![]
                        .spacing(5)
                        .align_items(Alignment::Center);

                    if let Some(_color) = quality_color {
                        user_row = user_row.push(text("●"));
                    }
                    user_row = user_row.push(text(user));
                    user_list = user_list.push(user_row);

                    // Per-user volume slider
                    if let Some(&uid) = user_id_map.get(user) {
                        let vol = user_volumes
                            .lock()
                            .ok()
                            .and_then(|v| v.get(&uid).copied())
                            .unwrap_or(1.0);
                        let vol_pct = (vol * 100.0) as u32;
                        
                        user_list = user_list.push(
                            row![
                                slider(0.0..=2.0, vol, move |v| Message::UserVolumeChanged(uid, v))
                                    .width(80),
                                text(format!("{vol_pct}%")).size(12)
                            ]
                            .spacing(5)
                            .align_items(Alignment::Center)
                        );
                    }
                }
            }

            // Audio controls at bottom of sidebar
            let mut audio_controls = column![]
                .spacing(8);

            if let Some(ref a) = audio {
                let is_muted = a.muted.load(Ordering::Relaxed);
                let is_deaf = a.deafened.load(Ordering::Relaxed);
                let is_vad = a.vad_active.load(Ordering::Relaxed);

                // Mute/Deafen buttons (custom icons)
                let mic_icon = if is_muted {
                    image(image::Handle::from_memory(include_bytes!("../assets/mic_off.png").to_vec()))
                } else {
                    image(image::Handle::from_memory(include_bytes!("../assets/mic_on.png").to_vec()))
                };
                let deaf_icon = if is_deaf {
                    image(image::Handle::from_memory(include_bytes!("../assets/deaf_on.png").to_vec()))
                } else {
                    image(image::Handle::from_memory(include_bytes!("../assets/deaf_off.png").to_vec()))
                };

                let mute_btn = button(mic_icon.width(24).height(24))
                    .on_press(Message::ToggleMute)
                    .padding(4);
                let deaf_btn = button(deaf_icon.width(24).height(24))
                    .on_press(Message::ToggleDeafen)
                    .padding(4);

                audio_controls = audio_controls.push(
                    row![mute_btn, deaf_btn].spacing(5)
                );

                // Transmit indicator
                if *use_open_mic && is_vad && !is_muted && !is_deaf {
                    audio_controls = audio_controls.push(text(">> TRANSMITTING"));
                } else if *use_open_mic && !is_muted && !is_deaf {
                    audio_controls = audio_controls.push(text("Open Mic (silent)"));
                } else if a.ptt_active.load(Ordering::Relaxed) && !is_muted && !is_deaf {
                    audio_controls = audio_controls.push(text(">> TRANSMITTING"));
                }

                // Voice mode indicator
                let mode_text = if *use_open_mic { "Open Mic" } else { "PTT" };
                audio_controls = audio_controls.push(text(mode_text).size(11));
            } else {
                audio_controls = audio_controls.push(text("! No audio"));
            }

            let sidebar = column![
                user_list,
                Space::with_height(Length::Fill),
                audio_controls,
            ]
            .width(180)
            .padding(5);

            // Chat messages area (center)
            let mut chat_messages = column![]
                .spacing(2)
                .padding(10);

            for msg in messages.iter() {
                if msg.from == "system" {
                    chat_messages = chat_messages.push(text(&msg.text));
                } else {
                    chat_messages = chat_messages.push(
                        row![
                            text(format!("{}:", msg.from)),
                            text(&msg.text)
                        ].spacing(5)
                    );
                }
            }

            let chat_area = scrollable(chat_messages)
                .height(Length::Fill);

            // Input bar (bottom)
            let input_bar = row![
                text_input("type a message...", input)
                    .on_input(Message::MessageInputChanged)
                    .on_submit(Message::SendMessage)
                    .width(Length::Fill),
                button(
                    image(image::Handle::from_memory(include_bytes!("../assets/send.png").to_vec()))
                        .width(20).height(20)
                ).on_press(Message::SendMessage).padding(4)
            ]
            .spacing(5)
            .padding(10)
            .align_items(Alignment::Center);

            // Status bar (very bottom)
            let status_text = if let Some(ref rs) = reconnect_status {
                rs.clone()
            } else {
                format!(
                    "{} {} · Room: {} · {} online · Loss: {:.1}% · Ping: {}",
                    voice_status,
                    username,
                    room,
                    users.len(),
                    upload_loss,
                    latency_str,
                )
            };

            let status_bar = container(
                text(status_text).size(12)
            )
            .padding(5)
            .width(Length::Fill)
            .center_x();

            // Settings panel (replaces chat when open)
            let right_side: Element<Message> = if self.show_settings {
                let mut settings = column![
                    text("Settings").size(20),
                    Space::with_height(10),
                ].spacing(8).padding(15).width(Length::Fill);

                if let Some(ref a) = audio {
                    // Voice mode
                    let open_mic_label = if *use_open_mic { "> Open Mic" } else { "  Open Mic" };
                    let ptt_label = if !*use_open_mic { "> Push to Talk" } else { "  Push to Talk" };
                    settings = settings.push(text("Voice Mode:").size(14));
                    settings = settings.push(button(open_mic_label).on_press(Message::ToggleOpenMic));
                    settings = settings.push(button(ptt_label).on_press(Message::ToggleOpenMic));

                    // VAD sensitivity
                    if *use_open_mic {
                        let thresh = a.vad_threshold.load(Ordering::Relaxed) as f32 / 10000.0;
                        let sens = 1.0 - (thresh - 0.001) / 0.099;
                        settings = settings.push(
                            row![
                                text("Sensitivity:").size(12),
                                slider(0.0..=1.0, sens, Message::VadSensitivityChanged).width(150)
                            ].spacing(8).align_items(Alignment::Center)
                        );
                    }

                    // PTT keybind
                    if !*use_open_mic {
                        let btn_text = if *listening_for_ptt {
                            "Press any key... (Esc to cancel)".to_string()
                        } else {
                            format!("PTT: {}", ptt_bind_name)
                        };
                        settings = settings.push(button(text(&btn_text)).on_press(Message::PttBindPressed));
                    }

                    settings = settings.push(Space::with_height(5));

                    // Noise suppression
                    let ns_on = a.noise_suppression.load(Ordering::Relaxed);
                    let ns_label = if ns_on { "Noise Suppression: ON" } else { "Noise Suppression: OFF" };
                    settings = settings.push(button(ns_label).on_press(Message::ToggleNoiseSupression));

                    // Noise gate
                    let ng_on = a.noise_gate_enabled.load(Ordering::Relaxed);
                    let ng_label = if ng_on { "Noise Gate: ON" } else { "Noise Gate: OFF" };
                    settings = settings.push(button(ng_label).on_press(Message::ToggleNoiseGate));
                    if ng_on {
                        let thresh = a.noise_gate_threshold.load(Ordering::Relaxed) as f32 / 10000.0;
                        settings = settings.push(
                            row![
                                text("Gate:").size(12),
                                slider(0.001..=0.05, thresh, Message::NoiseGateThresholdChanged).width(150)
                            ].spacing(8).align_items(Alignment::Center)
                        );
                    }

                    // Loopback
                    let lb_on = a.loopback.load(Ordering::Relaxed);
                    let lb_label = if lb_on { "Loopback: ON" } else { "Loopback: OFF" };
                    settings = settings.push(button(lb_label).on_press(Message::ToggleLoopback));
                }

                settings = settings.push(Space::with_height(10));
                settings = settings.push(button("Close Settings").on_press(Message::SettingsClosePressed));

                scrollable(settings).height(Length::Fill).into()
            } else {
                column![
                    chat_area,
                    input_bar,
                ].width(Length::Fill).into()
            };

            let main_content = column![
                top_bar,
                row![
                    sidebar,
                    right_side,
                ]
                .height(Length::Fill),
                status_bar,
            ];

            container(main_content)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            text("Error: Invalid state").into()
        }
    }

}