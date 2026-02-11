use anyhow::{bail, Result};
use shared::{ServerMessage, MAX_ROOM_USERS};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{RwLock, mpsc};
use tracing::{info, warn};

static NEXT_ROOM_ID: AtomicU16 = AtomicU16::new(1);
static NEXT_USER_ID: AtomicU16 = AtomicU16::new(1);

pub struct VoiceEndpoint {
    pub addr: SocketAddr,
    /// A dedicated UDP socket `connect()`ed to this client's address.
    /// Sending through this looks like a reply to the OS/firewall.
    pub socket: Arc<UdpSocket>,
}

pub struct Member {
    pub tx: mpsc::UnboundedSender<ServerMessage>,
    pub user_id: u16,
    /// Set once the client sends its first UDP packet
    pub voice: Option<VoiceEndpoint>,
}

struct Room {
    room_id: u16,
    password: Option<String>,
    members: HashMap<String, Member>,
}

impl Room {
    fn new(room_id: u16, password: Option<String>) -> Self {
        Self {
            room_id,
            password,
            members: HashMap::new(),
        }
    }
}

pub struct RoomManager {
    rooms: RwLock<HashMap<String, Room>>,
}

impl RoomManager {
    pub fn new() -> Self {
        Self {
            rooms: RwLock::new(HashMap::new()),
        }
    }

    /// Join a room. Returns (existing usernames, assigned user_id, room_id).
    pub async fn join(
        &self,
        room_name: &str,
        username: &str,
        password: Option<&str>,
    ) -> Result<(Vec<String>, u16, u16)> {
        let mut rooms = self.rooms.write().await;
        let room = rooms
            .entry(room_name.to_string())
            .or_insert_with(|| {
                let rid = NEXT_ROOM_ID.fetch_add(1, Ordering::Relaxed);
                Room::new(rid, password.map(|s| s.to_string()))
            });

        // Check password
        if let Some(ref room_pass) = room.password {
            match password {
                Some(p) if p == room_pass => {}
                _ => bail!("incorrect room password"),
            }
        }

        if room.members.len() >= MAX_ROOM_USERS {
            bail!("room is full (max {MAX_ROOM_USERS})");
        }

        if room.members.contains_key(username) {
            bail!("username '{username}' already taken in this room");
        }

        let users: Vec<String> = room.members.keys().cloned().collect();
        let user_id = NEXT_USER_ID.fetch_add(1, Ordering::Relaxed);
        let room_id = room.room_id;

        info!(
            "{username} joined room '{room_name}' (id={room_id}, uid={user_id}, {} users)",
            users.len() + 1
        );

        Ok((users, user_id, room_id))
    }

    pub async fn subscribe(
        &self,
        room_name: &str,
        username: &str,
        user_id: u16,
        tx: mpsc::UnboundedSender<ServerMessage>,
    ) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_name) {
            room.members.insert(
                username.to_string(),
                Member {
                    tx,
                    user_id,
                    voice: None,
                },
            );
        }
    }

    pub async fn leave(&self, room_name: &str, username: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_name) {
            room.members.remove(username);
            info!(
                "{username} left room '{room_name}' ({} users)",
                room.members.len()
            );
            if room.members.is_empty() {
                rooms.remove(room_name);
                info!("room '{room_name}' removed (empty)");
            }
        }
    }

    pub async fn broadcast(&self, room_name: &str, sender: &str, msg: ServerMessage) {
        let rooms = self.rooms.read().await;
        if let Some(room) = rooms.get(room_name) {
            for (name, member) in &room.members {
                if name != sender {
                    let _ = member.tx.send(msg.clone());
                }
            }
        }
    }

    /// Register a voice address for a user (called on first UDP packet).
    /// Creates a per-client connected UDP socket for NAT/firewall traversal.
    pub async fn register_voice_addr(
        &self,
        room_id: u16,
        user_id: u16,
        addr: SocketAddr,
    ) -> bool {
        let mut rooms = self.rooms.write().await;
        for room in rooms.values_mut() {
            if room.room_id == room_id {
                for member in room.members.values_mut() {
                    if member.user_id == user_id {
                        // If already registered to the same addr, skip
                        if let Some(ref ep) = member.voice {
                            if ep.addr == addr {
                                return true;
                            }
                        }
                        // Create a new connected socket for this client
                        match create_connected_socket(addr).await {
                            Ok(sock) => {
                                info!("Created connected UDP socket for user {user_id} -> {addr}");
                                member.voice = Some(VoiceEndpoint {
                                    addr,
                                    socket: Arc::new(sock),
                                });
                                return true;
                            }
                            Err(e) => {
                                warn!("Failed to create connected socket for {addr}: {e}");
                                return false;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Get all voice endpoints in a room except the sender.
    /// Returns (addr, connected_socket) pairs.
    pub async fn get_voice_peers(
        &self,
        room_id: u16,
        sender_user_id: u16,
    ) -> Vec<(SocketAddr, Arc<UdpSocket>)> {
        let rooms = self.rooms.read().await;
        for room in rooms.values() {
            if room.room_id == room_id {
                return room
                    .members
                    .values()
                    .filter(|m| m.user_id != sender_user_id && m.voice.is_some())
                    .map(|m| {
                        let ep = m.voice.as_ref().unwrap();
                        (ep.addr, ep.socket.clone())
                    })
                    .collect();
            }
        }
        Vec::new()
    }
}

/// Create a UDP socket bound to an ephemeral port and `connect()`ed to the
/// target address. Sending on this socket looks like a reply to the client's
/// OS/firewall/NAT, making it far more likely to pass through.
async fn create_connected_socket(target: SocketAddr) -> Result<UdpSocket> {
    let bind_addr = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = UdpSocket::bind(bind_addr).await?;
    sock.connect(target).await?;
    Ok(sock)
}
