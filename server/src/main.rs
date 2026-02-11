mod room;

use anyhow::Result;
use room::RoomManager;
use shared::{ClientMessage, ServerMessage, protocol};
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::TcpListener;
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

    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("listening on 0.0.0.0:{port}");

    let rooms = Arc::new(RoomManager::new());

    loop {
        let (stream, addr) = listener.accept().await?;
        info!("connection from {addr}");

        let rooms = rooms.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, rooms).await {
                warn!("client {addr} error: {e}");
            }
            info!("client {addr} disconnected");
        });
    }
}

async fn handle_client(
    stream: tokio::net::TcpStream,
    rooms: Arc<RoomManager>,
) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut writer = writer;

    // First message must be a Join
    let msg: ClientMessage = protocol::read_message(&mut reader).await?;
    let (username, room_name) = match msg {
        ClientMessage::Join {
            username,
            room,
            password,
        } => {
            match rooms.join(&room, &username, password.as_deref()).await {
                Ok(users) => {
                    let state = ServerMessage::RoomState {
                        room: room.clone(),
                        users,
                    };
                    protocol::write_message(&mut writer, &state).await?;
                    (username, room)
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

    // Channel for outbound messages to this client
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();
    rooms.subscribe(&room_name, &username, tx).await;

    // Broadcast join to others
    rooms
        .broadcast(
            &room_name,
            &username,
            ServerMessage::UserJoined {
                username: username.clone(),
            },
        )
        .await;

    // Writer task — sends queued messages to the TCP stream
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if protocol::write_message(&mut writer, &msg).await.is_err() {
                break;
            }
        }
    });

    // Read loop — process incoming messages
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
            Ok(ClientMessage::Join { .. }) => {
                // Already in a room — ignore
            }
            Err(_) => break,
        }
    }

    // Cleanup
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
