use crate::middleware::firmware_update::{
    fw_update_factory, FwUpdate, SUPPORTED_DEVICES, SUPPORTED_MSD_DEVICES,
};
use crate::middleware::power_controller::PowerController;
use crate::middleware::usbboot::{FlashProgress, FlashStatus};
use crate::middleware::{
    app_persistency::ApplicationPersistency, event_listener::EventListener,
    pin_controller::PinController, usbboot, NodeId, UsbMode, UsbRoute,
};
use anyhow::{ensure, Context};
use evdev::Key;
use log::{debug, info, trace};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncSeekExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

/// Stores which slots are actually used. This information is used to determine
/// for instance, which nodes need to be powered on, when such command is given
const ACTIVATED_NODES_KEY: &str = "activated_nodes";
/// stores to which node the USB multiplexer is configured to.
const USB_CONFIG: &str = "usb_config";

const REBOOT_DELAY: Duration = Duration::from_millis(500);

/// Describes the different configuration the USB bus can be setup
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum UsbConfig {
    /// USB-A port is host, NodeId is the device. 2nd argument specifies if the
    /// usbboot pin should be set.
    UsbA(NodeId, bool),
    /// BMC is host, NodeId is the device. 2nd argument specifies if the
    /// usbboot pin should be set.
    Bmc(NodeId, bool),
    /// NodeId is host, [UsbRoute] is configured for device
    Node(NodeId, UsbRoute),
}

#[derive(Debug)]
pub struct BmcApplication {
    pin_controller: PinController,
    power_controller: PowerController,
    app_db: ApplicationPersistency,
    nodes_on: AtomicBool,
}

impl BmcApplication {
    pub async fn new() -> anyhow::Result<Arc<Self>> {
        let pin_controller = PinController::new().context("pin_controller")?;
        let power_controller = PowerController::new().context("power_controller")?;
        let app_db = ApplicationPersistency::new()
            .await
            .context("application persistency")?;

        let instance = Arc::new(Self {
            pin_controller,
            power_controller,
            app_db,
            nodes_on: AtomicBool::new(false),
        });

        instance.initialize().await?;
        Self::run_event_listener(instance.clone()).context("event_listener")?;
        Ok(instance)
    }

    fn run_event_listener(instance: Arc<BmcApplication>) -> anyhow::Result<()> {
        EventListener::new(
            (instance, Option::<oneshot::Sender<()>>::None),
            "/dev/input/event0",
        )
        .add_action(Key::KEY_1, 1, |(app, s)| {
            let (sender, receiver) = oneshot::channel();
            *s = Some(sender);

            let bmc = app.clone();
            tokio::spawn(async move {
                let long_press = tokio::time::timeout(Duration::from_secs(3), receiver)
                    .await
                    .is_err();
                Self::toggle_power_states(bmc, long_press).await
            });
        })
        .add_action(Key::KEY_1, 0, |(_, sender)| {
            let _ = sender.take().and_then(|s| s.send(()).ok());
        })
        .add_action(Key::KEY_POWER, 1, |(app, _)| {
            tokio::spawn(Self::toggle_power_states(app.clone(), false));
        })
        .add_action(Key::KEY_RESTART, 1, |_| {
            tokio::spawn(reboot());
        })
        .run()
        .context("event_listener error")
    }

    async fn toggle_power_states(
        app: Arc<BmcApplication>,
        reset_activation: bool,
    ) -> anyhow::Result<()> {
        let mut node_values = app
            .app_db
            .get::<u8>(ACTIVATED_NODES_KEY)
            .await
            .unwrap_or_default();

        // assume that on the first time, the users want to activate the slots
        if node_values == 0 || reset_activation {
            node_values = if node_values < 15 { 0b1111 } else { 0b0000 };
            app.app_db.set(ACTIVATED_NODES_KEY, node_values).await?;
        }

        let current = app.nodes_on.load(Ordering::Relaxed);

        info!(
            "toggling nodes {:#6b} to {}. reset happened: {}",
            node_values,
            if current { "off" } else { "on" },
            reset_activation,
        );

        if current {
            app.power_off().await
        } else {
            app.power_on().await
        }
    }

    async fn initialize(&self) -> anyhow::Result<()> {
        self.initialize_usb_mode().await?;
        self.initialize_power().await
    }

    async fn initialize_power(&self) -> anyhow::Result<()> {
        if self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await.is_err() {
            // default, given a new app persistency
            self.app_db.set::<u8>(ACTIVATED_NODES_KEY, 0).await?;
        } else {
            self.power_on().await?;
        }
        Ok(())
    }

    async fn initialize_usb_mode(&self) -> anyhow::Result<()> {
        let config = self
            .app_db
            .get::<UsbConfig>(USB_CONFIG)
            .await
            .unwrap_or(UsbConfig::UsbA(NodeId::Node1, false));
        self.configure_usb(config).await.context("USB configure")
    }

    pub async fn get_usb_mode(&self) -> anyhow::Result<UsbConfig> {
        self.app_db.get::<UsbConfig>(USB_CONFIG).await
    }

