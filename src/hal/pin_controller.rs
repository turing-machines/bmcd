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
use super::helpers::load_lines;
use crate::gpio_output_array;
use crate::gpio_output_lines;

use super::gpio_definitions::*;
use super::NodeId;
use super::UsbMode;
use super::UsbRoute;
use anyhow::Context;
use gpiod::{Chip, Lines, Output};
use thiserror::Error;
use tracing::debug;

const USB_PORT_POWER: &str = "/sys/bus/platform/devices/usb-port-power/state";

const NODE1_USBOTG_DEV: &str = "node1-usbotg-dev";
const NODE2_USBOTG_DEV: &str = "node2-usbotg-dev";
const NODE3_USBOTG_DEV: &str = "node3-usbotg-dev";
const NODE4_USBOTG_DEV: &str = "node4-usbotg-dev";

const NODE1_RPIBOOT: &str = "node1-rpiboot";
const NODE2_RPIBOOT: &str = "node2-rpiboot";
const NODE3_RPIBOOT: &str = "node3-rpiboot";
const NODE4_RPIBOOT: &str = "node4-rpiboot";

/// This class is responsible for switching USB busses to the various "USB
/// endpoints", e.g. a USB port on the bus or a connection to the BMC(t113). The
/// hardware changed over time, and depending on which version of the board is
/// used different usecases can be realized. To expose an interface that has the
/// best support for all hardware platforms, the USB switch is seen as a
/// blackbox, which connects the Nodes with the BMC or the USB_OTG port.
///
///```text
///                                           ┌─────┐
/// ┌────────────────────┐     ┌─────────┐    │     │
/// │                    │     │         │    │Node1│
/// │  4XNODE USB_OTG /  │     │         ├────┤     │
/// │  (v2.4) USB-A      ├─────┤         │    └─────┘
/// │                    │     │USB switch
/// └────────────────────┘     │Blackbox │    ┌─────┐
///                            │         │    │     │
/// ┌────────┐                 │         ├────┼Node2│
/// │        │                 │         │    │     │
/// │  T113  ├─────────────────┤         │    └─────┘
/// │        │                 │         │
/// └────────┘                 │         ├─┐  ┌─────┐
///                            └─────────┘ │  │     │
///                                        └──┤N... │
///                                           │     │
///                                           └─────┘
/// ```
///
/// # Turing Pi v2.4
///
/// The hardware of the turing pi v2.4 is capable to switch one node to either
/// the USB-A port on the board or to the T113. Both the Node or the USB_OTG/T113
/// can be put in host mode. The only limitation is that only one connection can
/// be routed.
///
/// # Turing Pi >= v2.5
///
/// On the turing pi 2.5 the USB-A port is replaced with an USB-C port and
/// relabeled to 4XNODE_USB_OTG. The 2.5 board still has an USB-A port, but is
/// exclusively connected to Node1.
///
/// ```text
///                            ┌───────────────────┐
///                       ┌────┴─┐                 │
/// ┌───────┐             │      │    ┌──────┐     │
/// │ USB-A ├─────────────┤switch├────┤switch├───┐ │
/// └───────┴             └──────┘    └─┬────┘   │ │
///                                     │     ┌──┴─┴┐
/// ┌────────────────────┐     ┌────────┴┐    │     │
/// │                    │     │         │    │Node1│
/// │  4XNODE USB_OTG /  │     │         │    │     │
/// │  (v2.4) USB-A      ├─────┤         │    └─────┘
/// │                    │     │USB switch
/// └────────────────────┘     │Blackbox │    ┌─────┐
/// ```
///
/// The switches enable us to connect different USB ports on Node1 to the switch
/// or USB-A. This way we can provide support to more devices devices.
pub struct PinController {
    usb_switch: Box<dyn UsbConfiguration + Sync + Send>,
    rpi_boot: [Lines<Output>; 4],
}

impl PinController {
    /// create a new Pin controller
    pub fn new(has_usb_switch: bool) -> anyhow::Result<Self> {
        let chip1 = if has_usb_switch {
            "/dev/gpiochip1"
        } else {
            "/dev/gpiochip2"
        };

        let chip0 = Chip::new("/dev/gpiochip0").context("gpiod chip0")?;
        let chip1 = Chip::new(chip1).context("gpiod chip1")?;
        let chip1_lines = load_lines(&chip1);

        let rpi1 = *chip1_lines
            .get(NODE1_RPIBOOT)
            .ok_or(anyhow::anyhow!("cannot find node1-rpiboot gpio"))?;
        let rpi2 = *chip1_lines
            .get(NODE2_RPIBOOT)
            .ok_or(anyhow::anyhow!("cannot find node2-rpiboot gpio"))?;
        let rpi3 = *chip1_lines
            .get(NODE3_RPIBOOT)
            .ok_or(anyhow::anyhow!("cannot find node3-rpiboot gpio"))?;
        let rpi4 = *chip1_lines
            .get(NODE4_RPIBOOT)
            .ok_or(anyhow::anyhow!("cannot find node4-rpiboot gpio"))?;

        let rpi_boot = gpio_output_array!(chip1, rpi1, rpi2, rpi3, rpi4);

        let usb_switch = if has_usb_switch {
            Box::new(UsbMuxSwitch::new(&chip0, &chip1)?) as Box<dyn UsbConfiguration + Send + Sync>
        } else {
            // the node1 switching is not yet implemented. Make sure that the
            // switch routes the usb0 to the USB hub and usb2 to the the usbA
            // port.
            let node_source =
                gpio_output_lines!(chip0, [NODE1_OUTPUT_SWITCH_V2_5, NODE1_SOURCE_SWITCH_V2_5]);
            node_source.set_values(0u8)?;

            Box::new(UsbHub::new(&chip0)?)
        };

        Ok(Self {
            usb_switch,
            rpi_boot,
        })
    }

