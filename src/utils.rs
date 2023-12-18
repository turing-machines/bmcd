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
mod event_listener;
mod io;

use anyhow::bail;
use std::time::{SystemTime, UNIX_EPOCH};

#[doc(inline)]
pub use event_listener::*;
pub use io::*;
use std::{path::PathBuf, process::Output};
use tokio::io::AsyncBufReadExt;

pub fn string_from_utf16(bytes: &[u8], little_endian: bool) -> String {
    let u16s = bytes.chunks_exact(2).map(|pair| {
        let Ok(owned) = pair.try_into() else {
            unreachable!()
        };

        if little_endian {
            u16::from_le_bytes(owned)
        } else {
            u16::from_be_bytes(owned)
        }
    });

    let mut string = char::decode_utf16(u16s)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect::<String>();

    if bytes.len() % 2 == 1 {
        string.push(char::REPLACEMENT_CHARACTER)
    }

    string
}

pub fn string_from_utf32(bytes: &[u8], little_endian: bool) -> String {
    bytes
        .chunks(4)
        .map(|slice| {
            let Ok(owned) = slice.try_into() else {
                return char::REPLACEMENT_CHARACTER;
            };

            let scalar = if little_endian {
                u32::from_le_bytes(owned)
            } else {
                u32::from_be_bytes(owned)
            };

            char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect()
}

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

/// Get current time in seconds since Unix epoch. Returns `None` if current time is before epoch.
pub fn get_timestamp_unix() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|x| x.as_secs())
}

pub async fn logging_sink_stdio(output: &Output) -> std::io::Result<()> {
    let mut lines = output.stdout.lines();
    while let Some(line) = lines.next_line().await? {
        log::info!("{}", line);
    }

    let mut lines = output.stderr.lines();
    while let Some(line) = lines.next_line().await? {
        log::error!("{}", line);
    }
    Ok(())
}
