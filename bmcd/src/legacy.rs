//! Routes for legacy API present in versions <= 1.1.0 of the firmware.

use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;

use actix_web::{rt, web, HttpResponse};
use nix::sys::statfs::statfs;
use serde_json::json;
use tokio::sync::mpsc;
use tpi_rs::app::bmc_application::{BmcApplication, UsbConfig};
use tpi_rs::middleware::{NodeId, UsbMode, UsbRoute};

type Query = web::Query<std::collections::HashMap<String, String>>;

trait ToHttpResponse {
    fn to_response(&self, action: &'static str) -> HttpResponse;
}

impl<T, E: Display> ToHttpResponse for Result<T, E> {
    fn to_response(&self, action: &'static str) -> HttpResponse {
        match self {
            Ok(_) => success_response(),
            Err(e) => {
                let msg = format!("Failed to {}: {}", action, e);
                HttpResponse::InternalServerError().body(msg)
            }
        }
    }
}

fn success_response() -> HttpResponse {
    // For backwards compatibility, `response` holds an array, even though it's unnecessary
    let msg = json!({
        "response": [{ "result": "ok" }]
    });

    HttpResponse::Ok().json(msg)
}

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/bmc").service(
            web::resource("")
                .route(web::get().to(api_entry))
                .route(web::post().to(api_post)),
        ),
    );
}

// TODO: BmcApplication::new() needs to return just BmcApplication, not wrapped in Arc<>.
async fn api_entry(bmc: web::Data<Arc<BmcApplication>>, query: Query) -> HttpResponse {
    let is_set = match query.get("opt").map(String::as_str) {
        Some("set") => true,
        Some("get") => false,
        _ => return HttpResponse::BadRequest().body("Missing `opt` parameter"),
    };

    let Some(ty) = query.get("type") else {
        return HttpResponse::BadRequest().body("Missing `type` parameter");
    };

    let bmc = &bmc;

    match (ty.as_ref(), is_set) {
        ("clear_usb_boot", true) => clear_usb_boot(bmc),
        ("network", true) => reset_network(bmc).await,
        ("nodeinfo", true) => set_node_info(),
        ("nodeinfo", false) => get_node_info(bmc),
        ("node_to_msd", true) => set_node_to_msd(bmc, query).await,
        ("other", false) => get_system_information().await,
        ("power", true) => set_node_power(bmc, query).await,
        ("power", false) => get_node_power(bmc).await,
        ("sdcard", true) => format_sdcard(),
        ("sdcard", false) => get_sdcard_info(),
        ("uart", true) => write_to_uart(bmc, query),
        ("uart", false) => read_from_uart(bmc, query),
        ("usb", true) => set_usb_mode(bmc, query).await,
        ("usb", false) => get_usb_mode(bmc).await,
        _ => HttpResponse::BadRequest().body("Invalid `type` parameter"),
    }
}

fn clear_usb_boot(bmc: &BmcApplication) -> HttpResponse {
    bmc.clear_usb_boot().to_response("clear USB boot mode")
}

async fn reset_network(bmc: &BmcApplication) -> HttpResponse {
    bmc.rtl_reset().await.to_response("reset network switch")
}

fn set_node_info() -> HttpResponse {
    // In previous versions of the firmware this was dead code
    HttpResponse::NotImplemented().body("Method type `set` on parameter `nodeinfo` is deprecated")
}

fn get_node_info(_bmc: &BmcApplication) -> HttpResponse {
    // TODO: implement serial listening in BmcApplication
    let (n1, n2, n3, n4) = (0, 0, 0, 0);

    let body = json!({
        "response": [{
            "node1": n1,
            "node2": n2,
            "node3": n3,
            "node4": n4,
        }]
    });

    HttpResponse::Ok().json(body)
}

