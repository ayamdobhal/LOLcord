use crate::audio::AudioState;
use shared::voice;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{error, info};

/// Spawn voice send/receive tasks.
pub async fn run(
    audio: Arc<AudioState>,
    server_addr: SocketAddr,
    room_id: u16,
    user_id: u16,
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

    // Receive task
    let recv_socket = socket.clone();
    let recv_audio = audio.clone();
    let recv_handle = tokio::spawn(async move {
        let mut buf = [0u8; voice::MAX_PACKET_SIZE];
        loop {
            match recv_socket.recv_from(&mut buf).await {
                Ok((len, _addr)) => {
                    if len <= voice::HEADER_SIZE {
                        continue;
                    }
                    if let Some((_rid, uid, _seq)) = voice::decode_header(&buf[..len]) {
                        if uid == user_id {
                            continue;
                        }
                    }
                    let opus_data = &buf[voice::HEADER_SIZE..len];
                    recv_audio.decode_and_play(opus_data);
                }
                Err(e) => {
                    error!("voice recv error: {e}");
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = send_handle => {},
        _ = recv_handle => {},
    }
}
