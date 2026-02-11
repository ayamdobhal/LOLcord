use anyhow::Result;
use shared::{ClientMessage, ServerMessage, protocol};
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{error, info};

pub struct Connection {
    pub server_rx: mpsc::UnboundedReceiver<ServerMessage>,
    pub client_tx: mpsc::UnboundedSender<ClientMessage>,
}

impl Connection {
    /// Connect to the server and spawn read/write tasks on the tokio runtime.
    pub async fn connect(
        addr: &str,
        join_msg: ClientMessage,
    ) -> Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        info!("connected to {addr}");

        let (reader, writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut writer = writer;

        // Send join message
        protocol::write_message(&mut writer, &join_msg).await?;

        // Server → client channel
        let (server_tx, server_rx) = mpsc::unbounded_channel();
        // Client → server channel
        let (client_tx, mut client_rx) = mpsc::unbounded_channel();

        // Read task
        tokio::spawn(async move {
            loop {
                match protocol::read_message::<_, ServerMessage>(&mut reader).await {
                    Ok(msg) => {
                        if server_tx.send(msg).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("read error: {e}");
                        break;
                    }
                }
            }
        });

        // Write task
        tokio::spawn(async move {
            while let Some(msg) = client_rx.recv().await {
                if protocol::write_message(&mut writer, &msg).await.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            server_rx,
            client_tx,
        })
    }
}
