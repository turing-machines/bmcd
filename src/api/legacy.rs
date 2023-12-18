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
//! Routes for legacy API present in versions <= 2.0.0 of the firmware.
use crate::api::into_legacy_response::LegacyResponse;
use crate::api::into_legacy_response::{LegacyResult, Null};
use crate::app::bmc_application::{BmcApplication, Encoding, UsbConfig};
use crate::app::bmc_info::{
    get_fs_stat, get_ipv4_address, get_mac_address, get_net_interfaces, get_storage_info,
};
use crate::app::transfer_action::InitializeTransfer;
use crate::app::transfer_action::UpgradeCommand;
use crate::hal::{NodeId, UsbMode, UsbRoute};
use crate::streaming_data_service::data_transfer::DataTransfer;
use crate::streaming_data_service::StreamingDataService;
use crate::utils;
use actix_files::file_extension_to_mime;
use actix_multipart::Multipart;
use actix_web::guard::{fn_guard, GuardContext};
use actix_web::http::{header, StatusCode};
use actix_web::{get, post, web, HttpResponse, Responder};
use anyhow::Context;
use async_compression::tokio::bufread::GzipEncoder;
use async_compression::Level;
use humansize::{format_size, DECIMAL};
use serde_json::json;
use std::collections::HashMap;
use std::ops::Deref;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;
use tokio_stream::StreamExt;
use tokio_util::io::ReaderStream;
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
                    .to(handle_transfer_request),
            )
            .route(
                web::post()
                    .guard(fn_guard(set_node_info_guard))
                    .to(set_node_aux_info),
            )
            .route(web::get().to(api_entry)),
    )
    .service(handle_file_upload)
    .service(cancel_file_upload)
    .service(backup_handler);
}

pub fn info_config(cfg: &mut web::ServiceConfig) {
    cfg.service(info_handler);
}

fn flash_status_guard(context: &GuardContext<'_>) -> bool {
    let Some(query) = context.head().uri.query() else {
        return false;
    };
    query.contains("opt=get") && (query.contains("type=flash") || query.contains("type=firmware"))
}

fn flash_guard(context: &GuardContext<'_>) -> bool {
    let Some(query) = context.head().uri.query() else {
        return false;
    };
    query.contains("opt=set") && (query.contains("type=flash") || query.contains("type=firmware"))
}

fn set_node_info_guard(context: &GuardContext<'_>) -> bool {
    let Some(query) = context.head().uri.query() else {
        return false;
    };
    query.contains("opt=set") && query.contains("type=node_info")
}

#[get("/api/bmc/backup")]
async fn backup_handler() -> impl Responder {
    let archive = tokio::task::spawn_blocking(move || {
        let mut builder = tar::Builder::new(Vec::new());
        builder.mode(tar::HeaderMode::Deterministic);
        builder
            .append_dir_all(".", "/mnt/overlay/upper/")
            .and_then(|_| builder.finish())
            .and_then(|_| builder.into_inner())
    })
    .await
    .expect("error joining archiving task");

    match archive {
        Ok(buffer) => {
            let now = chrono::Local::now();
            let content_disposition = format!(
                r#"attachment; filename="tp2-backup-{}.tar.gz""#,
                now.format("%d-%m-%Y")
            );
            let encoder = GzipEncoder::with_quality(std::io::Cursor::new(buffer), Level::Best);
            HttpResponse::Ok()
                .insert_header(header::ContentType(file_extension_to_mime("gz")))
                .insert_header((header::CONTENT_DISPOSITION, content_disposition))
                .streaming(ReaderStream::new(encoder))
        }
        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
    }
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
        ("node_info", false) => get_node_aux_info(bmc).await.into(),
        ("node_to_msd", true) => set_node_to_msd(bmc, query).await.into(),
        ("other", false) => get_system_information().await.into(),
        ("power", true) => set_node_power(bmc, query).await,
        ("power", false) => get_node_power(bmc).await.into(),
        ("reboot", true) => reboot().await.into(),
        ("reload", true) => reload_self().into(),
        ("reset", true) => reset_node(bmc, query).await.into(),
        ("sdcard", true) => format_sdcard().into(),
        ("sdcard", false) => get_sdcard_info(),
        ("uart", true) => write_to_uart(bmc, query).await.into(),
        ("uart", false) => read_from_uart(bmc, query).await.into(),
        ("usb", true) => set_usb_mode(bmc, query).await.into(),
        ("usb", false) => get_usb_mode(bmc).await.into(),
        ("info", false) => get_info().await.into(),
        ("about", false) => get_about().await.into(),
        _ => (
            StatusCode::BAD_REQUEST,
            format!("Invalid `type` parameter {}", ty),
        )
            .into(),
    }
}

