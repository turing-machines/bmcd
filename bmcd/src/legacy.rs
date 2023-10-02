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
//! Routes for legacy API present in versions <= 1.1.0 of the firmware.
use crate::flash_service::FlashService;
use crate::into_legacy_response::LegacyResponse;
use crate::into_legacy_response::{LegacyResult, Null};
use actix_web::guard::{fn_guard, GuardContext};
use actix_web::http::StatusCode;
use actix_web::web::Bytes;
use actix_web::{get, web, HttpRequest, Responder};
use anyhow::Context;
use nix::sys::statfs::statfs;
use serde_json::json;
use std::ops::Deref;
use std::str::FromStr;
use tokio::sync::mpsc;
use tpi_rs::app::bmc_application::{BmcApplication, Encoding, UsbConfig};
use tpi_rs::middleware::{NodeId, UsbMode, UsbRoute};
use tpi_rs::utils::logging_sink;
type Query = web::Query<std::collections::HashMap<String, String>>;

/// version 1:
///
/// * get requests with type&opt queries
///
/// version 1.1:
///
/// * enabled HTTPS
/// * chunked upload of flash images
const API_VERSION: &str = "1.1";

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::resource("/api/bmc")
            .route(
                web::get()
                    .guard(fn_guard(flash_status_guard))
                    .to(handle_flash_status),
            )
            .route(
                web::get()
                    .guard(fn_guard(flash_guard))
                    .to(handle_flash_request),
            )
            .route(web::post().guard(fn_guard(flash_guard)).to(handle_chunk))
            .route(web::get().to(api_entry)),
    );
}

pub fn info_config(cfg: &mut web::ServiceConfig) {
    cfg.service(info_handler);
}

fn flash_status_guard(context: &GuardContext<'_>) -> bool {
    let Some(query) = context.head().uri.query() else { return false; };
    query.contains("status") && query.contains("type=flash") && query.contains("opt=get")
}

fn flash_guard(context: &GuardContext<'_>) -> bool {
    let Some(query) = context.head().uri.query() else { return false; };
    query.contains("opt=set") && query.contains("type=flash")
}

#[get("/api/bmc/info")]
async fn info_handler() -> impl Responder {
    get_system_information().await.into()
}

async fn api_entry(bmc: web::Data<BmcApplication>, query: Query) -> impl Responder {
    let is_set = match query.get("opt").map(String::as_str) {
        Some("set") => true,
        Some("get") => false,
        _ => return LegacyResponse::bad_request("Missing `opt` parameter"),
    };

    let Some(ty) = query.get("type") else {
        return LegacyResponse::bad_request("Missing `type` parameter");
    };

    let bmc = bmc.as_ref();
    match (ty.as_ref(), is_set) {
        ("usb_boot", true) => usb_boot(bmc, query).await.into(),
        ("clear_usb_boot", true) => clear_usb_boot(bmc).into(),
        ("network", true) => reset_network(bmc).await.into(),
        ("nodeinfo", true) => set_node_info().into(),
        ("nodeinfo", false) => get_node_info(bmc).into(),
        ("node_to_msd", true) => set_node_to_msd(bmc, query).await.into(),
        ("other", false) => get_system_information().await.into(),
        ("power", true) => set_node_power(bmc, query).await,
        ("power", false) => get_node_power(bmc).await.into(),
        ("reset", true) => reset_node(bmc, query).await.into(),
        ("sdcard", true) => format_sdcard().into(),
        ("sdcard", false) => get_sdcard_info(),
        ("uart", true) => write_to_uart(bmc, query).await.into(),
        ("uart", false) => read_from_uart(bmc, query).await.into(),
        ("usb", true) => set_usb_mode(bmc, query).await.into(),
        ("usb", false) => get_usb_mode(bmc).await.into(),
        _ => (
            StatusCode::BAD_REQUEST,
            format!("Invalid `type` parameter {}", ty),
        )
            .into(),
    }
}

async fn reset_node(bmc: &BmcApplication, query: Query) -> LegacyResult<()> {
    let node = get_node_param(&query)?;
    Ok(bmc.reset_node(node).await?)
}

async fn usb_boot(bmc: &BmcApplication, query: Query) -> LegacyResult<()> {
    let node = get_node_param(&query)?;
    bmc.usb_boot(node, true).await.map_err(Into::into)
}

fn clear_usb_boot(bmc: &BmcApplication) -> impl Into<LegacyResponse> {
    bmc.clear_usb_boot().context("clear USB boot mode")
}

async fn reset_network(bmc: &BmcApplication) -> impl Into<LegacyResponse> {
    bmc.rtl_reset().await.context("reset network switch")
}

fn set_node_info() -> impl Into<LegacyResponse> {
    // In previous versions of the firmware this was dead code
    LegacyResponse::not_implemented("Method type `set` on parameter `nodeinfo` is deprecated")
}

