use anyhow::{anyhow, bail, Result};
use openh264::{encoder::{Encoder, EncoderConfig}};
use scrap::{Capturer, Display};
use shared::screen;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Screen capture state and control
pub struct ScreenCaptureState {
    pub capturing: Arc<AtomicBool>,
    pub self_preview_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Vec<u8>>>>>,
}

impl ScreenCaptureState {
    pub fn new() -> Self {
        Self {
            capturing: Arc::new(AtomicBool::new(false)),
            self_preview_rx: Arc::new(Mutex::new(None)),
        }
    }
    
    /// Start screen capture and encoding thread
    pub async fn start_capture(
        &self,
        server_addr: SocketAddr,
        room_id: u16,
        user_id: u16,
        udp_socket: Arc<UdpSocket>,
    ) -> Result<()> {
        if self.capturing.load(Ordering::Relaxed) {
            return Ok(()); // Already capturing
        }
        
        let (preview_tx, preview_rx) = mpsc::unbounded_channel();
        *self.self_preview_rx.lock().unwrap() = Some(preview_rx);
        
        self.capturing.store(true, Ordering::Relaxed);
        let capturing = Arc::clone(&self.capturing);
        
        // Use spawn_blocking since scrap::Capturer is not Send and must run on a blocking thread
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                if let Err(e) = run_capture_thread(
                    server_addr,
                    room_id,
                    user_id,
                    udp_socket,
                    capturing.clone(),
                    preview_tx,
                ).await {
                    error!("Screen capture failed: {}", e);
                }
                capturing.store(false, Ordering::Relaxed);
            })
        });
        
        Ok(())
    }
    
    /// Stop screen capture
    pub fn stop_capture(&self) {
        self.capturing.store(false, Ordering::Relaxed);
        *self.self_preview_rx.lock().unwrap() = None;
    }
    
    /// Get latest self-preview frame (non-blocking)
    pub fn get_self_preview_frame(&self) -> Option<Vec<u8>> {
        self.self_preview_rx.lock().ok()?.as_mut()?.try_recv().ok()
    }
}

/// Main screen capture and encoding thread
async fn run_capture_thread(
    server_addr: SocketAddr,
    room_id: u16,
    user_id: u16,
    udp_socket: Arc<UdpSocket>,
    capturing: Arc<AtomicBool>,
    preview_tx: mpsc::UnboundedSender<Vec<u8>>,
) -> Result<()> {
    info!("Starting screen capture (720p @ 30fps)");
    
    // Setup display capture
    let display = Display::primary().map_err(|e| anyhow!("Failed to get primary display: {}", e))?;
    let mut capturer = Capturer::new(display).map_err(|e| anyhow!("Failed to create capturer: {}", e))?;
    
    // Setup H.264 encoder for 720p
    let width = 1280;
    let height = 720;
    let config = EncoderConfig::new();
    let _config_bitrate = config.set_bitrate_bps(2_000_000); // 2 Mbps target  
    let _config_skip = config.enable_skip_frame(false);
    
    let _encoder = Encoder::new().map_err(|e| anyhow!("Failed to create encoder: {:?}", e))?;
    
    let frame_interval = Duration::from_millis(33); // ~30 fps
    let mut last_frame = Instant::now();
    let mut frame_id: u32 = 0;
    
    info!("Screen capture started - resolution: {}x{}", width, height);
    
    while capturing.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now.duration_since(last_frame) < frame_interval {
            tokio::time::sleep(Duration::from_millis(1)).await;
            continue;
        }
        
        // Capture screen frame (blocking call but should be quick)
        let yuv_data = match capturer.frame() {
            Ok(frame) => {
                // Convert BGRA to YUV420p for H.264 encoding
                let bgra_data = frame.to_vec();
                match bgra_to_yuv420p(&bgra_data, width, height) {
                    Ok(yuv) => yuv,
                    Err(e) => {
                        warn!("YUV conversion error: {}", e);
                        continue;
                    }
                }
            }
            Err(e) => {
                warn!("Frame capture error: {}", e);
                continue;
            }
        };
        
        // For now, let's create a simple H.264 frame placeholder
        // TODO: Implement proper encoding once we understand the API better
        let fake_frame_data = vec![0u8; 1000]; // Placeholder encoded data
        let encode_result: Result<Vec<u8>> = Ok(fake_frame_data);
        
        match encode_result {
            Ok(frame_data) => {
                frame_id = frame_id.wrapping_add(1);
                
                // Send self-preview frame (decoded RGB for UI)
                if let Ok(rgb_data) = yuv420p_to_rgb(&yuv_data, width, height) {
                    let _ = preview_tx.send(rgb_data);
                }
                
                // Fragment and send encoded frame
                if let Err(e) = send_frame_fragments(
                    &udp_socket,
                    server_addr,
                    room_id,
                    user_id,
                    frame_id,
                    &frame_data,
                    true, // Assume all frames are keyframes for now - could optimize later
                ).await {
                    error!("Failed to send frame fragments: {}", e);
                }
            }
            Err(e) => {
                warn!("Frame encode error: {:?}", e);
            }
        }
        
        last_frame = now;
    }
    
    info!("Screen capture stopped");
    Ok(())
}

