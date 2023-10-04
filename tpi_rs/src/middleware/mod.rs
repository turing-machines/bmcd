pub mod firmware_update;
mod gpio_definitions;
mod helpers;
pub mod persistency;
pub mod pin_controller;
pub mod power_controller;
pub mod serial;
pub mod usbboot;

#[repr(C)]
#[derive(Debug, Eq, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
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
}

impl UsbMode {
    pub fn from_api_mode(value: i32) -> Self {
        match value & 0x1 {
            0 => UsbMode::Host,
            1 => UsbMode::Device,
            _ => unreachable!(),
        }
    }
}