fn get_node_info(_bmc: &BmcApplication) -> impl Into<LegacyResponse> {
    // TODO: implement serial listening in BmcApplication
    let (n1, n2, n3, n4) = (0, 0, 0, 0);

    json! {
       [{
            "node1": n1,
            "node2": n2,
            "node3": n3,
            "node4": n4,
       }]
    }
}

async fn set_node_to_msd(bmc: &BmcApplication, query: Query) -> LegacyResult<()> {
    let node = get_node_param(&query)?;

    let (tx, rx) = mpsc::channel(64);
    logging_sink(rx);
    bmc.set_node_in_msd(node, UsbRoute::Bmc, tx)
        .await
        .map(|_| ())
        .map_err(Into::into)
}

fn get_node_param(query: &Query) -> LegacyResult<NodeId> {
    let Some(node_str) = query.get("node") else {
        return Err(LegacyResponse::bad_request("Missing `node` parameter"));
    };

    let Ok(node_num) = i32::from_str(node_str) else {
        return Err(LegacyResponse::bad_request(
            "Parameter `node` is not a number",
        ));
    };

    let Ok(node) = node_num.try_into() else {
        return Err(LegacyResponse::bad_request(
            "Parameter `node` is out of range 0..3 of node IDs",
        ));
    };

    Ok(node)
}

async fn get_system_information() -> impl Into<LegacyResponse> {
    let version = env!("CARGO_PKG_VERSION");
    let build_time = build_time::build_time_utc!("%Y-%m-%d %H:%M:%S-00:00");
    let ipv4 = get_ipv4_address().unwrap_or("Unknown".to_owned());
    let mac = get_mac_address().await;

    json!(
        [{
            "api": API_VERSION,
            "version": version,
            "buildtime": build_time,
            "ip": ipv4,
            "mac": mac,
        }]
    )
}

