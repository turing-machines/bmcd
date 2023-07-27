use crate::gpio_output_lines;

use super::gpio_definitions::*;
use super::NodeId;
use super::UsbMode;
use super::UsbRoute;
use anyhow::Context;
use gpiod::Active;
use gpiod::{Chip, Lines, Output};
use log::trace;
use std::time::Duration;
use tokio::time::sleep;

/// This middleware is responsible for controlling the gpio pins on the board, which includes USB
/// multiplexers. Due to hardware limitations, only one node can be connected over the USB bus at a
/// time. This structure the GPIOD device library internally.
pub struct PinController {
    usb_vbus: Lines<Output>,
    usb_mux: Lines<Output>,
    usb_pwen: Lines<Output>,
    usb_switch: Lines<Output>,
    rpi_boot: Lines<Output>,
    rtl_reset: Lines<Output>,
}

impl PinController {
    /// create a new Pin controller
    pub fn new() -> anyhow::Result<Self> {
        let chip0 = Chip::new("/dev/gpiochip0").context("gpiod chip0")?;
        let usb_vbus = gpio_output_lines!(
            chip0,
            Active::High,
            [
                PORT1_USB_VBUS,
                PORT2_USB_VBUS,
                PORT3_USB_VBUS,
                PORT4_USB_VBUS
            ]
        );

        let rpi_boot = gpio_output_lines!(
            chip0,
            Active::Low,
            [PORT1_RPIBOOT, PORT2_RPIBOOT, PORT3_RPIBOOT, PORT4_RPIBOOT]
        );
        let usb_mux =
            gpio_output_lines!(chip0, Active::High, [USB_SEL1, USB_OE1, USB_SEL2, USB_OE2]);
        let usb_switch = gpio_output_lines!(chip0, Active::High, [USB_SWITCH]);
        let usb_pwen = gpio_output_lines!(chip0, Active::Low, [USB_PWEN]);
        let rtl_reset = gpio_output_lines!(chip0, Active::Low, [RTL_RESET]);

        Ok(Self {
            usb_vbus,
            usb_mux,
            usb_pwen,
            usb_switch,
            rpi_boot,
            rtl_reset,
        })
    }

    /// Select which node is active in the multiplexer (see PORTx in `set_usb_route()`)
    pub fn select_usb(&self, node: NodeId, mode: UsbMode) -> std::io::Result<()> {
        trace!("select usb for node {:?}, mode:{:?}", node, mode);
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
        trace!("select usb route {:?}", route);
        match route {
            UsbRoute::UsbA => {
                self.usb_switch.set_values(0_u8)?;
                self.usb_pwen.set_values(1_u8)
            }
            UsbRoute::Bmc => {
                self.usb_switch.set_values(1_u8)?;
                self.usb_pwen.set_values(0_u8)
            }
        }
    }

    /// Set given nodes into usb boot mode. When powering the node on with this mode enabled, the
    /// given node will boot into USB mode. Typically means that booting of eMMC is disabled.
    pub fn set_usb_boot(&self, node: NodeId) -> std::io::Result<()> {
        trace!("setting usbboot {:#06b}", node.to_bitfield());
        self.rpi_boot.set_values(node.to_bitfield())
    }

    /// Clear USB boot mode of all nodes
    pub fn clear_usb_boot(&self) -> std::io::Result<()> {
        trace!("clearing usbboot pins");
        self.rpi_boot.set_values(0_u8)
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
