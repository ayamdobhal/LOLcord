use crate::audio::AudioState;
use crate::crypto;
use crate::jitter::JitterBuffer;
use crate::screen_capture::ScreenCaptureState;
use crate::screen_decode::ScreenDecoder;
use shared::{voice, screen};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex,
};
use tokio::net::UdpSocket;
use tracing::{error, info, warn};

/// Shared upload quality stats.
pub struct UploadStats {
    pub packets_sent: AtomicU32,
    pub packets_failed: AtomicU32,
}

impl UploadStats {
    pub fn new() -> Self {
        Self {
            packets_sent: AtomicU32::new(0),
            packets_failed: AtomicU32::new(0),
        }
    }

    /// Get and reset upload loss percentage.
    pub fn take_loss_percent(&self) -> f32 {
        let sent = self.packets_sent.swap(0, Ordering::Relaxed);
        let failed = self.packets_failed.swap(0, Ordering::Relaxed);
        let total = sent + failed;
        if total == 0 {
            0.0
        } else {
            (failed as f32 / total as f32) * 100.0
        }
    }
}

/// Spawn voice send/receive tasks.
/// `encryption_key`: if Some, encrypt/decrypt voice payloads (E2E).
pub async fn run(
    audio: Arc<AudioState>,
    server_addr: SocketAddr,
    room_id: u16,
    user_id: u16,
    jitter: Arc<Mutex<JitterBuffer>>,
    user_volumes: Arc<Mutex<HashMap<u16, f32>>>,
    encryption_key: Option<[u8; 32]>,
    upload_stats: Arc<UploadStats>,
    screen_capture: Option<Arc<ScreenCaptureState>>,
    screen_decoder: Option<Arc<ScreenDecoder>>,
) {
    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!("failed to bind UDP socket: {e}");
            return;
        }
    };

    info!("voice UDP bound to {:?}, encrypted={}", socket.local_addr(), encryption_key.is_some());

    // Screen capture activation task
    let _capture_socket = socket.clone();
    let capture_handle = if let Some(ref sc) = screen_capture {
        let sc_clone = sc.clone();
        Some(tokio::spawn(async move {
            let _last_check = std::time::Instant::now();
            let check_interval = std::time::Duration::from_millis(100);
            
            loop {
                tokio::time::sleep(check_interval).await;
                
                // Check if we should start screen capture
                if sc_clone.capturing.load(std::sync::atomic::Ordering::Relaxed) {
                    // Capture should already be running
                    continue;
                }
                
                // For now, we'll handle the capture start/stop through server messages
                // This task can be used for periodic cleanup or status checks
            }
        }))
    } else {
        None
    };

    // Send task
    let send_socket = socket.clone();
    let send_audio = audio.clone();
    let send_key = encryption_key;
    let send_handle = tokio::spawn(async move {
        let mut seq: u16 = 0;
        let frame_interval =
            tokio::time::Duration::from_millis(voice::FRAME_DURATION_MS as u64);
        let mut interval = tokio::time::interval(frame_interval);

        loop {
            interval.tick().await;

            if let Some(opus_data) = send_audio.try_encode_frame() {
                let header = voice::encode_header(room_id, user_id, seq);

                let payload = if let Some(ref key) = send_key {
                    match crypto::encrypt(key, &opus_data) {
                        Ok(encrypted) => encrypted,
                        Err(e) => {
                            warn!("voice encrypt error: {e}");
                            continue;
                        }
                    }
                } else {
                    opus_data
                };

                let mut packet =
                    Vec::with_capacity(voice::HEADER_SIZE + payload.len());
                packet.extend_from_slice(&header);
                packet.extend_from_slice(&payload);

                match send_socket.send_to(&packet, server_addr).await {
                    Ok(_) => {
                        upload_stats.packets_sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        upload_stats.packets_failed.fetch_add(1, Ordering::Relaxed);
                        error!("voice send error: {e}");
                        break;
                    }
                }
                seq = seq.wrapping_add(1);
            }
        }
    });

    // Receive task — insert voice into jitter buffer, screen packets into decoder
    let recv_socket = socket.clone();
    let recv_jitter = jitter.clone();
    let recv_key = encryption_key;
    let recv_decoder = screen_decoder.clone();
    let recv_handle = tokio::spawn(async move {
        const MAX_BUF_SIZE: usize = 2048; // Large enough for both voice and screen packets
        let mut buf = [0u8; MAX_BUF_SIZE];
        loop {
            match recv_socket.recv_from(&mut buf).await {
                Ok((len, _addr)) => {
                    // Try voice packet first
                    if len > voice::HEADER_SIZE {
                        if let Some((_rid, uid, seq)) = voice::decode_header(&buf[..len]) {
                            if uid != user_id {
                                let encrypted_payload = &buf[voice::HEADER_SIZE..len];

                                let opus_data = if let Some(ref key) = recv_key {
                                    match crypto::decrypt(key, encrypted_payload) {
                                        Ok(decrypted) => decrypted,
                                        Err(_) => continue,
                                    }
                                } else {
                                    encrypted_payload.to_vec()
                                };

                                if let Ok(mut jb) = recv_jitter.lock() {
                                    jb.insert(uid, seq, opus_data);
                                }
                            }
                            continue;
                        }
                    }
                    
                    // Try screen packet
                    if len > screen::HEADER_SIZE {
                        if screen::decode_header(&buf[..len]).is_some() {
                            // Forward to screen decoder
                            if let Some(ref decoder) = recv_decoder {
                                if let Err(e) = decoder.process_packet(&buf[..len]).await {
                                    warn!("Screen packet decode error: {}", e);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("UDP recv error: {e}");
                    break;
                }
            }
        }
    });

    // Playback task — drain jitter buffer at steady 20ms intervals
    let play_audio = audio.clone();
    let play_jitter = jitter.clone();
    let play_volumes = user_volumes.clone();
    let play_handle = tokio::spawn(async move {
        let frame_interval =
            tokio::time::Duration::from_millis(voice::FRAME_DURATION_MS as u64);
        let mut interval = tokio::time::interval(frame_interval);

        loop {
            interval.tick().await;

            if play_audio.deafened.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }

            // Sync volumes from UI into jitter buffer
            if let (Ok(mut jb), Ok(vols)) = (play_jitter.lock(), play_volumes.lock()) {
                for (&uid, &vol) in vols.iter() {
                    jb.volumes.insert(uid, vol);
                }

                if let Some(pcm) = jb.mix_frame() {
                    play_audio.push_playback(&pcm);
                }
            }
        }
    });

    if let Some(capture_handle) = capture_handle {
        tokio::select! {
            _ = send_handle => {},
            _ = recv_handle => {},
            _ = play_handle => {},
            _ = capture_handle => {},
        }
    } else {
        tokio::select! {
            _ = send_handle => {},
            _ = recv_handle => {},
            _ = play_handle => {},
        }
    }
}