    /// Select which node is active in the multiplexer (see PORTx in `set_usb_route()`)
    pub fn select_usb(&self, node: NodeId, mode: UsbMode) -> Result<(), PowerControllerError> {
        debug!("select USB for node {:?}, mode:{:?}", node, mode);
        self.usb_switch.configure_usb(node, mode)?;

        if UsbMode::Flash == mode {
            self.set_usb_boot(node.to_bitfield(), node.to_bitfield())?;
        } else {
            self.set_usb_boot(0, 0b1111)?;
        }

        Ok(())
    }

    /// Set which way the USB is routed: USB-A ↔ PORTx (`UsbRoute::AlternativePort`) or BMC ↔ PORTx
    /// (`UsbRoute::Bmc`)
    pub fn set_usb_route(&self, route: UsbRoute) -> Result<(), PowerControllerError> {
        debug!("select USB route {:?}", route);
        self.usb_switch.set_usb_route(route)
    }

    /// Set given nodes into usb boot mode. When powering the node on with this mode enabled, the
    /// given node will boot into USB mode. Typically means that booting of eMMC is disabled.
    pub fn set_usb_boot(
        &self,
        nodes_state: u8,
        nodes_mask: u8,
    ) -> Result<(), PowerControllerError> {
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

    pub async fn rtl_reset(&self) -> Result<(), PowerControllerError> {
        //  self.rtl_reset.set_values(1u8)?;
        //  sleep(Duration::from_secs(1)).await;
        //  self.rtl_reset.set_values(0u8)?;
        //  Ok(())
        todo!()
    }
}

trait UsbConfiguration {
    fn set_usb_route(&self, route: UsbRoute) -> Result<(), PowerControllerError>;
    fn configure_usb(&self, node: NodeId, mode: UsbMode) -> Result<(), PowerControllerError>;
}

struct UsbMuxSwitch {
    usb_mux: Lines<Output>,
    usb_vbus: Lines<Output>,
    output_switch: Lines<Output>,
}

impl UsbMuxSwitch {
    pub fn new(chip0: &Chip, chip1: &Chip) -> Result<Self, PowerControllerError> {
        let usb_mux = gpio_output_lines!(chip0, [USB_SEL1, USB_OE1, USB_SEL2, USB_OE2]);
        let output_switch = gpio_output_lines!(chip0, [USB_SWITCH]);
        let chip1_lines = load_lines(chip1);

        let usb1 = *chip1_lines
            .get(NODE1_USBOTG_DEV)
            .ok_or(anyhow::anyhow!("cannot find node-1-usbotg-dev gpio"))?;
        let usb2 = *chip1_lines
            .get(NODE2_USBOTG_DEV)
            .ok_or(anyhow::anyhow!("cannot find node-2-usbotg-dev gpio"))?;
        let usb3 = *chip1_lines
            .get(NODE3_USBOTG_DEV)
            .ok_or(anyhow::anyhow!("cannot find node-3-usbotg-dev gpio"))?;
        let usb4 = *chip1_lines
            .get(NODE4_USBOTG_DEV)
            .ok_or(anyhow::anyhow!("cannot find node-4-usbotg-dev gpio"))?;

        let usb_vbus = gpio_output_lines!(chip1, [usb1, usb2, usb3, usb4]);
        Ok(Self {
            usb_mux,
            usb_vbus,
            output_switch,
        })
    }
}

impl UsbConfiguration for UsbMuxSwitch {
    fn set_usb_route(&self, route: UsbRoute) -> Result<(), PowerControllerError> {
        match route {
            UsbRoute::AlternativePort => {
                self.output_switch.set_values(0_u8)?;
                std::fs::write(USB_PORT_POWER, b"enabled")
            }
            UsbRoute::Bmc => {
                self.output_switch.set_values(1_u8)?;
                std::fs::write(USB_PORT_POWER, b"disabled")
            }
        }?;

        Ok(())
    }

    fn configure_usb(&self, node: NodeId, mode: UsbMode) -> Result<(), PowerControllerError> {
        let values: u8 = match node {
            NodeId::Node1 => 0b1100,
            NodeId::Node2 => 0b1101,
            NodeId::Node3 => 0b0011,
            NodeId::Node4 => 0b0111,
        };
        self.usb_mux.set_values(values)?;
        let vbus = match mode {
            UsbMode::Host => node.to_inverse_bitfield(),
            UsbMode::Device | UsbMode::Flash => 0b1111,
        };
        self.usb_vbus.set_values(vbus)?;
        Ok(())
    }
}

struct UsbHub {
    output_switch: Lines<Output>,
}

impl UsbHub {
    pub fn new(chip: &Chip) -> Result<Self, PowerControllerError> {
        let output_switch = gpio_output_lines!(chip, [USB_SWITCH_V2_5]);
        Ok(Self { output_switch })
    }
}

impl UsbConfiguration for UsbHub {
    fn set_usb_route(&self, route: UsbRoute) -> Result<(), PowerControllerError> {
        match route {
            UsbRoute::AlternativePort => self.output_switch.set_values(0_u8),
            UsbRoute::Bmc => self.output_switch.set_values(1_u8),
        }?;

        Ok(())
    }

    fn configure_usb(&self, _: NodeId, mode: UsbMode) -> Result<(), PowerControllerError> {
        if mode == UsbMode::Host {
            return Err(PowerControllerError::HostModeNotSupported);
        }
        // nothing to do as all nodes are already connected as USB devices to
        // the USB hub.
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum PowerControllerError {
    #[error(
        "Selecting one of the nodes as USB Host role \
        is not supported by the current hardware"
    )]
    HostModeNotSupported,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}
