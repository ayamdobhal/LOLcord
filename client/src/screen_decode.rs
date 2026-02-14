use anyhow::{anyhow, Result};
use openh264::{decoder::Decoder};
use shared::screen;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Fragment of a screen frame
#[derive(Debug, Clone)]
struct Fragment {
    data: Vec<u8>,
    fragment_idx: u16,
    received_at: Instant,
}

/// Complete frame being assembled
#[derive(Debug)]
struct FrameAssembly {
    frame_id: u32,
    is_keyframe: bool,
    expected_fragments: u16,
    fragments: HashMap<u16, Fragment>,
    first_fragment_time: Instant,
}

impl FrameAssembly {
    fn new(frame_id: u32, is_keyframe: bool, expected_fragments: u16) -> Self {
        Self {
            frame_id,
            is_keyframe,
            expected_fragments,
            fragments: HashMap::new(),
            first_fragment_time: Instant::now(),
        }
    }
    
    fn add_fragment(&mut self, fragment_idx: u16, data: Vec<u8>) {
        self.fragments.insert(fragment_idx, Fragment {
            data,
            fragment_idx,
            received_at: Instant::now(),
        });
    }
    
    fn is_complete(&self) -> bool {
        self.fragments.len() == self.expected_fragments as usize
    }
    
    fn is_expired(&self, timeout: Duration) -> bool {
        self.first_fragment_time.elapsed() > timeout
    }
    
    fn assemble(&self) -> Option<Vec<u8>> {
        if !self.is_complete() {
            return None;
        }
        
        let mut result = Vec::new();
        for i in 0..self.expected_fragments {
            if let Some(fragment) = self.fragments.get(&i) {
                result.extend_from_slice(&fragment.data);
            } else {
                return None; // Missing fragment
            }
        }
        
        Some(result)
    }
}

/// Screen decoder state
pub struct ScreenDecoder {
    decoder: Mutex<Decoder>,
    frame_assembly: Mutex<HashMap<u32, FrameAssembly>>,
    decoded_frames_tx: mpsc::UnboundedSender<(u16, Vec<u8>)>, // (user_id, rgba_data)
    decoded_frames_rx: Arc<Mutex<mpsc::UnboundedReceiver<(u16, Vec<u8>)>>>,
}

impl ScreenDecoder {
    pub fn new() -> Result<Self> {
        let decoder = Decoder::new().map_err(|e| anyhow!("Failed to create decoder: {:?}", e))?;
        let (decoded_frames_tx, decoded_frames_rx) = mpsc::unbounded_channel();
        
        Ok(Self {
            decoder: Mutex::new(decoder),
            frame_assembly: Mutex::new(HashMap::new()),
            decoded_frames_tx,
            decoded_frames_rx: Arc::new(Mutex::new(decoded_frames_rx)),
        })
    }
    
    /// Process incoming screen packet
    pub async fn process_packet(&self, packet: &[u8]) -> Result<()> {
        let header = screen::decode_header(packet)
            .ok_or_else(|| anyhow!("Invalid screen packet header"))?;
        
        let (_room_id, user_id, frame_id, fragment_idx, fragment_count, is_keyframe) = header;
        let payload = &packet[screen::HEADER_SIZE..];
        
        // Add fragment to assembly
        self.add_fragment(frame_id, fragment_idx, fragment_count, is_keyframe, payload.to_vec()).await;
        
        // Try to assemble and decode complete frames
        self.try_decode_frames(user_id).await;
        
        // Cleanup expired frames
        self.cleanup_expired_frames().await;
        
        Ok(())
    }
    
    /// Get latest decoded frame for a user (non-blocking)
    pub fn get_decoded_frame(&self) -> Option<(u16, Vec<u8>)> {
        self.decoded_frames_rx.lock().ok()?.try_recv().ok()
    }
    
    async fn add_fragment(
        &self,
        frame_id: u32,
        fragment_idx: u16,
        fragment_count: u16,
        is_keyframe: bool,
        data: Vec<u8>,
    ) {
        let mut assembly = self.frame_assembly.lock().unwrap();
        
        let frame_assembly = assembly.entry(frame_id).or_insert_with(|| {
            debug!("Starting assembly for frame {} ({} fragments)", frame_id, fragment_count);
            FrameAssembly::new(frame_id, is_keyframe, fragment_count)
        });
        
        frame_assembly.add_fragment(fragment_idx, data);
        
        debug!(
            "Fragment {}/{} for frame {} ({})",
            fragment_idx + 1,
            fragment_count,
            frame_id,
            if is_keyframe { "keyframe" } else { "delta" }
        );
    }
    
    async fn try_decode_frames(&self, user_id: u16) {
        let complete_frames: Vec<(u32, Vec<u8>, bool)> = {
            let mut assembly = self.frame_assembly.lock().unwrap();
            let mut complete = Vec::new();
            
            // Find complete frames
            for (frame_id, frame) in assembly.iter() {
                if frame.is_complete() {
                    if let Some(data) = frame.assemble() {
                        complete.push((*frame_id, data, frame.is_keyframe));
                    }
                }
            }
            
            // Remove complete frames from assembly map
            for (frame_id, _, _) in &complete {
                assembly.remove(frame_id);
            }
            
            complete
        };
        
        // Decode complete frames
        for (frame_id, h264_data, is_keyframe) in complete_frames {
            if let Err(e) = self.decode_frame(user_id, frame_id, &h264_data, is_keyframe).await {
                warn!("Failed to decode frame {}: {}", frame_id, e);
            }
        }
    }
    
    async fn decode_frame(&self, user_id: u16, frame_id: u32, h264_data: &[u8], is_keyframe: bool) -> Result<()> {
        // Process decoding entirely within the lock to avoid borrow issues
        {
            let mut decoder_guard = self.decoder.lock().unwrap();
            let decode_result = decoder_guard.decode(h264_data);
            
            match decode_result {
                Ok(Some(_yuv)) => {
                    debug!("Decoded frame {} ({})", frame_id, if is_keyframe { "keyframe" } else { "delta" });
                    
                    // For now, create a placeholder RGBA frame
                    // TODO: Implement proper YUV to RGBA conversion once we understand the API
                    let width = 1280usize;
                    let height = 720usize;
                    let rgba_data = vec![128u8; width * height * 4]; // Gray placeholder
                    
                    let _ = self.decoded_frames_tx.send((user_id, rgba_data));
                }
                Ok(None) => {
                    debug!("Frame {} decoded but no output (incomplete)", frame_id);
                }
                Err(e) => {
                    warn!("H.264 decode error for frame {}: {:?}", frame_id, e);
                }
            }
        }
        
        Ok(())
    }
    
    async fn cleanup_expired_frames(&self) {
        let timeout = Duration::from_millis(100); // 100ms timeout for incomplete frames
        let mut assembly = self.frame_assembly.lock().unwrap();
        
        let expired_frames: Vec<u32> = assembly
            .iter()
            .filter(|(_, frame)| frame.is_expired(timeout))
            .map(|(frame_id, _)| *frame_id)
            .collect();
        
        for frame_id in expired_frames {
            debug!("Dropping expired frame {}", frame_id);
            assembly.remove(&frame_id);
        }
    }
}

// YUV to RGBA conversion - placeholder for now
// TODO: Implement proper conversion once we understand the openh264 API