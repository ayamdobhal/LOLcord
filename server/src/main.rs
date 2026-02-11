mod config;
mod room;

use anyhow::Result;
use config::ServerConfig;
use room::RoomManager;
use shared::{ClientMessage, ServerMessage, protocol, voice};
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "server=info".into()),
        )
        .init();

    let cfg = ServerConfig::load();
    cfg.log();

    let port = cfg.tcp_port;
    let voice_port = cfg.udp_port;

    let rooms = Arc::new(RoomManager::new(cfg.max_users_per_room, cfg.max_rooms));

    // TCP signaling
    let tcp_listener = TcpListener::bind(format!("{}:{port}", cfg.bind_addr)).await?;
    info!("TCP signaling on {}:{port}", cfg.bind_addr);

    // UDP voice relay
    let udp_socket = Arc::new(UdpSocket::bind(format!("{}:{voice_port}", cfg.bind_addr)).await?);
    info!("UDP voice relay on {}:{voice_port}", cfg.bind_addr);

    // Spawn UDP voice relay task
    let rooms_voice = rooms.clone();
    let udp = udp_socket.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; voice::MAX_PACKET_SIZE];
        let mut pkt_count: u64 = 0;
        loop {
            match udp.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    if let Some((room_id, user_id, seq)) = voice::decode_header(&buf[..len]) {
                        let registered = rooms_voice
                            .register_voice_addr(room_id, user_id, addr)
                            .await;

                        let peers = rooms_voice.get_voice_peers(room_id, user_id).await;

                        pkt_count += 1;
                        if pkt_count % 250 == 1 {
                            info!(
                                "UDP: pkt#{pkt_count} from {addr} room={room_id} user={user_id} seq={seq} len={len} registered={registered} peers={}",
                                peers.len()
                            );
                        }

                        for peer_addr in peers {
                            if let Err(e) = udp.send_to(&buf[..len], peer_addr).await {
                                warn!("UDP forward to {peer_addr} failed: {e}");
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("UDP recv error: {e}");
                }
            }
        }
    });

    // TCP accept loop
    loop {
        let (stream, addr) = tcp_listener.accept().await?;
        info!("connection from {addr}");

        let rooms = rooms.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, rooms, voice_port).await {
                warn!("client {addr} error: {e}");
            }
            info!("client {addr} disconnected");
        });
    }
}

async fn handle_client(
    stream: tokio::net::TcpStream,
    rooms: Arc<RoomManager>,
    voice_port: u16,
) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut writer = writer;

    // First message must be a Join
    let msg: ClientMessage = protocol::read_message(&mut reader).await?;
    let (username, room_name, user_id) = match msg {
        ClientMessage::Join {
            username,
            room,
            password,
        } => {
            match rooms.join(&room, &username, password.as_deref()).await {
                Ok((users, user_id, room_id, user_ids)) => {
                    let state = ServerMessage::RoomState {
                        room: room.clone(),
                        users,
                        user_id,
                        room_id,
                        voice_port,
                        user_ids,
                    };
                    protocol::write_message(&mut writer, &state).await?;
                    (username, room, user_id)
                }
                Err(e) => {
                    let err = ServerMessage::Error {
                        message: e.to_string(),
                    };
                    protocol::write_message(&mut writer, &err).await?;
                    return Ok(());
                }
            }
        }
        _ => {
            let err = ServerMessage::Error {
                message: "must join a room first".into(),
            };
            protocol::write_message(&mut writer, &err).await?;
            return Ok(());
        }
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();
    let direct_tx = tx.clone();
    rooms.subscribe(&room_name, &username, user_id, tx).await;

    rooms
        .broadcast(
            &room_name,
            &username,
            ServerMessage::UserJoined {
                username: username.clone(),
                user_id: Some(user_id),
            },
        )
        .await;

    let write_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if protocol::write_message(&mut writer, &msg).await.is_err() {
                break;
            }
        }
    });

    loop {
        match protocol::read_message::<_, ClientMessage>(&mut reader).await {
            Ok(ClientMessage::Chat { text }) => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                rooms
                    .broadcast(
                        &room_name,
                        &username,
                        ServerMessage::Chat {
                            from: username.clone(),
                            text,
                            ts,
                        },
                    )
                    .await;
            }
            Ok(ClientMessage::Ping { ts }) => {
                let _ = direct_tx.send(ServerMessage::Pong { ts });
            }
            Ok(ClientMessage::Leave) => break,
            Ok(ClientMessage::Join { .. }) => {}
            Err(_) => break,
        }
    }

    rooms.leave(&room_name, &username).await;
    rooms
        .broadcast(
            &room_name,
            &username,
            ServerMessage::UserLeft {
                username: username.clone(),
            },
        )
        .await;

    write_handle.abort();
    Ok(())
}
