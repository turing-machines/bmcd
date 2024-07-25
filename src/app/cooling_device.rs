// Copyright 2023 Turing Machines
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{anyhow, bail};
use serde::Serialize;
use std::{ffi::c_ulong, fs, io, path::Path};
use tracing::{instrument, warn};

#[derive(Debug, Serialize)]
pub struct CoolingDevice {
    pub device: String,
    pub speed: c_ulong,
    pub max_speed: c_ulong,
}

pub async fn get_cooling_state() -> Vec<CoolingDevice> {
    let mut result = Vec::new();

    if let Ok(mut dir) = tokio::fs::read_dir("/sys/class/thermal").await {
        while let Some(device) = dir.next_entry().await.unwrap_or(None) {
            let mut device_name = device.file_name().to_string_lossy().into_owned();
            if !device_name.starts_with("cooling_device") {
                continue;
            }

            let device_path = device.path();
            if let Some(name) = is_system_fan(&device_path).map(|n| n.replace('-', " ")) {
                device_name = name;
            }

            let cur_state_path = device_path.join("cur_state");
            let max_state_path = device_path.join("max_state");

            let cur_state = match tokio::fs::read_to_string(cur_state_path).await {
                Ok(state) => state.trim().parse::<c_ulong>().unwrap_or(0),
                Err(err) => {
                    eprintln!("Error reading cur_state file: {}", err);
                    0
                }
            };

            let max_state = match tokio::fs::read_to_string(max_state_path).await {
                Ok(max_speed) => max_speed.trim().parse::<c_ulong>().unwrap_or(0),
                Err(err) => {
                    eprintln!("Error reading max_state file: {}", err);
                    0
                }
            };

            result.push(CoolingDevice {
                device: device_name,
                speed: cur_state,
                max_speed: max_state,
            });
        }
    }

    result
}

#[instrument(level = "debug", ret)]
fn is_system_fan(dev_path: &Path) -> Option<String> {
    let typ = std::fs::read_to_string(dev_path.join("type")).ok()?;
    if typ.trim() == "pwm-fan" {
        let pwm_fan_nodes = get_pwm_fan_nodes().ok()?;
        if pwm_fan_nodes.len() > 1 {
            warn!("more as one pwm-fan device detected, selecting first for system_fan");
        }
        return pwm_fan_nodes.first().cloned();
    }
    None
}

#[instrument(level = "debug", ret)]
fn get_pwm_fan_nodes() -> io::Result<Vec<String>> {
    let mut nodes = Vec::new();

    for entry in fs::read_dir("/sys/bus/platform/drivers/pwm-fan")? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                nodes.push(name.to_string());
            }
        }
    }

    Ok(nodes)
}

#[instrument(err)]
pub async fn set_cooling_state(device: &str, speed: &c_ulong) -> anyhow::Result<()> {
    // quick and dirty workaround
    let dev_name = if device == "system fan" {
        "cooling_device0"
    } else {
        device
    };

    let device_path = Path::new("/sys/class/thermal")
        .join(dev_name)
        .join("cur_state");

    let devices = get_cooling_state().await;
    let device = devices
        .iter()
        .find(|d| d.device == device)
        .ok_or(anyhow!("cooling device: `{}` does not exist", device))?;

    if speed > &device.max_speed {
        bail!(
            "given speed '{}' exceeds maximum speed of '{}'",
            speed,
            device.max_speed
        );
    }

    tokio::fs::write(device_path, speed.to_string()).await?;
    Ok(())
}
