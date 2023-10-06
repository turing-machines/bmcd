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
use super::helpers::bit_iterator;
use crate::gpio_output_array;
use crate::gpio_output_lines;

use super::gpio_definitions::*;
use super::NodeId;
use super::UsbMode;
use super::UsbRoute;
use anyhow::Context;
use gpiod::{Chip, Lines, Output};
use log::debug;
use std::time::Duration;
use tokio::time::sleep;
const USB_PORT_POWER: &str = "/sys/bus/platform/devices/usb-port-power/state";

/// This middleware is responsible for controlling the gpio pins on the board, which includes USB
/// multiplexers. Due to hardware limitations, only one node can be connected over the USB bus at a
/// time. This structure the GPIOD device library internally.
pub struct PinController {
    usb_vbus: Lines<Output>,
    usb_mux: Lines<Output>,
    usb_switch: Lines<Output>,
    rpi_boot: [Lines<Output>; 4],
    rtl_reset: Lines<Output>,
}

impl PinController {
    /// create a new Pin controller
    pub fn new() -> anyhow::Result<Self> {
        let chip0 = Chip::new("/dev/gpiochip0").context("gpiod chip0")?;
        let chip1 = Chip::new("/dev/gpiochip1").context("gpiod chip1")?;
        let usb_vbus = gpio_output_lines!(
            chip1,
            [
                NODE1_USBOTG_DEV,
                NODE2_USBOTG_DEV,
                NODE3_USBOTG_DEV,
                NODE4_USBOTG_DEV
            ]
        );

        let rpi_boot = gpio_output_array!(
            chip1,
            PORT1_RPIBOOT,
            PORT2_RPIBOOT,
            PORT3_RPIBOOT,
            PORT4_RPIBOOT
        );
        let usb_mux = gpio_output_lines!(chip0, [USB_SEL1, USB_OE1, USB_SEL2, USB_OE2]);
        let usb_switch = gpio_output_lines!(chip0, [USB_SWITCH]);
        let rtl_reset = chip0
            .request_lines(gpiod::Options::output([RTL_RESET]).active(gpiod::Active::Low))
            .context(concat!("error initializing pin rtl reset"))?;

        Ok(Self {
            usb_vbus,
            usb_mux,
            usb_switch,
            rpi_boot,
            rtl_reset,
        })
    }

    /// Select which node is active in the multiplexer (see PORTx in `set_usb_route()`)
    pub fn select_usb(&self, node: NodeId, mode: UsbMode) -> std::io::Result<()> {
        debug!("select USB for node {:?}, mode:{:?}", node, mode);
        let values: u8 = match node {
            NodeId::Node1 => 0b1100,
            NodeId::Node2 => 0b1101,
            NodeId::Node3 => 0b0011,
            NodeId::Node4 => 0b0111,
        };
        self.usb_mux.set_values(values)?;

        let vbus = match mode {
            UsbMode::Host => node.to_inverse_bitfield(),
            UsbMode::Device => 0b1111,
        };
        self.usb_vbus.set_values(vbus)
    }

    /// Set which way the USB is routed: USB-A ↔ PORTx (`UsbRoute::UsbA`) or BMC ↔ PORTx
    /// (`UsbRoute::Bmc`)
    pub async fn set_usb_route(&self, route: UsbRoute) -> std::io::Result<()> {
        debug!("select USB route {:?}", route);
        match route {
            UsbRoute::UsbA => {
                self.usb_switch.set_values(0_u8)?;
                tokio::fs::write(USB_PORT_POWER, b"enabled").await
            }
            UsbRoute::Bmc => {
                self.usb_switch.set_values(1_u8)?;
                tokio::fs::write(USB_PORT_POWER, b"disabled").await
            }
        }
    }

    /// Set given nodes into usb boot mode. When powering the node on with this mode enabled, the
    /// given node will boot into USB mode. Typically means that booting of eMMC is disabled.
    pub fn set_usb_boot(&self, nodes_state: u8, nodes_mask: u8) -> std::io::Result<()> {
        let updates = bit_iterator(nodes_state, nodes_mask);

        for (idx, state) in updates {
            debug!(
                "updating usb_boot state of node {} to {}",
                idx + 1,
                if state != 0 { "enable" } else { "disable" }
            );
            self.rpi_boot[idx].set_values(state)?;
        }
        Ok(())
    }

    pub async fn rtl_reset(&self) -> std::io::Result<()> {
        self.rtl_reset.set_values(1u8)?;
        sleep(Duration::from_secs(1)).await;
        self.rtl_reset.set_values(0u8)
    }
}

impl std::fmt::Debug for PinController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PinController")
    }
}
