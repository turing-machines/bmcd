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
use self::serial::SerialConnections;
use self::serial_handler::Encoding;
use crate::api::{
    get_node_param,
    into_legacy_response::{LegacyResponse, LegacyResult},
};
use crate::serial_service::serial_websocket::run_websocket;
use actix_web::{
    post, route,
    web::{self},
    HttpRequest, HttpResponse, Responder,
};
use bytes::BytesMut;
type Query = web::Query<std::collections::HashMap<String, String>>;

pub mod serial;
pub mod serial_handler;
mod serial_websocket;

pub fn serial_config(cfg: &mut web::ServiceConfig) {
    cfg.service(serial_status).service(handle_ws);
}

#[post("/serial/status")]
async fn serial_status(serials: web::Data<SerialConnections>) -> impl Responder {
    serde_json::to_string(&serials.get_state()).map_or_else(|e| e.to_string(), |s| s)
}

pub async fn legacy_serial_set_handler(
    serials: web::Data<SerialConnections>,
    query: Query,
) -> LegacyResult<()> {
    let node = get_node_param(&query)?;

    let Some(cmd) = query.get("cmd") else {
        return Err(LegacyResponse::bad_request("Missing `cmd` parameter"));
    };

    let mut data: BytesMut = cmd.as_str().into();
    data.extend_from_slice(b"\r\n");

    serials[node].write(data.into()).await.map_err(Into::into)
}

pub async fn legacy_serial_get_handler(
    serials: web::Data<SerialConnections>,
    query: Query,
) -> LegacyResult<LegacyResponse> {
    let node = get_node_param(&query)?;
    let encoding = get_encoding_param(&query)?;
    let data = serials[node].read_as_string(encoding).await?;

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

#[route("/serial/ws", method = "GET", method = "POST")]
async fn handle_ws(
    req: HttpRequest,
    query: Query,
    stream: web::Payload,
    serials: web::Data<SerialConnections>,
) -> Result<HttpResponse, actix_web::Error> {
    let node = get_node_param(&query)?;
    let (res, session, msg_stream) = actix_ws::handle(&req, stream)?;
    match serials[node].open_channel() {
        Ok((stream, sink)) => {
            run_websocket(session, msg_stream, stream, sink).await;
            Ok(res)
        }
        Err(e) => Ok(HttpResponse::InternalServerError().body(e.to_string())),
    }
}
