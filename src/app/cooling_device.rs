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

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CoolingDevice {
    pub device: String,
    pub speed: u8,
    pub max_speed: u8,
}

pub async fn get_cooling_state() -> Vec<CoolingDevice> {
    let mut result = Vec::new();

    if let Ok(mut dir) = tokio::fs::read_dir("/sys/class/thermal").await {
        while let Some(device) = dir.next_entry().await.unwrap_or(None) {
            let device_name = device.file_name().to_string_lossy().into_owned();
            if !device_name.starts_with("cooling_device") {
                continue;
            }

            let device_path = device.path();

            let cur_state_path = device_path.join("cur_state");
            let max_state_path = device_path.join("max_state");

            let cur_state = match tokio::fs::read_to_string(cur_state_path).await {
                Ok(state) => state.trim().parse::<u8>().unwrap_or(0),
                Err(err) => {
                    eprintln!("Error reading cur_state file: {}", err);
                    0
                }
            };

            let max_state = match tokio::fs::read_to_string(max_state_path).await {
                Ok(max_speed) => max_speed.trim().parse::<u8>().unwrap_or(0),
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

pub async fn set_cooling_state(device: &str, speed: &u8) -> anyhow::Result<()> {
    let device_path = Path::new("/sys/class/thermal").join(device).join("cur_state");

    tokio::fs::write(device_path, speed.to_string()).await?;

    Ok(())
}
