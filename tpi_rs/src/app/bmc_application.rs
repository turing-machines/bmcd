use crate::middleware::firmware_update::transport::FwUpdateTransport;
use crate::middleware::firmware_update::{
    fw_update_transport, FlashProgress, FlashStatus, SUPPORTED_MSD_DEVICES,
};
use crate::middleware::persistency::app_persistency::ApplicationPersistency;
use crate::middleware::persistency::app_persistency::PersistencyBuilder;
use crate::middleware::power_controller::PowerController;
use crate::middleware::usbboot;
use crate::middleware::{pin_controller::PinController, NodeId, UsbMode, UsbRoute};
use anyhow::{ensure, Context};
use log::{debug, info, trace};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
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

#[derive(Debug)]
pub struct BmcApplication {
    pub(super) pin_controller: PinController,
    pub(super) power_controller: PowerController,
    pub(super) app_db: ApplicationPersistency,
    pub(super) nodes_on: AtomicBool,
}

impl BmcApplication {
    pub async fn new() -> anyhow::Result<Self> {
        let pin_controller = PinController::new().context("pin_controller")?;
        let power_controller = PowerController::new().context("power_controller")?;
        let app_db = PersistencyBuilder::default()
            .register_key(ACTIVATED_NODES_KEY, &0u8)
            .register_key(USB_CONFIG, &UsbConfig::UsbA(NodeId::Node1))
            .build()
            .await?;

        let instance = Self {
            pin_controller,
            power_controller,
            app_db,
            nodes_on: AtomicBool::new(false),
        };

        instance.initialize().await?;
        Ok(instance)
    }

    pub async fn toggle_power_states(&self, reset_activation: bool) -> anyhow::Result<()> {
        let mut node_values = self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await;

        // assume that on the first time, the users want to activate the slots
        if node_values == 0 || reset_activation {
            node_values = if node_values < 15 { 0b1111 } else { 0b0000 };
            self.app_db.set(ACTIVATED_NODES_KEY, node_values).await;
        }

        let current = self.nodes_on.load(Ordering::Relaxed);

        info!(
            "toggling nodes {:#6b} to {}. reset: {}",
            node_values,
            if current { "off" } else { "on" },
            reset_activation,
        );

        if current {
            self.power_off().await
        } else {
            self.power_on().await
        }
    }

    async fn initialize(&self) -> anyhow::Result<()> {
        self.initialize_usb_mode().await?;
        self.initialize_power().await
    }

    async fn initialize_power(&self) -> anyhow::Result<()> {
        if self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await != 0 {
            self.power_on().await?;
        }
        Ok(())
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
        if self.nodes_on.load(Ordering::Relaxed) {
            let state = self.app_db.try_get::<u8>(ACTIVATED_NODES_KEY).await?;
            Ok(state & node.to_bitfield() != 0)
        } else {
            Ok(false)
        }
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

        if new_state != state {
            self.app_db.set::<u8>(ACTIVATED_NODES_KEY, new_state).await;
            debug!("node activated bits updated:{:#06b}.", new_state);
        }

        if new_state == 0 {
            self.nodes_on.store(false, Ordering::Relaxed);
        } else {
            self.nodes_on.store(true, Ordering::Relaxed);
        }

        // also update the actual power state accordingly
        self.power_controller
            .set_power_node(node_states, mask)
            .await
    }

    pub async fn power_on(&self) -> anyhow::Result<()> {
        let activated = self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await;
        self.nodes_on.store(true, Ordering::Relaxed);
        self.power_controller.power_led(true).await?;
        self.power_controller
            .set_power_node(activated, activated)
            .await
    }

    pub async fn power_off(&self) -> anyhow::Result<()> {
        self.nodes_on.store(false, Ordering::Relaxed);
        self.power_controller.power_led(false).await?;
        self.power_controller.set_power_node(0b0000, 0b1111).await
    }

    pub async fn configure_usb(&self, config: UsbConfig) -> anyhow::Result<()> {
        log::debug!("changing usb config to {:?}", config);
        let (mode, dest, route) = match config {
            UsbConfig::UsbA(device) => (UsbMode::Device, device, UsbRoute::UsbA),
            UsbConfig::Bmc(device) => (UsbMode::Device, device, UsbRoute::Bmc),
            UsbConfig::Node(host, route) => (UsbMode::Host, host, route),
        };

        self.pin_controller.set_usb_route(route).await?;
        self.pin_controller.select_usb(dest, mode)?;
        self.app_db.set(USB_CONFIG, config).await;
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

    pub async fn set_node_in_msd(
        &self,
        node: NodeId,
        router: UsbRoute,
        progress_sender: mpsc::Sender<FlashProgress>,
    ) -> anyhow::Result<()> {
        // The SUPPORTED_MSD_DEVICES list contains vid_pids of USB drivers we know will load the
        // storage of a node as a MSD device.
        self.configure_node_for_fwupgrade(
            node,
            router,
            progress_sender,
            SUPPORTED_MSD_DEVICES.deref(),
        )
        .await
        .map(|_| ())
    }

    pub async fn configure_node_for_fwupgrade<'a, I>(
        &self,
        node: NodeId,
        router: UsbRoute,
        progress_sender: mpsc::Sender<FlashProgress>,
        any_of: I,
    ) -> anyhow::Result<Box<dyn FwUpdateTransport>>
    where
        I: IntoIterator<Item = &'a (u16, u16)>,
    {
        let mut progress_state = FlashProgress {
            message: String::new(),
            status: FlashStatus::Idle,
        };

        progress_state.message = format!("Powering off node {:?}...", node);
        progress_state.status = FlashStatus::Progress {
            read_percent: 0,
            est_minutes: u64::MAX,
            est_seconds: u64::MAX,
        };
        progress_sender.send(progress_state.clone()).await?;

        self.activate_slot(!node.to_bitfield(), node.to_bitfield())
            .await?;
        self.pin_controller
            .set_usb_boot(!node.to_bitfield(), node.to_bitfield())?;

        sleep(REBOOT_DELAY).await;

        let config = match router {
            UsbRoute::Bmc => UsbConfig::Bmc(node),
            UsbRoute::UsbA => UsbConfig::UsbA(node),
        };
        self.usb_boot(node, true).await?;
        self.configure_usb(config).await?;

        progress_state.message = String::from("Prerequisite settings toggled, powering on...");
        progress_sender.send(progress_state.clone()).await?;

        self.activate_slot(node.to_bitfield(), node.to_bitfield())
            .await?;

        tokio::time::sleep(Duration::from_secs(2)).await;

        progress_state.message = String::from("Checking for presence of a USB device...");
        progress_sender.send(progress_state.clone()).await?;

        let matches = usbboot::get_usb_devices(any_of)?;
        let usb_device = usbboot::extract_one_device(&matches).map_err(|e| {
            progress_sender
                .try_send(FlashProgress {
                    status: FlashStatus::Error(e),
                    message: String::new(),
                })
                .unwrap();
            e
        })?;

        fw_update_transport(usb_device, progress_sender)?
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
}
