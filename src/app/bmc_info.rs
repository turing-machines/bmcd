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

pub fn get_ipv4_address() -> Option<String> {
    for interface in if_addrs::get_if_addrs().ok()? {
        // NOTE: for compatibility reasons, only IPv4 of eth0 is returned. Ideally, both IPv4 and
        // IPv6 addresses of all non-loopback interfaces should be returned.
        if interface.is_loopback() || interface.name != "eth0" {
            continue;
        }

        let std::net::IpAddr::V4(ip) = interface.ip() else {
            continue;
        };
        return Some(format!("{}", ip));
    }
    None
}

#[derive(Debug, Serialize)]
pub struct NetInfo {
    device: String,
    ip: String,
    mac: String,
}

pub async fn get_net_interfaces() -> Vec<NetInfo> {
    let mut result = Vec::new();
    let Some(interfaces) = if_addrs::get_if_addrs().ok() else {
        return result;
    };

    for interface in interfaces
        .iter()
        .filter(|i| !i.is_loopback() || i.is_link_local())
    {
        result.push(NetInfo {
            device: interface.name.clone(),
            ip: interface.ip().to_string(),
            mac: get_mac_address(&interface.name).await,
        });
    }

    result
}

pub async fn get_mac_address(interface: &str) -> String {
    tokio::fs::read_to_string(format!("/sys/class/net/{}/address", interface))
        .await
        .unwrap_or("Unknown".to_owned())
}

pub fn get_fs_stat(device: &str) -> anyhow::Result<(u64, u64)> {
    let stat = nix::sys::statvfs::statvfs(device)?;
    let total = stat.blocks() as u64 * stat.fragment_size() as u64;
    let free = stat.blocks_free() as u64 * stat.fragment_size() as u64;

    Ok((total, free))
}

#[derive(Debug, Serialize)]
pub struct StorageInfo {
    name: String,
    total_bytes: u64,
    bytes_free: u64,
}

pub fn get_storage_info() -> Vec<StorageInfo> {
    let mut info = vec![];

    if let Ok((total, free)) = get_fs_stat("/") {
        info.push(StorageInfo {
            name: "bmc".to_string(),
            total_bytes: total,
            bytes_free: free,
        });
    }

    if Path::new("/dev/mmcblk0").exists() {
        if let Ok((total, free)) = get_fs_stat("/dev/mmcblk0") {
            info.push(StorageInfo {
                name: "SD card".to_string(),
                total_bytes: total,
                bytes_free: free,
            });
        }
    }
    info
}