async fn set_node_to_msd(bmc: &Arc<BmcApplication>, query: Query) -> HttpResponse {
    let node = match get_node_param(&query) {
        Ok(n) => n,
        Err(e) => return HttpResponse::BadRequest().body(e),
    };

    let (tx, mut rx) = mpsc::channel(64);

    let bmc = bmc.clone();
    let handle = rt::spawn(async move { bmc.set_node_in_msd(node, UsbRoute::Bmc, tx).await });

    let logger = rt::spawn(async move {
        while let Some(msg) = rx.recv().await {
            log::info!("{}", msg);
        }
    });

    tokio::try_join!(handle, logger).to_response("await threads")
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

async fn get_system_information() -> HttpResponse {
    let version = env!("CARGO_PKG_VERSION");
    let build_time = build_time::build_time_utc!("%Y-%m-%d %H:%M:%S-00:00");
    let ipv4 = get_ipv4_address().unwrap_or("Unknown".to_owned());
    let mac = get_mac_address().await;

    let body = json!({
        "response": [{
            "version": version,
            "buildtime": build_time,
            "ip": ipv4,
            "mac": mac,
        }]
    });

    HttpResponse::Ok().json(body)
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

async fn set_node_power(bmc: &BmcApplication, query: Query) -> HttpResponse {
    let mut mask = 0;
    let mut states = 0;

    for idx in 0..4 {
        let param = format!("node{}", idx + 1);
        let req_status = match query.get(&param).map(String::as_str) {
            Some("0") => false,
            Some("1") => true,
            Some(x) => {
                let msg = format!("Invalid value `{}` for parameter `{}`", x, param);
                return HttpResponse::BadRequest().body(msg);
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
        .to_response("set power state")
}

async fn get_node_power(bmc: &BmcApplication) -> HttpResponse {
    let n1 = get_node_power_status(bmc, NodeId::Node1).await;
    let n2 = get_node_power_status(bmc, NodeId::Node2).await;
    let n3 = get_node_power_status(bmc, NodeId::Node3).await;
    let n4 = get_node_power_status(bmc, NodeId::Node4).await;

    let body = json!({
        "response": [{
            "node1": n1,
            "node2": n2,
            "node3": n3,
            "node4": n4,
        }]
    });

    HttpResponse::Ok().json(body)
}

async fn get_node_power_status(bmc: &BmcApplication, node: NodeId) -> String {
    let Ok(status) = bmc.get_node_power(node).await else {
        return "Unknown".to_owned();
    };

    u8::from(status).to_string()
}

fn format_sdcard() -> HttpResponse {
    HttpResponse::NotImplemented().body("microSD card formatting is not implemented")
}

fn get_sdcard_info() -> HttpResponse {
    match get_sdcard_fs_stat() {
        Ok((total, free)) => {
            let used = total - free;
            let body = json!({
                "response": [{
                    "total": total,
                    "use": used,
                    "free": free,
                }]
            });

            HttpResponse::Ok().json(body)
        }
        Err(e) => {
            let msg = format!("Failed to get microSD card info: {}", e);
            HttpResponse::InternalServerError().body(msg)
        }
    }
}

fn get_sdcard_fs_stat() -> anyhow::Result<(u64, u64)> {
    let stat = statfs("/mnt/sdcard")?;
    let bsize = u64::try_from(stat.block_size())?;
    let total = bsize * stat.blocks();
    let free = bsize * stat.blocks_available();

    Ok((total, free))
}

fn write_to_uart(bmc: &BmcApplication, query: Query) -> HttpResponse {
    let node = match get_node_param(&query) {
        Ok(n) => n,
        Err(e) => return HttpResponse::BadRequest().body(e),
    };

    let Some(cmd) = query.get("cmd") else {
        return HttpResponse::BadRequest().body("Missing `cmd` parameter");
    };

    uart_write(bmc, node, cmd).to_response("write over UART")
}

fn uart_write(_bmc: &BmcApplication, _node: NodeId, _cmd: &str) -> anyhow::Result<()> {
    todo!()
}

fn read_from_uart(bmc: &BmcApplication, query: Query) -> HttpResponse {
    let node = match get_node_param(&query) {
        Ok(n) => n,
        Err(e) => return HttpResponse::BadRequest().body(e),
    };

    uart_read(bmc, node).to_response("read from UART")
}

fn uart_read(_bmc: &BmcApplication, _node: NodeId) -> anyhow::Result<()> {
    todo!()
}

async fn set_usb_mode(bmc: &BmcApplication, query: Query) -> HttpResponse {
    let node = match get_node_param(&query) {
        Ok(n) => n,
        Err(e) => return HttpResponse::BadRequest().body(e),
    };

    let Some(mode_str) = query.get("mode") else {
        return HttpResponse::BadRequest().body("Missing `mode` parameter");
    };

    let Ok(mode_num) = i32::from_str(mode_str) else {
        return HttpResponse::BadRequest().body("Parameter `mode` is not a number");
    };

    let Ok(mode) = mode_num.try_into() else {
        return HttpResponse::BadRequest().body("Parameter `mode` can be either 0 or 1");
    };

    let cfg = match mode {
        UsbMode::Device => UsbConfig::UsbA(node, true),
        UsbMode::Host => UsbConfig::Node(node, UsbRoute::UsbA),
    };

    bmc.configure_usb(cfg).await.to_response("set USB mode")
}

async fn get_usb_mode(bmc: &BmcApplication) -> HttpResponse {
    let config = match bmc.get_usb_mode().await {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Failed to get current USB mode: {}", e);
            return HttpResponse::InternalServerError().body(msg);
        }
    };

    let (node, mode) = match config {
        UsbConfig::UsbA(node, _) | UsbConfig::Bmc(node, _) => (node, UsbMode::Device),
        UsbConfig::Node(node, _) => (node, UsbMode::Host),
    };

    let body = json!({
        "response": [{
            "mode": mode,
            "node": node,
        }]
    });

    HttpResponse::Ok().json(body)
}

async fn api_post(query: Query) -> HttpResponse {
    if query.get("opt") != Some(&"set".to_owned()) {
        return HttpResponse::BadRequest().body("Invalid `opt` parameter");
    }

    let Some(ty) = query.get("type") else {
        return HttpResponse::BadRequest().body("Missing `type` parameter");
    };

    match ty.as_ref() {
        "firmware" => stub(),
        "flash" => stub(),
        _ => HttpResponse::BadRequest().body("Invalid `type` parameter"),
    }
}

fn stub() -> HttpResponse {
    HttpResponse::Ok().finish()
}