#[allow(clippy::unused_unit)]
fn reload_self() -> impl Into<LegacyResponse> {
    tokio::task::spawn_blocking(move || {
        Command::new("sh")
            .arg("-c")
            .arg("/etc/init.d/S94bmcd restart")
            .status()
    });

    ()
}

async fn get_about() -> impl Into<LegacyResponse> {
    let version = env!("CARGO_PKG_VERSION");
    let build_time = build_time::build_time_utc!("%Y-%m-%d %H:%M:%S-00:00");

    let mut buildroot = "unknown".to_string();
    let mut build_version = "unknown".to_string();

    if let Ok(os_release) = read_os_release().await {
        if let Some(buildroot_edition) = os_release.get("PRETTY_NAME") {
            buildroot = buildroot_edition.trim_matches('"').to_string();
        }
        if let Some(version) = os_release.get("VERSION") {
            build_version = version.to_string();
        }
    }

    json!(
        {
            "api": API_VERSION,
            "version": version,
            "buildtime": build_time,
            "buildroot": buildroot,
            "build_version": build_version,
        }
    )
}

async fn get_info() -> impl Into<LegacyResponse> {
    let storage = get_storage_info();
    let ips = get_net_interfaces().await;
    json!(
        {
            "ip": ips,
            "storage": storage,
        }
    )
}