fn get_ipv4_address() -> Option<String> {
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

async fn get_mac_address() -> String {
    tokio::fs::read_to_string("/sys/class/net/eth0/address")
        .await
        .unwrap_or("Unknown".to_owned())
}

async fn set_node_power(bmc: &BmcApplication, query: Query) -> LegacyResponse {
    let mut mask = 0;
    let mut states = 0;

    for idx in 0..4 {
        let param = format!("node{}", idx + 1);
        let req_status = match query.get(&param).map(String::as_str) {
            Some("0") => false,
            Some("1") => true,
            Some(x) => {
                let msg = format!("Invalid value `{}` for parameter `{}`", x, param);
                return (StatusCode::BAD_REQUEST, msg).into();
            }
            None => continue,
        };
        let bit = 1 << idx;

        mask |= bit;

        if req_status {
            states |= bit;
        }
    }

    bmc.activate_slot(states, mask)
        .await
        .context("set power state")
        .into()
}

async fn get_node_power(bmc: &BmcApplication) -> impl Into<LegacyResponse> {
    let n1 = get_node_power_status(bmc, NodeId::Node1).await;
    let n2 = get_node_power_status(bmc, NodeId::Node2).await;
    let n3 = get_node_power_status(bmc, NodeId::Node3).await;
    let n4 = get_node_power_status(bmc, NodeId::Node4).await;

    json!(
     [{
        "node1": n1,
        "node2": n2,
        "node3": n3,
        "node4": n4,
    }]
    )
}

async fn get_node_power_status(bmc: &BmcApplication, node: NodeId) -> String {
    let Ok(status) = bmc.get_node_power(node).await else {
        return "Unknown".to_owned();
    };

    u8::from(status).to_string()
}

fn format_sdcard() -> impl Into<LegacyResponse> {
    LegacyResponse::not_implemented("microSD card formatting is not implemented")
}

fn get_sdcard_info() -> LegacyResponse {
    match get_sdcard_fs_stat() {
        Ok((total, free)) => {
            let used = total - free;
            json!(
                 [{
                    "total": total,
                    "use": used,
                    "free": free,
                }]
            )
            .into()
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            "Failed to get microSD card info: {}",
        )
            .into(),
    }
}

fn get_sdcard_fs_stat() -> anyhow::Result<(u64, u64)> {
    let stat = statfs("/mnt/sdcard")?;
    let bsize = u64::try_from(stat.block_size())?;
    let total = bsize * stat.blocks();
    let free = bsize * stat.blocks_available();

    Ok((total, free))
}

async fn write_to_uart(bmc: &BmcApplication, query: Query) -> LegacyResult<()> {
    let node = get_node_param(&query)?;
    let Some(cmd) = query.get("cmd") else {
        return Err(LegacyResponse::bad_request("Missing `cmd` parameter"));
    };
    let mut data = cmd.clone();

    data.push_str("\r\n");

    bmc.serial_write(node, data.as_bytes())
        .await
        .context("write over UART")
        .map_err(Into::into)
}

async fn read_from_uart(bmc: &BmcApplication, query: Query) -> LegacyResult<LegacyResponse> {
    let node = get_node_param(&query)?;
    let enc = get_encoding_param(&query)?;
    let data = bmc.serial_read(node, enc).await?;

    Ok(LegacyResponse::UartData(data))
}

fn get_encoding_param(query: &Query) -> LegacyResult<Encoding> {
    let Some(enc_str) = query.get("encoding") else {
        return Ok(Encoding::Utf8);
    };

    match enc_str.as_str() {
        "utf8" => Ok(Encoding::Utf8),
        "utf16" | "utf16le" => Ok(Encoding::Utf16 {
            little_endian: true,
        }),
        "utf16be" => Ok(Encoding::Utf16 {
            little_endian: false,
        }),
        "utf32" | "utf32le" => Ok(Encoding::Utf32 {
            little_endian: true,
        }),
        "utf32be" => Ok(Encoding::Utf32 {
            little_endian: false,
        }),
        _ => {
            let msg = "Invalid `encoding` parameter. Expected: utf8, utf16, utf16le, utf16be, \
                       utf32, utf32le, utf32be.";
            Err(LegacyResponse::bad_request(msg))
        }
    }
}

/// switches the USB configuration.
/// API values are mapped to the `UsbConfig` as followed:
///
/// | i32 | Mode   | Route |
/// |-----|--------|-------|
/// | 0   | Host   | USB-A |
/// | 1   | Device | USB-A |
/// | 2   | Host   | BMC   |
/// | 3   | Device | BMC   |
///
async fn set_usb_mode(bmc: &BmcApplication, query: Query) -> LegacyResult<()> {
    let node = get_node_param(&query)?;
    let mode_str = query
        .get("mode")
        .ok_or(LegacyResponse::bad_request("Missing `mode` parameter"))?;

    let mode_num = i32::from_str(mode_str)
        .map_err(|_| LegacyResponse::bad_request("Parameter `mode` is not a number"))?;

    let mode = UsbMode::from_api_mode(mode_num);

    let route = if (mode_num >> 1) & 0x1 == 1 {
        UsbRoute::Bmc
    } else {
        UsbRoute::UsbA
    };

    let cfg = match (mode, route) {
        (UsbMode::Device, UsbRoute::UsbA) => UsbConfig::UsbA(node),
        (UsbMode::Device, UsbRoute::Bmc) => UsbConfig::Bmc(node),
        (UsbMode::Host, route) => UsbConfig::Node(node, route),
    };

    bmc.configure_usb(cfg)
        .await
        .context("set USB mode")
        .map_err(Into::into)
}

/// gets the USB configuration from the POV of the configured node.
async fn get_usb_mode(bmc: &BmcApplication) -> impl Into<LegacyResponse> {
    let config = bmc.get_usb_mode().await;

    let (node, mode, route) = match config {
        UsbConfig::UsbA(node) => (node, UsbMode::Device, UsbRoute::UsbA),
        UsbConfig::Bmc(node) => (node, UsbMode::Device, UsbRoute::Bmc),
        UsbConfig::Node(node, route) => (node, UsbMode::Host, route),
    };

    json!(
        [{
            "mode": mode,
            "node": node,
            "route": route,
        }]
    )
}

async fn handle_flash_status(flash: web::Data<FlashService>) -> LegacyResult<String> {
    Ok(serde_json::to_string(flash.status().await.deref())?)
}

async fn handle_flash_request(
    flash: web::Data<FlashService>,
    bmc: web::Data<BmcApplication>,
    request: HttpRequest,
    query: Query,
) -> LegacyResult<Null> {
    let node = get_node_param(&query)?;
    let file = query
        .get("file")
        .ok_or(LegacyResponse::bad_request(
            "Invalid `file` query parameter",
        ))?
        .to_string();

    let size = query.get("length").ok_or((
        StatusCode::LENGTH_REQUIRED,
        "Invalid `length` query parameter",
    ))?;

    let size = u64::from_str(size)
        .map_err(|_| LegacyResponse::bad_request("`length` parameter is not a number"))?;

    let peer: String = request
        .connection_info()
        .peer_addr()
        .map(Into::into)
        .context("peer_addr unknown")?;

    flash
        .start_transfer(&peer, file, size, node, bmc.into_inner())
        .await?;

    Ok(Null)
}

async fn handle_chunk(
    flash: web::Data<FlashService>,
    request: HttpRequest,
    chunk: Bytes,
) -> LegacyResult<Null> {
    let peer: String = request
        .connection_info()
        .peer_addr()
        .map(Into::into)
        .context("peer_addr unknown")?;

    flash.put_chunk(peer, chunk).await?;
    Ok(Null)
}
