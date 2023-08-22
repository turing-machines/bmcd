//! Routes for legacy API present in versions <= 1.1.0 of the firmware.
use actix_web::http::StatusCode;
use actix_web::{web, HttpResponse, HttpResponseBuilder};
use anyhow::Context;
use nix::sys::statfs::statfs;
use serde_json::json;
use std::str::FromStr;
use tokio::sync::mpsc;
use tpi_rs::app::bmc_application::{BmcApplication, UsbConfig};
use tpi_rs::middleware::{NodeId, UsbMode, UsbRoute};

type Query = web::Query<std::collections::HashMap<String, String>>;

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::resource("/api/bmc")
            .route(web::get().to(api_entry))
            .route(web::post().to(api_post)),
    );
}

/// Trait is implemented for all types that implement `Into<LegacyResponse>`
trait IntoLegacyResponse {
    fn legacy_response(self) -> LegacyResponse;
}

/// Specifies the different repsonses that this legacy API can return. Implements
/// `From<LegacyResponse>` to enforce the legacy json format in the return body.
enum LegacyResponse {
    Success(Option<serde_json::Value>),
    Error(StatusCode, &'static str),
    ErrorOwned(StatusCode, String),
}

impl<T: Into<LegacyResponse>> IntoLegacyResponse for T {
    fn legacy_response(self) -> LegacyResponse {
        self.into()
    }
}

impl IntoLegacyResponse for () {
    fn legacy_response(self) -> LegacyResponse {
        LegacyResponse::Success(None)
    }
}

impl<T: IntoLegacyResponse, E: IntoLegacyResponse> From<Result<T, E>> for LegacyResponse {
    fn from(value: Result<T, E>) -> Self {
        value.map_or_else(|e| e.legacy_response(), |ok| ok.legacy_response())
    }
}

impl From<(StatusCode, &'static str)> for LegacyResponse {
    fn from(value: (StatusCode, &'static str)) -> Self {
        LegacyResponse::Error(value.0, value.1)
    }
}

impl From<(StatusCode, String)> for LegacyResponse {
    fn from(value: (StatusCode, String)) -> Self {
        LegacyResponse::ErrorOwned(value.0, value.1)
    }
}

impl From<serde_json::Value> for LegacyResponse {
    fn from(value: serde_json::Value) -> Self {
        LegacyResponse::Success(Some(value))
    }
}

impl From<anyhow::Error> for LegacyResponse {
    fn from(e: anyhow::Error) -> Self {
        LegacyResponse::ErrorOwned(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to {}: {}", e, e.root_cause()),
        )
    }
}

type LegacyResult<T> = Result<T, LegacyResponse>;

impl From<LegacyResponse> for HttpResponse {
    fn from(value: LegacyResponse) -> Self {
        let (response, result) = match value {
            LegacyResponse::Success(None) => {
                (StatusCode::OK, serde_json::Value::String("ok".to_string()))
            }
            LegacyResponse::Success(Some(body)) => (StatusCode::OK, body),
            LegacyResponse::Error(status_code, msg) => {
                (status_code, serde_json::Value::String(msg.to_string()))
            }
            LegacyResponse::ErrorOwned(status_code, msg) => {
                (status_code, serde_json::Value::String(msg))
            }
        };

        let msg = json!({
            "response": [{ "result": result }]
        });

        HttpResponseBuilder::new(response).json(msg)
    }
}

async fn api_entry(bmc: web::Data<BmcApplication>, query: Query) -> HttpResponse {
    let is_set = match query.get("opt").map(String::as_str) {
        Some("set") => true,
        Some("get") => false,
        _ => {
            return (StatusCode::BAD_REQUEST, "Missing `opt` parameter")
                .legacy_response()
                .into()
        }
    };

    let Some(ty) = query.get("type") else {
            return (StatusCode::BAD_REQUEST, "Missing `opt` parameter").legacy_response().into()
    };

    let bmc = bmc.as_ref();
    match (ty.as_ref(), is_set) {
        ("clear_usb_boot", true) => clear_usb_boot(bmc).legacy_response(),
        ("network", true) => reset_network(bmc).await.legacy_response(),
        ("nodeinfo", true) => set_node_info().legacy_response(),
        ("nodeinfo", false) => get_node_info(bmc).legacy_response(),
        ("node_to_msd", true) => set_node_to_msd(bmc, query).await.legacy_response(),
        ("other", false) => get_system_information().await.legacy_response(),
        ("power", true) => set_node_power(bmc, query).await.legacy_response(),
        ("power", false) => get_node_power(bmc).await.legacy_response(),
        ("sdcard", true) => format_sdcard().legacy_response(),
        ("sdcard", false) => get_sdcard_info(),
        ("uart", true) => write_to_uart(bmc, query),
        ("uart", false) => read_from_uart(bmc, query),
        ("usb", true) => set_usb_mode(bmc, query).await,
        ("usb", false) => get_usb_mode(bmc).await.into(),
        _ => (StatusCode::BAD_REQUEST, "Invalid `type` parameter").legacy_response(),
    }
    .into()
}

fn clear_usb_boot(bmc: &BmcApplication) -> impl IntoLegacyResponse {
    bmc.clear_usb_boot().context("clear USB boot mode")
}

async fn reset_network(bmc: &BmcApplication) -> impl IntoLegacyResponse {
    bmc.rtl_reset().await.context("reset network switch")
}

fn set_node_info() -> impl IntoLegacyResponse {
    // In previous versions of the firmware this was dead code
    (
        StatusCode::NOT_IMPLEMENTED,
        "Method type `set` on parameter `nodeinfo` is deprecated",
    )
}

fn get_node_info(_bmc: &BmcApplication) -> impl IntoLegacyResponse {
    // TODO: implement serial listening in BmcApplication
    let (n1, n2, n3, n4) = (0, 0, 0, 0);

    json!(
       [{
            "node1": n1,
            "node2": n2,
            "node3": n3,
            "node4": n4,
       }]
    )
}

async fn set_node_to_msd(bmc: &BmcApplication, query: Query) -> LegacyResult<()> {
    let node = get_node_param(&query).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let (tx, mut rx) = mpsc::channel(64);
    let logger = async move {
        while let Some(msg) = rx.recv().await {
            log::info!("{}", msg);
        }
        Ok(())
    };

    tokio::try_join!(bmc.set_node_in_msd(node, UsbRoute::Bmc, tx), logger)
        .map(|_| ())
        .map_err(Into::into)
}

fn get_node_param(query: &Query) -> Result<NodeId, &'static str> {
    let Some(node_str) = query.get("node") else {
        return Err("Missing `node` parameter");
    };

    let Ok(node_num) = i32::from_str(node_str) else {
        return Err("Parameter `node` is not a number");
    };

    let Ok(node) = node_num.try_into() else {
        return Err("Parameter `node` is out of range 0..3 of node IDs");
    };

    Ok(node)
}

async fn get_system_information() -> impl IntoLegacyResponse {
    let version = env!("CARGO_PKG_VERSION");
    let build_time = build_time::build_time_utc!("%Y-%m-%d %H:%M:%S-00:00");
    let ipv4 = get_ipv4_address().unwrap_or("Unknown".to_owned());
    let mac = get_mac_address().await;

    json!(
        [{
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

async fn set_node_power(bmc: &BmcApplication, query: Query) -> impl IntoLegacyResponse {
    let mut mask = 0;
    let mut states = 0;

    for idx in 0..4 {
        let param = format!("node{}", idx + 1);
        let req_status = match query.get(&param).map(String::as_str) {
            Some("0") => false,
            Some("1") => true,
            Some(x) => {
                let msg = format!("Invalid value `{}` for parameter `{}`", x, param);
                return LegacyResponse::ErrorOwned(StatusCode::BAD_REQUEST, msg);
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

async fn get_node_power(bmc: &BmcApplication) -> impl IntoLegacyResponse {
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

fn format_sdcard() -> impl IntoLegacyResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "microSD card formatting is not implemented",
    )
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

fn write_to_uart(bmc: &BmcApplication, query: Query) -> LegacyResponse {
    let node = match get_node_param(&query) {
        Ok(n) => n,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into(),
    };

    let Some(cmd) = query.get("cmd") else {
       return (StatusCode::BAD_REQUEST, "Missing `cmd` parameter").into();
    };

    uart_write(bmc, node, cmd).context("write over UART").into()
}

fn uart_write(_bmc: &BmcApplication, _node: NodeId, _cmd: &str) -> anyhow::Result<()> {
    todo!()
}

fn read_from_uart(bmc: &BmcApplication, query: Query) -> LegacyResponse {
    let node = match get_node_param(&query) {
        Ok(n) => n,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into(),
    };

    uart_read(bmc, node).context("read from UART").into()
}

fn uart_read(_bmc: &BmcApplication, _node: NodeId) -> anyhow::Result<()> {
    todo!()
}

async fn set_usb_mode(bmc: &BmcApplication, query: Query) -> LegacyResponse {
    let node = match get_node_param(&query) {
        Ok(n) => n,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into(),
    };

    let Some(mode_str) = query.get("mode") else {
        return (StatusCode::BAD_REQUEST, "Missing `mode` parameter").into();
    };

    let Ok(mode_num) = i32::from_str(mode_str) else {
        return (StatusCode::BAD_REQUEST, "Parameter `mode` is not a number").into();
    };

    let Ok(mode) = mode_num.try_into() else {
        return (StatusCode::BAD_REQUEST, "Parameter `mode` can be either 0 or 1").into();
    };

    let cfg = match mode {
        UsbMode::Device => UsbConfig::UsbA(node, true),
        UsbMode::Host => UsbConfig::Node(node, UsbRoute::UsbA),
    };

    bmc.configure_usb(cfg).await.context("set USB mode").into()
}

async fn get_usb_mode(bmc: &BmcApplication) -> anyhow::Result<impl IntoLegacyResponse> {
    let config = bmc
        .get_usb_mode()
        .await
        .context("Failed to get current USB mode")?;

    let (node, mode) = match config {
        UsbConfig::UsbA(node, _) | UsbConfig::Bmc(node, _) => (node, UsbMode::Device),
        UsbConfig::Node(node, _) => (node, UsbMode::Host),
    };

    Ok(json!(
        [{
            "mode": mode,
            "node": node,
        }]
    ))
}

async fn api_post(query: Query) -> HttpResponse {
    if query.get("opt") != Some(&"set".to_owned()) {
        return LegacyResponse::Error(StatusCode::BAD_REQUEST, "Invalid `opt` parameter").into();
    }

    let Some(ty) = query.get("type") else {
        return LegacyResponse::Error(StatusCode::BAD_REQUEST, "Missing `type` parameter").into();
    };

    match ty.as_ref() {
        "firmware" => stub(),
        "flash" => stub(),
        _ => LegacyResponse::Error(StatusCode::BAD_REQUEST, "Invalid `type` parameter").into(),
    }
}

fn stub() -> HttpResponse {
    LegacyResponse::Success(None).into()
}
