use crate::audio::AudioState;
use crate::jitter::JitterBuffer;
use shared::voice;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tracing::{error, info};

/// Spawn voice send/receive tasks.
pub async fn run(
    audio: Arc<AudioState>,
    server_addr: SocketAddr,
    room_id: u16,
    user_id: u16,
    jitter: Arc<Mutex<JitterBuffer>>,
    user_volumes: Arc<Mutex<HashMap<u16, f32>>>,
) {
    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!("failed to bind UDP socket: {e}");
            return;
        }
    };

    info!("voice UDP bound to {:?}", socket.local_addr());

    // Send task
    let send_socket = socket.clone();
    let send_audio = audio.clone();
    let send_handle = tokio::spawn(async move {
        let mut seq: u16 = 0;
        let frame_interval =
            tokio::time::Duration::from_millis(voice::FRAME_DURATION_MS as u64);
        let mut interval = tokio::time::interval(frame_interval);

        loop {
            interval.tick().await;

            if let Some(opus_data) = send_audio.try_encode_frame() {
                let header = voice::encode_header(room_id, user_id, seq);
                let mut packet =
                    Vec::with_capacity(voice::HEADER_SIZE + opus_data.len());
                packet.extend_from_slice(&header);
                packet.extend_from_slice(&opus_data);

                if let Err(e) = send_socket.send_to(&packet, server_addr).await {
                    error!("voice send error: {e}");
                    break;
                }
                seq = seq.wrapping_add(1);
            }
        }
    });

    // Receive task — insert into jitter buffer
    let recv_socket = socket.clone();
    let recv_jitter = jitter.clone();
    let recv_handle = tokio::spawn(async move {
        let mut buf = [0u8; voice::MAX_PACKET_SIZE];
        loop {
            match recv_socket.recv_from(&mut buf).await {
                Ok((len, _addr)) => {
                    if len <= voice::HEADER_SIZE {
                        continue;
                    }
                    if let Some((_rid, uid, seq)) = voice::decode_header(&buf[..len]) {
                        if uid == user_id {
                            continue;
                        }
                        let opus_data = buf[voice::HEADER_SIZE..len].to_vec();
                        if let Ok(mut jb) = recv_jitter.lock() {
                            jb.insert(uid, seq, opus_data);
                        }
                    }
                }
                Err(e) => {
                    error!("voice recv error: {e}");
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

    tokio::select! {
        _ = send_handle => {},
        _ = recv_handle => {},
        _ = play_handle => {},
    }
}