    /// routine to support legacy API
    pub async fn get_node_power(&self, node: NodeId) -> anyhow::Result<bool> {
        if self.nodes_on.load(Ordering::Relaxed) {
            let state = self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await?;
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
        ensure!(node_states != 0);

        let state = self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await?;
        let new_state = (state & !mask) | (node_states & mask);

        if new_state != state {
            self.app_db
                .set::<u8>(ACTIVATED_NODES_KEY, new_state)
                .await?;
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
        let activated = self.app_db.get::<u8>(ACTIVATED_NODES_KEY).await?;
        self.nodes_on
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.power_controller
            .set_power_node(activated, activated)
            .await
    }

    pub async fn power_off(&self) -> anyhow::Result<()> {
        self.nodes_on.store(false, Ordering::Relaxed);
        self.power_controller.set_power_node(0b0000, 0b1111).await
    }

    pub async fn configure_usb(&self, config: UsbConfig) -> anyhow::Result<()> {
        let (mode, dest, route, usbboot) = match config {
            UsbConfig::UsbA(device, rpiboot) => {
                (UsbMode::Device, device, UsbRoute::UsbA, Some(rpiboot))
            }
            UsbConfig::Bmc(device, rpiboot) => {
                (UsbMode::Device, device, UsbRoute::Bmc, Some(rpiboot))
            }
            UsbConfig::Node(host, route) => (UsbMode::Host, host, route, None),
        };

        self.pin_controller.clear_usb_boot()?;
        self.pin_controller.set_usb_route(route).await?;
        self.pin_controller.select_usb(dest, mode)?;
        if let Some(true) = usbboot {
            self.pin_controller.set_usb_boot(dest)?;
        }
        self.app_db.set(USB_CONFIG, config).await?;
        Ok(())
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

    async fn configure_node_for_fwupgrade<'a, I>(
        &self,
        node: NodeId,
        router: UsbRoute,
        progress_sender: mpsc::Sender<FlashProgress>,
        any_of: I,
    ) -> anyhow::Result<Box<dyn FwUpdate>>
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
        self.pin_controller.clear_usb_boot()?;

        sleep(REBOOT_DELAY).await;

        let config = match router {
            UsbRoute::Bmc => UsbConfig::Bmc(node, true),
            UsbRoute::UsbA => UsbConfig::UsbA(node, true),
        };
        self.configure_usb(config).await?;

        progress_state.message = String::from("Prerequisite settings toggled, powering on...");
        progress_sender.send(progress_state.clone()).await?;

        self.activate_slot(node.to_bitfield(), node.to_bitfield())
            .await?;

        sleep(Duration::from_secs(2)).await;

        progress_state.message = String::from("Checking for presence of a USB device...");
        progress_sender.send(progress_state.clone()).await?;

        let usb_device = usbboot::find_first_usb_device(any_of).map_err(|e| {
            progress_sender
                .try_send(FlashProgress {
                    status: FlashStatus::Error(e),
                    message: String::new(),
                })
                .unwrap();
            e
        })?;

        fw_update_factory(&usb_device, progress_sender)?
            .await
            .context("USB driver init error")
    }

    pub async fn flash_node(
        self: Arc<BmcApplication>,
        node: NodeId,
        image_path: PathBuf,
        progress_sender: mpsc::Sender<FlashProgress>,
    ) -> anyhow::Result<()> {
        let mut driver = self
            .configure_node_for_fwupgrade(
                node,
                UsbRoute::Bmc,
                progress_sender.clone(),
                SUPPORTED_DEVICES.keys(),
            )
            .await?;

        let mut progress_state = FlashProgress {
            message: String::new(),
            status: FlashStatus::Setup,
        };

        progress_state.message = format!("Writing {:?}", image_path);
        progress_sender.send(progress_state.clone()).await?;

        let (img_len, img_checksum) =
            usbboot::write_to_device(image_path, &mut driver, &progress_sender).await?;

        progress_state.message = String::from("Verifying checksum...");
        progress_sender.send(progress_state.clone()).await?;

        driver.seek(std::io::SeekFrom::Start(0)).await?;

        usbboot::verify_checksum(img_checksum, img_len, &mut driver, &progress_sender).await?;

        progress_state.message = String::from("Flashing successful, restarting device...");
        progress_sender.send(progress_state.clone()).await?;

        self.activate_slot(!node.to_bitfield(), node.to_bitfield())
            .await?;

        //TODO: we probably want to restore the state prior flashing
        self.configure_usb(UsbConfig::UsbA(node, false)).await?;

        sleep(REBOOT_DELAY).await;

        self.activate_slot(node.to_bitfield(), node.to_bitfield())
            .await?;

        progress_state.message = String::from("Done");
        progress_sender.send(progress_state).await?;
        Ok(())
    }

    pub fn clear_usb_boot(&self) -> anyhow::Result<()> {
        self.pin_controller
            .clear_usb_boot()
            .context("error clearing usbboot")
    }
}

async fn reboot() -> anyhow::Result<()> {
    tokio::fs::write("/sys/class/leds/fp:reset/brightness", b"1").await?;
    Command::new("shutdown").args(["-r", "now"]).spawn()?;
    Ok(())
}
