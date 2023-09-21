//! This module contains static pin numbers related to GPIO
pub const GPIO_PIN_PG: u32 = 192;
pub const GPIO_PIN_PD: u32 = 96;

pub const RTL_RESET: u32 = GPIO_PIN_PG + 13;
#[allow(unused)]
pub const SYS_RESET: u32 = GPIO_PIN_PG + 11;
#[allow(unused)]
pub const POWER_DETECT: u32 = GPIO_PIN_PG + 10;
#[allow(unused)]
pub const POWER_BOARD: u32 = GPIO_PIN_PG + 15;

pub const PORT1_EN: u32 = GPIO_PIN_PD + 11;
pub const PORT2_EN: u32 = GPIO_PIN_PD + 10;
pub const PORT3_EN: u32 = GPIO_PIN_PD + 9;
pub const PORT4_EN: u32 = GPIO_PIN_PD + 8;

pub const MODE1_EN: u32 = GPIO_PIN_PD + 7;
pub const MODE2_EN: u32 = GPIO_PIN_PD + 6;
pub const MODE3_EN: u32 = GPIO_PIN_PD + 5;
pub const MODE4_EN: u32 = GPIO_PIN_PD + 4;
pub const POWER_EN: u32 = GPIO_PIN_PD + 3;

pub const PORT1_USB_VBUS: u32 = GPIO_PIN_PD + 19;
pub const PORT2_USB_VBUS: u32 = GPIO_PIN_PD + 18;
pub const PORT3_USB_VBUS: u32 = GPIO_PIN_PD + 17;
pub const PORT4_USB_VBUS: u32 = GPIO_PIN_PD + 16;

pub const PORT1_RPIBOOT: u32 = GPIO_PIN_PD + 15;
pub const PORT2_RPIBOOT: u32 = GPIO_PIN_PD + 14;
pub const PORT3_RPIBOOT: u32 = GPIO_PIN_PD + 12;
pub const PORT4_RPIBOOT: u32 = GPIO_PIN_PD + 13;

pub const USB_SEL1: u32 = GPIO_PIN_PG + 1;
pub const USB_SEL2: u32 = GPIO_PIN_PG; // PG 0
pub const USB_OE1: u32 = GPIO_PIN_PG + 2;
pub const USB_OE2: u32 = GPIO_PIN_PG + 3;

pub const USB_SWITCH: u32 = GPIO_PIN_PG + 5;
pub const USB_PWEN: u32 = GPIO_PIN_PG + 4;
