use cpal::traits::{DeviceTrait, HostTrait};

#[derive(Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub index: usize,
}

pub fn list_input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| {
            devices
                .enumerate()
                .filter_map(|(i, d)| {
                    Some(DeviceInfo {
                        name: d.name().ok()?,
                        index: i,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|devices| {
            devices
                .enumerate()
                .filter_map(|(i, d)| {
                    Some(DeviceInfo {
                        name: d.name().ok()?,
                        index: i,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn get_input_device(index: usize) -> Option<cpal::Device> {
    let host = cpal::default_host();
    host.input_devices().ok()?.nth(index)
}

pub fn get_output_device(index: usize) -> Option<cpal::Device> {
    let host = cpal::default_host();
    host.output_devices().ok()?.nth(index)
}
