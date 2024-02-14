use async_trait::async_trait;

use super::{NodeId, UsbMode, UsbRoute};
use std::io;

#[async_trait]
pub trait UsbController {
    fn select_usb(&self, node: NodeId, mode: UsbMode) -> io::Result<()>;
    async fn set_usb_route(&self, route: UsbRoute) -> io::Result<()>;
    fn set_usb_boot(&self, nodes_state: u8, nodes_mask: u8) -> std::io::Result<()>;
}

#[async_trait]
#[allow(dead_code)]
pub trait EthernetManagementController {
    async fn reset(&self) -> io::Result<()>;
}

#[async_trait]
pub trait PowerController {
    async fn set_power_node(&self, node_states: u8, node_mask: u8) -> anyhow::Result<()>;
    async fn reset_node(&self, node: NodeId) -> anyhow::Result<()>;
    async fn power_led(&self, on: bool) -> std::io::Result<()>;
}
