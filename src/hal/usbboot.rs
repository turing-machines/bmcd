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
use anyhow::bail;
use std::path::PathBuf;

pub async fn get_device_path(allowed_vendors: &[&str]) -> anyhow::Result<PathBuf> {
    let mut contents = tokio::fs::read_dir("/sys/block/").await.map_err(|err| {
        std::io::Error::new(err.kind(), format!("Failed to list devices: {}", err))
    })?;

    let mut matching_devices = vec![];

    while let Some(entry) = contents.next_entry().await.map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!("Intermittent IO error while listing devices: {}", err),
        )
    })? {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        let vendor_path = format!("/sys/block/{}/device/vendor", file_name);
        let Ok(vendor) = tokio::fs::read_to_string(vendor_path).await else {
            continue;
        };
        let vendor = vendor.trim();

        for allowed_vendor in allowed_vendors {
            if vendor == *allowed_vendor {
                matching_devices.push(file_name.clone());
            }
        }
    }

    let name = match &matching_devices[..] {
        [] => {
            bail!("No supported USB devices found");
        }
        [device] => device.clone(),
        _ => {
            bail!("Several supported devices found");
        }
    };

    Ok(tokio::fs::canonicalize(format!("/dev/{}", name)).await?)
}
