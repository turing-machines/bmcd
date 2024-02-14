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
pub mod into_legacy_response;
pub mod legacy;
use self::into_legacy_response::{LegacyResponse, LegacyResult};
use crate::bmc::NodeId;
use actix_web::web;
use std::str::FromStr;

pub fn get_node_param(
    query: &web::Query<std::collections::HashMap<String, String>>,
) -> LegacyResult<NodeId> {
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
