use cpal::traits::{DeviceTrait, HostTrait};

#[derive(Clone)]
pub struct DeviceInfo {
    pub name: String,
    /// None = system default, Some(n) = nth device
    pub index: Option<usize>,
}

pub fn list_input_devices() -> Vec<DeviceInfo> {
    let mut devices = vec![DeviceInfo {
        name: "System Default".to_string(),
        index: None,
    }];
    let host = cpal::default_host();
    if let Ok(iter) = host.input_devices() {
        for (i, d) in iter.enumerate() {
            if let Ok(name) = d.name() {
                devices.push(DeviceInfo {
                    name,
                    index: Some(i),
                });
            }
        }
    }
    devices
}

pub fn list_output_devices() -> Vec<DeviceInfo> {
    let mut devices = vec![DeviceInfo {
        name: "System Default".to_string(),
        index: None,
    }];
    let host = cpal::default_host();
    if let Ok(iter) = host.output_devices() {
        for (i, d) in iter.enumerate() {
            if let Ok(name) = d.name() {
                devices.push(DeviceInfo {
                    name,
                    index: Some(i),
                });
            }
        }
    }
    devices
}

pub fn get_input_device(index: Option<usize>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    match index {
        None => host.default_input_device(),
        Some(i) => host.input_devices().ok()?.nth(i),
    }
}

pub fn get_output_device(index: Option<usize>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    match index {
        None => host.default_output_device(),
        Some(i) => host.output_devices().ok()?.nth(i),
    }
}
