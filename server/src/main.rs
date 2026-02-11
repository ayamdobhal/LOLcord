mod room;

use anyhow::Result;
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

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(shared::DEFAULT_PORT);
    let voice_port = std::env::var("VOICE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(shared::DEFAULT_VOICE_PORT);

    let rooms = Arc::new(RoomManager::new());

    // TCP signaling
    let tcp_listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("TCP signaling on 0.0.0.0:{port}");

    // UDP voice relay
    let udp_socket = Arc::new(UdpSocket::bind(format!("0.0.0.0:{voice_port}")).await?);
    info!("UDP voice relay on 0.0.0.0:{voice_port}");

    // Spawn UDP voice relay task
    let rooms_voice = rooms.clone();
    let udp = udp_socket.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; voice::MAX_PACKET_SIZE];
        loop {
            match udp.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    if let Some((room_id, user_id, _seq)) = voice::decode_header(&buf[..len]) {
                        // Register sender's address on first packet
                        rooms_voice
                            .register_voice_addr(room_id, user_id, addr)
                            .await;

                        // Forward to all peers in the room
                        let peers = rooms_voice.get_voice_peers(room_id, user_id).await;
                        for peer_addr in peers {
                            let _ = udp.send_to(&buf[..len], peer_addr).await;
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
                Ok((users, user_id, room_id)) => {
                    let state = ServerMessage::RoomState {
                        room: room.clone(),
                        users,
                        user_id,
                        room_id,
                        voice_port,
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
    rooms.subscribe(&room_name, &username, user_id, tx).await;

    rooms
        .broadcast(
            &room_name,
            &username,
            ServerMessage::UserJoined {
                username: username.clone(),
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
