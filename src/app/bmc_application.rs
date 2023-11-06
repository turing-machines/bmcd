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
use crate::firmware_update::transport::FwUpdateTransport;
use crate::firmware_update::{fw_update_transport, SUPPORTED_DEVICES};
use crate::hal::PowerController;
use crate::hal::SerialConnections;
use crate::hal::{usb, NodeId, PinController, UsbMode, UsbRoute};
use crate::persistency::app_persistency::ApplicationPersistency;
use crate::persistency::app_persistency::PersistencyBuilder;
use crate::utils::{string_from_utf16, string_from_utf32};
use anyhow::{ensure, Context};
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;
use tokio::time::sleep;

/// Stores which slots are actually used. This information is used to determine
/// for instance, which nodes need to be powered on, when such command is given
pub const ACTIVATED_NODES_KEY: &str = "activated_nodes";
/// stores to which node the USB multiplexer is configured to.
pub const USB_CONFIG: &str = "usb_config";

const REBOOT_DELAY: Duration = Duration::from_millis(500);
/// Describes the different configuration the USB bus can be setup
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum UsbConfig {
    /// USB-A port is host, NodeId is the device.
    UsbA(NodeId),
    /// BMC is host, NodeId is the device.
    Bmc(NodeId),
    /// NodeId is host, [UsbRoute] is configured for device
    Node(NodeId, UsbRoute),
}

/// Encodings used when reading from a serial port
pub enum Encoding {
    Utf8,
    Utf16 { little_endian: bool },
    Utf32 { little_endian: bool },
}

#[derive(Debug)]
pub struct BmcApplication {
    pub(super) pin_controller: PinController,
    pub(super) power_controller: PowerController,
    pub(super) app_db: ApplicationPersistency,
    serial: SerialConnections,
}

impl BmcApplication {
    pub async fn new(database_write_timeout: Option<Duration>) -> anyhow::Result<Self> {
        let pin_controller = PinController::new().context("pin_controller")?;
        let power_controller = PowerController::new().context("power_controller")?;
        let app_db = PersistencyBuilder::default()
            .register_key(ACTIVATED_NODES_KEY, &0u8)
            .register_key(USB_CONFIG, &UsbConfig::UsbA(NodeId::Node1))
            .write_timeout(database_write_timeout)
            .build()
            .await?;
        let serial = SerialConnections::new()?;

        let instance = Self {
            pin_controller,
            power_controller,
            app_db,
            serial,
        };

        instance.initialize().await?;
        Ok(instance)
    }

    pub async fn toggle_power_states(&self, force_on: bool) -> anyhow::Result<()> {
        if force_on {
            return self.activate_slot(0b1111, 0b1111).await;
        }

        let mut node_values = self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await;
        node_values = if node_values < 15 { 0b1111 } else { 0b0000 };
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

        self.app_db.set::<u8>(ACTIVATED_NODES_KEY, new_state).await;
        debug!("node activated bits updated:{:#06b}.", new_state);

        let led = new_state != 0;
        self.power_controller.power_led(led).await?;

        // also update the actual power state accordingly
        self.power_controller
            .set_power_node(node_states, mask)
            .await
    }

    pub async fn configure_usb(&self, config: UsbConfig) -> anyhow::Result<()> {
        self.configure_usb_internal(config).await?;
        self.app_db.set(USB_CONFIG, config).await;
        Ok(())
    }

    async fn configure_usb_internal(&self, config: UsbConfig) -> anyhow::Result<()> {
        log::debug!("changing usb config to {:?}", config);
        let (mode, dest, route) = match config {
            UsbConfig::UsbA(device) => (UsbMode::Device, device, UsbRoute::UsbA),
            UsbConfig::Bmc(device) => (UsbMode::Device, device, UsbRoute::Bmc),
            UsbConfig::Node(host, route) => (UsbMode::Host, host, route),
        };

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

    pub async fn set_node_in_msd(&self, node: NodeId, router: UsbRoute) -> anyhow::Result<()> {
        self.configure_node_for_fwupgrade(node, router, SUPPORTED_DEVICES.keys())
            .await
            .map(|_| ())
    }

    pub async fn configure_node_for_fwupgrade<'a, I>(
        &self,
        node: NodeId,
        router: UsbRoute,
        any_of: I,
    ) -> anyhow::Result<Box<dyn FwUpdateTransport>>
    where
        I: IntoIterator<Item = &'a (u16, u16)>,
    {
        log::info!("Powering off node {:?}...", node);
        self.activate_slot(!node.to_bitfield(), node.to_bitfield())
            .await?;

        sleep(REBOOT_DELAY).await;

        let config = match router {
            UsbRoute::Bmc => UsbConfig::Bmc(node),
            UsbRoute::UsbA => UsbConfig::UsbA(node),
        };
        self.usb_boot(node, true).await?;
        self.configure_usb_internal(config).await?;

        log::info!("Prerequisite settings toggled, powering on...");
        self.activate_slot(node.to_bitfield(), node.to_bitfield())
            .await?;

        tokio::time::sleep(Duration::from_secs(1)).await;

        self.clear_usb_boot()?;
        log::info!("Checking for presence of a USB device...");

        let matches = usb::get_usb_devices(any_of)?;
        let usb_device = usb::extract_one_device(&matches)?;
        fw_update_transport(usb_device)?
            .await
            .context("USB driver init error")
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

    pub async fn start_serial_workers(&self) -> anyhow::Result<()> {
        Ok(self.serial.run().await?)
    }

    pub async fn serial_read(&self, node: NodeId, encoding: Encoding) -> anyhow::Result<String> {
        let bytes = self.serial.read(node).await?;

        let res = match encoding {
            Encoding::Utf8 => String::from_utf8_lossy(&bytes).to_string(),
            Encoding::Utf16 { little_endian } => string_from_utf16(&bytes, little_endian),
            Encoding::Utf32 { little_endian } => string_from_utf32(&bytes, little_endian),
        };
        Ok(res)
    }

    pub async fn serial_write(&self, node: NodeId, data: &[u8]) -> anyhow::Result<()> {
        Ok(self.serial.write(node, data).await?)
    }
}