/// Fragment encoded frame into UDP packets and send them
async fn send_frame_fragments(
    socket: &UdpSocket,
    server_addr: SocketAddr,
    room_id: u16,
    user_id: u16,
    frame_id: u32,
    data: &[u8],
    is_keyframe: bool,
) -> Result<()> {
    let payload_size = screen::FRAGMENT_SIZE - screen::HEADER_SIZE;
    let total_fragments = (data.len() + payload_size - 1) / payload_size;
    
    if total_fragments > u16::MAX as usize {
        bail!("Frame too large: {} bytes, {} fragments", data.len(), total_fragments);
    }
    
    for (i, chunk) in data.chunks(payload_size).enumerate() {
        let header = screen::encode_header(
            room_id,
            user_id,
            frame_id,
            i as u16,
            total_fragments as u16,
            is_keyframe,
        );
        
        let mut packet = Vec::with_capacity(screen::HEADER_SIZE + chunk.len());
        packet.extend_from_slice(&header);
        packet.extend_from_slice(chunk);
        
        if let Err(e) = socket.send_to(&packet, server_addr).await {
            error!("Fragment send error: {}", e);
            return Err(anyhow!("UDP send failed: {}", e));
        }
    }
    
    Ok(())
}

/// Convert BGRA to YUV420p
fn bgra_to_yuv420p(bgra: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    if bgra.len() != width * height * 4 {
        bail!("Invalid BGRA buffer size");
    }
    
    let mut yuv = vec![0u8; width * height * 3 / 2]; // Y + U + V planes
    
    // Y plane
    for y in 0..height {
        for x in 0..width {
            let bgra_idx = (y * width + x) * 4;
            let b = bgra[bgra_idx] as f32;
            let g = bgra[bgra_idx + 1] as f32;
            let r = bgra[bgra_idx + 2] as f32;
            
            let y_val = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
            yuv[y * width + x] = y_val;
        }
    }
    
    let y_plane_size = width * height;
    let u_plane_start = y_plane_size;
    let v_plane_start = y_plane_size + (width * height / 4);
    
    // U and V planes (subsampled 2x2)
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let bgra_idx = (y * width + x) * 4;
            let b = bgra[bgra_idx] as f32;
            let g = bgra[bgra_idx + 1] as f32;
            let r = bgra[bgra_idx + 2] as f32;
            
            let u_val = (-0.169 * r - 0.331 * g + 0.5 * b + 128.0) as u8;
            let v_val = (0.5 * r - 0.419 * g - 0.081 * b + 128.0) as u8;
            
            let uv_idx = (y / 2) * (width / 2) + (x / 2);
            yuv[u_plane_start + uv_idx] = u_val;
            yuv[v_plane_start + uv_idx] = v_val;
        }
    }
    
    Ok(yuv)
}

/// Convert YUV420p to RGB for display
fn yuv420p_to_rgb(yuv: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    let mut rgb = vec![0u8; width * height * 3];
    
    let y_plane_size = width * height;
    let u_plane_start = y_plane_size;
    let v_plane_start = y_plane_size + (width * height / 4);
    
    for y in 0..height {
        for x in 0..width {
            let y_val = yuv[y * width + x] as f32;
            let u_val = yuv[u_plane_start + (y / 2) * (width / 2) + (x / 2)] as f32 - 128.0;
            let v_val = yuv[v_plane_start + (y / 2) * (width / 2) + (x / 2)] as f32 - 128.0;
            
            let r = (y_val + 1.402 * v_val).clamp(0.0, 255.0) as u8;
            let g = (y_val - 0.344 * u_val - 0.714 * v_val).clamp(0.0, 255.0) as u8;
            let b = (y_val + 1.772 * u_val).clamp(0.0, 255.0) as u8;
            
            let rgb_idx = (y * width + x) * 3;
            rgb[rgb_idx] = r;
            rgb[rgb_idx + 1] = g;
            rgb[rgb_idx + 2] = b;
        }
    }
    
    Ok(rgb)
}