async fn reboot() -> LegacyResult<()> {
    BmcApplication::reboot().await.map_err(Into::into)
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

#[derive(Debug, serde::Deserialize)]
struct SetNodeInfos {
    node_info: Vec<SetNodeInfo>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SetNodeInfo {
    pub node: u8,
    pub name: Option<String>,
    pub module_name: Option<String>,
    pub uart_baud: Option<u32>,
}

async fn set_node_aux_info(
    bmc: web::Data<BmcApplication>,
    payload: web::Json<SetNodeInfos>,
) -> impl Responder {
    for info in &payload.node_info {
        bmc.set_node_info(info.clone()).await?;
    }

    Ok::<Null, LegacyResponse>(Null)
}

async fn get_node_aux_info(bmc: &BmcApplication) -> impl Into<LegacyResponse> {
    let Some(current_time) = utils::get_timestamp_unix() else {
        anyhow::bail!("Current time before Unix epoch");
    };

    let infos = bmc.get_node_infos().await;

    let mut output = json!(
        {
            "node_info": []
        }
    );

    let array = output["node_info"].as_array_mut().expect("key mismatch");

    for (i, n) in infos.data.iter().enumerate() {
        let insert_idx = array.len();
        let key = (i + 1).to_string();
        let uptime = match n.power_on_time {
            Some(powered_on) => current_time - powered_on,
            None => 0,
        };

        let node_info = json!(
            {
                key: {
                    "name": n.name,
                    "module_name": n.module_name,
                    "uptime": uptime,
                    "uart_baud": n.uart_baud,
                }
            }
        );

        array.insert(insert_idx, node_info);
    }

    Ok(output)
}

async fn set_node_to_msd(bmc: &BmcApplication, query: Query) -> LegacyResult<()> {
    let node = get_node_param(&query)?;
    bmc.node_in_msd(node).await?;
    Ok(())
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

async fn read_os_release() -> std::io::Result<HashMap<String, String>> {
    let buffer = tokio::fs::read("/etc/os-release").await?;
    let mut lines = buffer.lines();
    let mut results = HashMap::new();

    while let Some(line) = lines.next_line().await? {
        if let Some((key, value)) = line.split_once('=') {
            results.insert(key.to_string(), value.to_string());
        }
    }
    Ok(results)
}

/// function is here for backwards compliance. Data is mostly a duplication of [`get_about`]
async fn get_system_information() -> impl Into<LegacyResponse> {
    let version = env!("CARGO_PKG_VERSION");
    let build_time = build_time::build_time_utc!("%Y-%m-%d %H:%M:%S-00:00");
    let ipv4 = get_ipv4_address().unwrap_or("Unknown".to_owned());
    let mac = get_mac_address("eth0").await;

    let mut info = json!(
        {
            "api": API_VERSION,
            "version": version,
            "buildtime": build_time,
            "ip": ipv4,
            "mac": mac,
        }
    );

    if let Ok(os_release) = read_os_release().await {
        let obj = info.as_object_mut().unwrap();
        if let Some(buildroot_edition) = os_release.get("PRETTY_NAME") {
            obj.insert(
                "buildroot".to_string(),
                serde_json::value::to_value(buildroot_edition).unwrap(),
            );
        }
        if let Some(build_version) = os_release.get("VERSION") {
            obj.insert(
                "build_version".to_string(),
                serde_json::value::to_value(build_version).unwrap(),
            );
        }
    }

    json!([info])
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

/// function is here for backwards compliance. Data is mostly a duplication of [`get_info`]
fn get_sdcard_info() -> LegacyResponse {
    match get_fs_stat("/mnt/sdcard") {
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
/// | i32 | Mode         | Route |
/// |-----|--------------|-------|
/// | 0   | Host         | USB-A |
/// | 1   | Device       | USB-A |
/// | 2   | Flash host   | USB-A |
/// | 3   | Flash device | USB-A |
/// | 4   | Host         | BMC   |
/// | 5   | Device       | BMC   |
/// | 6   | Flash host   | BMC   |
/// | 7   | Flash device | BMC   |
///
async fn set_usb_mode(bmc: &BmcApplication, query: Query) -> LegacyResult<()> {
    let node = get_node_param(&query)?;
    let mode_str = query
        .get("mode")
        .ok_or(LegacyResponse::bad_request("Missing `mode` parameter"))?;

    let mode_num = i32::from_str(mode_str)
        .map_err(|_| LegacyResponse::bad_request("Parameter `mode` is not a number"))?;

    let mode = UsbMode::from_api_mode(mode_num);

    let route = if (mode_num >> 2) & 0x1 == 1 {
        UsbRoute::Bmc
    } else {
        UsbRoute::UsbA
    };

    let cfg = match (mode, route) {
        (UsbMode::Device, UsbRoute::UsbA) => UsbConfig::UsbA(node),
        (UsbMode::Device, UsbRoute::Bmc) => UsbConfig::Bmc(node),
        (UsbMode::Host, route) => UsbConfig::Node(node, route),
        (UsbMode::Flash, route) => UsbConfig::Flashing(node, route),
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
        UsbConfig::Flashing(node, route) => (node, UsbMode::Flash, route),
    };

    json!(
        [{
            "mode": mode,
            "node": node.to_string(),
            "route": route,
        }]
    )
}

async fn handle_flash_status(flash: web::Data<StreamingDataService>) -> LegacyResult<String> {
    Ok(serde_json::to_string(flash.status().await.deref())?)
}

async fn handle_transfer_request(
    ss: web::Data<StreamingDataService>,
    bmc: web::Data<BmcApplication>,
    query: Query,
) -> LegacyResult<String> {
    let (process_name, upgrade_command) = match query.get("type").map(|c| c.as_str()) {
        Some("firmware") => (
            "firmware upgrade service".to_string(),
            UpgradeCommand::OsUpgrade,
        ),
        Some("flash") => {
            let node = get_node_param(&query)?;
            (
                format!("{node} os install service"),
                UpgradeCommand::Module(node, bmc.clone().into_inner()),
            )
        }
        _ => {
            return Err(LegacyResponse::bad_request(
                "`type` should equal 'firmware' or 'flash'",
            ))
        }
    };

    let data_transfer = create_data_transfer(&query).await?;
    let do_crc = !query.contains_key("skip_crc");
    let transfer_request =
        InitializeTransfer::new(process_name, upgrade_command, data_transfer, do_crc);

    let handle = ss.request_transfer(transfer_request.try_into()?).await?;
    let json = json!({"handle": handle});
    Ok(json.to_string())
}

async fn create_data_transfer(query: &Query) -> LegacyResult<DataTransfer> {
    let file = query.get("file").ok_or(LegacyResponse::bad_request(
        "Invalid `file` query parameter",
    ))?;

    if query.contains_key("local") {
        return Ok(DataTransfer::local(PathBuf::from(file)));
    }

    let sha256 = try_map_sha256(query.get("sha256"))?;

    if file.starts_with("http") {
        let url = reqwest::Url::parse(file).map_err(|e| {
            LegacyResponse::bad_request(format!(
                "{file} could not be parsed to a url object: {:#}",
                e
            ))
        })?;
        return Ok(DataTransfer::url(url, sha256).await?);
    }

    let size = query.get("length").ok_or((
        StatusCode::LENGTH_REQUIRED,
        "Invalid `length` query parameter",
    ))?;

    let size = u64::from_str(size)
        .map_err(|_| LegacyResponse::bad_request("`length` parameter is not a number"))?;

    Ok(DataTransfer::remote(PathBuf::from(&file), size, 16, sha256))
}

pub fn try_map_sha256(value: Option<&String>) -> LegacyResult<Option<bytes::Bytes>> {
    let sha = if let Some(sha256) = value {
        let bytes = hex::decode(sha256)
            .map_err(|e| {
                LegacyResponse::bad_request(format!(
                    "`sha256` parameter contains invalid hex values: {}",
                    e
                ))
            })?
            .into();
        Some(bytes)
    } else {
        None
    };

    Ok(sha)
}

#[get("/api/bmc/upload/{handle}/cancel")]
async fn cancel_file_upload(ss: web::Data<StreamingDataService>) -> impl Responder {
    ss.cancel_all().await;
    HttpResponse::Ok().finish()
}

#[post("/api/bmc/upload/{handle}")]
async fn handle_file_upload(
    handle: web::Path<u32>,
    ss: web::Data<StreamingDataService>,
    mut payload: Multipart,
) -> impl Responder {
    let (sender, size) = ss.take_sender(*handle).await?;
    let Some(Ok(mut field)) = payload.next().await else {
        return Err(LegacyResponse::bad_request("Multipart form invalid"));
    };

    let mut bytes_send: u64 = 0;
    while let Some(Ok(chunk)) = field.next().await {
        let length = chunk.len();
        if sender.send(chunk).await.is_err() {
            return Err(return_transfer_error(ss).await.into());
        }

        bytes_send += length as u64;
    }

    if bytes_send != size {
        ss.cancel_all().await;
        return Err(LegacyResponse::bad_request(format!(
            "missing {} bytes",
            format_size(size - bytes_send, DECIMAL)
        )));
    }

    Ok(Null)
}

/// When the channel gets dropped, give the worker some time to shutdown so that the
/// actual error message can be bubbled up.
async fn return_transfer_error(ss: web::Data<StreamingDataService>) -> impl Into<LegacyResponse> {
    let msg = ss.try_get_error(Duration::from_secs(5)).await;
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        msg.unwrap_or("transfer canceled".to_string()),
    )
}
