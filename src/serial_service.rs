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
use crate::{
    api::{
        get_node_param,
        into_legacy_response::{LegacyResponse, LegacyResult},
    },
    hal::NodeId,
    utils::{string_from_utf16, string_from_utf32},
};
use actix_web::{
    guard::{fn_guard, GuardContext},
    post,
    web::{self, Data},
    Responder,
};
type Query = web::Query<std::collections::HashMap<String, String>>;

pub mod serial;
pub mod serial_channel;
pub mod serial_handler;

pub fn serial_config(cfg: &mut web::ServiceConfig) {
    let uart = Data::new(SerialService::new());
    cfg.app_data(uart)
        .service(
            web::resource("/api/bmc")
                .route(
                    web::get()
                        .guard(fn_guard(legacy_set_uart))
                        .to(serial_set_handler),
                )
                .route(
                    web::get()
                        .guard(fn_guard(legacy_get_uart))
                        .to(serial_get_handler),
                ),
        )
        .service(serial_status);
}

fn legacy_set_uart(context: &GuardContext<'_>) -> bool {
    let Some(query) = context.head().uri.query() else {
        return false;
    };
    query.contains("opt=set") && query.contains("type=uart")
}

fn legacy_get_uart(context: &GuardContext<'_>) -> bool {
    let Some(query) = context.head().uri.query() else {
        return false;
    };
    query.contains("opt=get") && query.contains("type=uart")
}

#[post("/api/bmc/serial/status")]
async fn serial_status(serial_service: web::Data<SerialService>) -> impl Responder {
    serial_service
        .status()
        .map_or_else(|e| e.to_string(), |s| s)
}

async fn serial_set_handler(uart: web::Data<SerialService>, query: Query) -> impl Responder {
    let node = match get_node_param(&query) {
        Ok(n) => n,
        Err(e) => return e,
    };

    let Some(cmd) = query.get("cmd") else {
        return LegacyResponse::bad_request("Missing `cmd` parameter");
    };

    let mut data = cmd.clone();
    data.push_str("\r\n");
    uart.write_node(node, data.as_bytes().into()).await.into()
}

async fn serial_get_handler(uart: web::Data<SerialService>, query: Query) -> impl Responder {
    let legacy_get = async {
        let node = get_node_param(&query)?;
        let encoding = get_encoding_param(&query)?;
        let data = uart.read_as_string(node, encoding).await?;
        Ok::<String, LegacyResponse>(data)
    };

    legacy_get
        .await
        .map_or_else(|e| e, LegacyResponse::UartData)
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

/// Encodings used when reading from a serial port
pub enum Encoding {
    Utf8,
    Utf16 { little_endian: bool },
    Utf32 { little_endian: bool },
}

pub struct SerialService {
    serial_connections: SerialConnections,
}

impl SerialService {
    pub async fn new() -> Self {
        let mut serial_connections = SerialConnections::new();
        serial_connections.run().await.expect("Serial run error");
        Self { serial_connections }
    }

    pub async fn read_as_string(&self, node: NodeId, encoding: Encoding) -> anyhow::Result<String> {
        let handler = &self.serial_connections[node as usize];
        let bytes = handler.read_whole_buffer().await?;
        let res = match encoding {
            Encoding::Utf8 => String::from_utf8_lossy(&bytes).to_string(),
            Encoding::Utf16 { little_endian } => string_from_utf16(&bytes, little_endian),
            Encoding::Utf32 { little_endian } => string_from_utf32(&bytes, little_endian),
        };
        Ok(res)
    }

    pub async fn write_node(&self, node: NodeId, bytes: bytes::BytesMut) -> anyhow::Result<()> {
        let handler = &self.serial_connections[node as usize];
        Ok(handler.write(bytes).await?)
    }

    pub fn status(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&self.serial_connections.get_state())?)
    }
}
