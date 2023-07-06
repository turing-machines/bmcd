pub mod app_persistency;
pub mod event_listener;
pub mod firmware_update_usb;
mod gpio_definitions;
pub(crate) mod helpers;
pub mod pin_controller;
pub mod power_controller;
pub mod rk1_fwudate;
pub mod rpi_fwupdate;
pub mod usbboot;

#[repr(C)]
#[derive(Debug, Eq, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum NodeId {
    Node1,
    Node2,
    Node3,
    Node4,
}

impl TryFrom<i32> for NodeId {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(NodeId::Node1),
            1 => Ok(NodeId::Node2),
            2 => Ok(NodeId::Node3),
            3 => Ok(NodeId::Node4),
            x => Err(format!("node id {} does not exist", x)),
        }
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

impl TryFrom<i32> for UsbRoute {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(UsbRoute::Bmc),
            1 => Ok(UsbRoute::UsbA),
            x => Err(format!("usb route{} does not exist", x)),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum UsbMode {
    Host,
    Device,
}

impl TryFrom<i32> for UsbMode {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(UsbMode::Host),
            1 => Ok(UsbMode::Device),
            x => Err(format!("usb mode {} does not exist", x)),
        }
    }
}
