//! Routes for legacy API present in versions <= 1.0.2 of the firmware.

use std::collections::HashMap;

use actix_web::{web, HttpResponse};

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/bmc").service(
            web::resource("")
                .route(web::get().to(api_entry))
                .route(web::post().to(api_post)),
        ),
    );
}

async fn api_entry(query: web::Query<HashMap<String, String>>) -> HttpResponse {
    let is_set = match query.get("opt").cloned().as_deref() {
        Some("set") => true,
        Some("get") => false,
        _ => return HttpResponse::BadRequest().finish(),
    };

    let Some(ty) = query.get("type") else {
        return HttpResponse::BadRequest().finish()
    };

    match (ty.as_ref(), is_set) {
        ("clear_usb_boot", true) => stub(),
        ("network", true) => stub(),
        ("nodeinfo", true) => stub(),
        ("nodeinfo", false) => stub(),
        ("node_to_msd", true) => stub(),
        ("other", true) => stub(),
        ("power", true) => stub(),
        ("power", false) => stub(),
        ("sdcard", true) => stub(),
        ("sdcard", false) => stub(),
        ("uart", true) => stub(),
        ("uart", false) => stub(),
        ("usb", true) => stub(),
        ("usb", false) => stub(),
        _ => return HttpResponse::BadRequest().finish(),
    }

    HttpResponse::Ok().finish()
}

async fn api_post(query: web::Query<HashMap<String, String>>) -> HttpResponse {
    if query.get("opt") != Some(&"set".to_owned()) {
        return HttpResponse::BadRequest().finish();
    }

    let Some(ty) = query.get("type") else {
        return HttpResponse::BadRequest().finish()
    };

    match ty.as_ref() {
        "firmware" => stub(),
        "flash" => stub(),
        _ => return HttpResponse::BadRequest().finish(),
    }

    HttpResponse::Ok().finish()
}

fn stub() {}
