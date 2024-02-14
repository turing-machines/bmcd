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
pub mod bmc_factory;
pub mod helpers;
pub mod stub;
pub mod traits;
use std::fmt::Display;

macro_rules! conditional_import {
    ($attribute_condition:meta, $($statement:item)+) => {
        $(
            #[$attribute_condition]
            $statement
        )*
    };
}

conditional_import! {
    cfg(not(feature = "stubbed")),
    mod gpio_definitions;
    mod pin_controller;
    mod power_controller;
    pub use pin_controller::*;
    pub use power_controller::*;
}

conditional_import! {
    cfg(feature = "stubbed"),
    mod stub;
    pub use stub::*;
}

#[repr(C)]
#[derive(Debug, Eq, Hash, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum NodeId {
    Node1,
    Node2,
    Node3,
    Node4,
}

impl TryFrom<u8> for NodeId {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(NodeId::Node1),
            1 => Ok(NodeId::Node2),
            2 => Ok(NodeId::Node3),
            3 => Ok(NodeId::Node4),
            x => Err(format!("node id {} does not exist", x)),
        }
    }
}

impl TryFrom<i32> for NodeId {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        (value as u8).try_into()
    }
}

impl NodeId {
    pub fn to_bitfield(self) -> u8 {
        1 << self as u8
    }

    pub fn to_inverse_bitfield(self) -> u8 {
        0b1111 & !(1 << self as u8)
    }
}

impl Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node {:?}", (*self as u8) + 1)
    }
}

#[repr(C)]
#[derive(Debug, Eq, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum NodeType {
    RaspberryPi4,
    JetsonTx2,
    RK1,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum UsbRoute {
    Bmc,
    UsbA,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum UsbMode {
    Host,
    Device,
    Flash,
}

impl UsbMode {
    pub fn from_api_mode(value: i32) -> Self {
        match value & 0b11 {
            0 => UsbMode::Host,
            1 => UsbMode::Device,
            2 => UsbMode::Flash,
            _ => unreachable!(),
        }
    }
}
