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
use crate::hal::helpers::bit_iterator;
use crate::hal::PowerController;
use crate::hal::{NodeId, PinController, UsbMode, UsbRoute};
use crate::persistency::app_persistency::ApplicationPersistency;
use crate::persistency::app_persistency::PersistencyBuilder;
use crate::usb_boot::NodeDrivers;
use crate::utils::{self, get_timestamp_unix};
use crate::{
    app::usb_gadget::append_msd_config_to_usb_gadget,
    app::usb_gadget::remove_msd_function_from_usb_gadget,
};

use anyhow::{ensure, Context};
use log::info;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite};

pub type NodeInfos = [NodeInfo; 4];

/// Stores which slots are actually used. This information is used to determine
/// for instance, which nodes need to be powered on, when such command is given
pub const ACTIVATED_NODES_KEY: &str = "activated_nodes";
/// stores to which node the USB multiplexer is configured to.
pub const USB_CONFIG: &str = "usb_config";
/// Stores information about nodes: name alias, time since powered on, and others. See [NodeInfo].
pub const NODE_INFO_KEY: &str = "node_info";

/// Describes the different configuration the USB bus can be setup
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum UsbConfig {
    /// USB-A port is host, NodeId is the device.
    UsbA(NodeId),
    /// BMC is host, NodeId is the device.
    Bmc(NodeId),
    /// NodeId is host, [UsbRoute] is configured for device
    Node(NodeId, UsbRoute),
    /// Configures the given node as a USB device with the usbboot pin high
    Flashing(NodeId, UsbRoute),
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeInfo {
    pub name: Option<String>,
    pub module_name: Option<String>,
    pub power_on_time: Option<u64>,
    pub uart_baud: Option<u32>,
}

pub struct BmcApplication {
    pub(super) pin_controller: PinController,
    pub(super) power_controller: PowerController,
    pub(super) app_db: ApplicationPersistency,
    node_drivers: NodeDrivers,
}

impl BmcApplication {
    pub async fn new(database_write_timeout: Option<Duration>) -> anyhow::Result<Self> {
        let pin_controller = PinController::new().context("pin_controller")?;
        let power_controller = PowerController::new().context("power_controller")?;
        let app_db = PersistencyBuilder::default()
            .register_key(ACTIVATED_NODES_KEY, &0u8)
            .register_key(USB_CONFIG, &UsbConfig::UsbA(NodeId::Node1))
            .register_key(NODE_INFO_KEY, &NodeInfos::default())
            .write_timeout(database_write_timeout)
            .build()
            .await?;

        let node_drivers = NodeDrivers::new();

        let instance = Self {
            pin_controller,
            power_controller,
            app_db,
            node_drivers,
        };

        instance.initialize().await?;
        Ok(instance)
    }

    /// toggles the power state of the nodes. When `inverse_toggle` == true, and
    /// not all nodes are off nor on, it will turn off all nodes instead of
    /// turning them on.
    ///
    /// # State table
    ///
    /// | state             | long_press | All nodes |
    /// | :---------------- | :--------: | :-------: |
    /// | 0b0000            |  False     | On        |
    /// | 0b0111            |  False     | Off       |
    /// | 0b1111            |  False     | Off       |
    /// | 0b0000            |  True      | On        |
    /// | 0b0111            |  True      | On        |
    /// | 0b1111            |  True      | Off       |
    ///
    /// # Return
    ///
    /// returns Err(e) on an internal gpio error or when there is an error
    /// writing power LED status.
    pub async fn toggle_power_states(&self, inverse_toggle: bool) -> anyhow::Result<()> {
        let node_values = self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await;

        let mut on = node_values == 0;
        if inverse_toggle && node_values != 0 && node_values != 0b1111 {
            on = !on;
        }

        let node_values = if on { 0b1111 } else { 0b0000 };
        self.activate_slot(node_values, 0b1111).await
    }

    async fn initialize(&self) -> anyhow::Result<()> {
        self.initialize_usb_mode().await?;
        let power_state = self.app_db.try_get::<u8>(ACTIVATED_NODES_KEY).await?;
        self.activate_slot(power_state, 0b1111).await
    }

    async fn initialize_usb_mode(&self) -> anyhow::Result<()> {
        let config = self.app_db.get::<UsbConfig>(USB_CONFIG).await;
        self.configure_usb(config).await.context("USB configure")
    }

    pub async fn get_usb_mode(&self) -> UsbConfig {
        self.app_db.get::<UsbConfig>(USB_CONFIG).await
    }

    /// routine to support legacy API
    pub async fn get_node_power(&self, node: NodeId) -> anyhow::Result<bool> {
        let state = self.app_db.try_get::<u8>(ACTIVATED_NODES_KEY).await?;
        Ok(state & node.to_bitfield() != 0)
    }

    /// This function is used to active a given node. Call this function if a
    /// module is inserted at that slot. Failing to call this method means that
    /// this slot is not considered for power up and power down commands.
    pub async fn activate_slot(&self, node_states: u8, mask: u8) -> anyhow::Result<()> {
        trace!(
            "activate slot. node_states={:#06b}, mask={:#06b}",
            node_states,
            mask
        );
        ensure!(mask != 0);

        let state = self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await;
        let new_state = (state & !mask) | (node_states & mask);

        self.update_power_on_times(state, node_states, mask).await;

        self.app_db.set::<u8>(ACTIVATED_NODES_KEY, new_state).await;
        debug!("node activated bits updated:{:#06b}.", new_state);

        let led = new_state != 0;
        self.power_controller.power_led(led).await?;

        // also update the actual power state accordingly
        self.power_controller
            .set_power_node(node_states, mask)
            .await
    }

    async fn update_power_on_times(&self, activated_nodes: u8, node_states: u8, mask: u8) {
        let mut node_infos = self.app_db.get::<NodeInfos>(NODE_INFO_KEY).await;

        for (idx, new_state) in bit_iterator(node_states, mask) {
            let current_state = activated_nodes & (1 << idx);
            let current_time = get_timestamp_unix();
            let node_info = &mut node_infos[idx];

            if new_state != current_state {
                if new_state == 1 {
                    node_info.power_on_time = current_time;
                } else {
                    node_info.power_on_time = None;
                }
            }
        }

        self.app_db
            .set::<NodeInfos>(NODE_INFO_KEY, node_infos)
            .await;
    }

    pub async fn configure_usb(&self, config: UsbConfig) -> anyhow::Result<()> {
        self.configure_usb_internal(config).await?;
        self.app_db.set(USB_CONFIG, config).await;
        Ok(())
    }

    async fn configure_usb_internal(&self, config: UsbConfig) -> anyhow::Result<()> {
        log::info!("changing usb config to {:?}", config);
        let (mode, dest, route) = match config {
            UsbConfig::UsbA(device) => (UsbMode::Device, device, UsbRoute::UsbA),
            UsbConfig::Bmc(device) => (UsbMode::Device, device, UsbRoute::Bmc),
            UsbConfig::Flashing(device, route) => (UsbMode::Flash, device, route),
            UsbConfig::Node(host, route) => (UsbMode::Host, host, route),
        };

        if mode != UsbMode::Flash {
            if let Err(e) = remove_msd_function_from_usb_gadget().await {
                log::error!("{:#}", e);
            }
        }

        self.pin_controller.set_usb_route(route).await?;
        self.pin_controller.select_usb(dest, mode)?;

        Ok(())
    }

    pub async fn usb_boot(&self, node: NodeId, on: bool) -> anyhow::Result<()> {
        let node_bits = node.to_bitfield();
        let (state, mask) = if on {
            (node_bits, node_bits)
        } else {
            (0u8, node_bits)
        };
        Ok(self.pin_controller.set_usb_boot(state, mask)?)
    }

    pub async fn rtl_reset(&self) -> anyhow::Result<()> {
        self.pin_controller.rtl_reset().await.context("rtl error")
    }

    pub async fn reset_node(&self, node: NodeId) -> anyhow::Result<()> {
        self.power_controller.reset_node(node).await
    }

    pub async fn node_in_msd(&self, node: NodeId) -> anyhow::Result<PathBuf> {
        // stop_usb_gadget_if_running().await?;

        self.reboot_into_usb(node, UsbConfig::Flashing(node, UsbRoute::Bmc))
            .await?;
        let blk_dev = self.node_drivers.load_as_block_device().await?;

        if let Err(e) = append_msd_config_to_usb_gadget(&blk_dev).await {
            log::error!("msd usb-gadget: {:#}", e);
        } else {
            info!("BMC-OTG: Node mass storage CDC enabled");
        }

        Ok(blk_dev)
    }

    pub async fn node_in_flash(
        &self,
        node: NodeId,
        router: UsbRoute,
    ) -> anyhow::Result<impl 'static + AsyncRead + AsyncWrite + AsyncSeek + Unpin> {
        self.reboot_into_usb(node, UsbConfig::Flashing(node, router))
            .await?;
        Ok(self.node_drivers.load_as_stream().await?)
    }

    async fn reboot_into_usb(&self, node: NodeId, config: UsbConfig) -> anyhow::Result<()> {
        log::info!("Powering off node {:?}...", node);
        self.activate_slot(!node.to_bitfield(), node.to_bitfield())
            .await?;
        self.configure_usb_internal(config).await?;

        log::info!("Powering on...");
        self.activate_slot(node.to_bitfield(), node.to_bitfield())
            .await?;

        tokio::time::sleep(Duration::from_secs(1)).await;

        self.clear_usb_boot()
    }

    pub fn clear_usb_boot(&self) -> anyhow::Result<()> {
        self.pin_controller
            .set_usb_boot(0u8, 0b1111)
            .context("error clearing usbboot")
    }

    pub async fn reboot() -> anyhow::Result<()> {
        tokio::fs::write("/sys/class/leds/fp:reset/brightness", b"1").await?;
        Command::new("shutdown").args(["-r", "now"]).spawn()?;
        Ok(())
    }

    pub async fn set_node_info(&self, new_info: HashMap<NodeId, NodeInfo>) -> anyhow::Result<()> {
        let mut stored_nodes = self.app_db.get::<NodeInfos>(NODE_INFO_KEY).await;

        for (i, info) in &mut new_info.into_iter().map(|(k, v)| (k as usize, v)) {
            let store_node = &mut stored_nodes[i];

            if let Some(name) = info.name {
                store_node.name = Some(name);
            }

            if let Some(module_name) = info.module_name {
                store_node.module_name = Some(module_name);
            }

            if let Some(uart_baud) = info.uart_baud {
                store_node.uart_baud = Some(uart_baud);
            }
        }

        self.app_db
            .set::<NodeInfos>(NODE_INFO_KEY, stored_nodes)
            .await;

        Ok(())
    }

    pub async fn get_node_infos(&self) -> anyhow::Result<NodeInfos> {
        let Some(current_time) = utils::get_timestamp_unix() else {
            anyhow::bail!("Current time before Unix epoch");
        };

        let mut node_infos = self.app_db.get::<NodeInfos>(NODE_INFO_KEY).await;
        node_infos.iter_mut().for_each(|info| {
            if let Some(time) = &mut info.power_on_time {
                *time = current_time - *time;
            }
        });

        Ok(node_infos)
    }
}
