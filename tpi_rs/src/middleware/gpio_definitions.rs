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
//! This module contains static pin numbers related to GPIO
pub const GPIO_PIN_PG: u32 = 192;

pub const RTL_RESET: u32 = GPIO_PIN_PG + 13;
#[allow(unused)]
pub const SYS_RESET: u32 = GPIO_PIN_PG + 11;
#[allow(unused)]
pub const POWER_DETECT: u32 = GPIO_PIN_PG + 10;
#[allow(unused)]
pub const POWER_BOARD: u32 = GPIO_PIN_PG + 15;
pub const USB_SEL1: u32 = GPIO_PIN_PG + 1;
pub const USB_SEL2: u32 = GPIO_PIN_PG; // PG 0
pub const USB_OE1: u32 = GPIO_PIN_PG + 2;
pub const USB_OE2: u32 = GPIO_PIN_PG + 3;

pub const USB_SWITCH: u32 = GPIO_PIN_PG + 5;

// gpiochip1 aggregater
pub const NODE1_USBOTG_DEV: u32 = 2;
pub const NODE2_USBOTG_DEV: u32 = 6;
pub const NODE3_USBOTG_DEV: u32 = 10;
pub const NODE4_USBOTG_DEV: u32 = 14;

pub const PORT1_EN: u32 = 0;
pub const PORT2_EN: u32 = 4;
pub const PORT3_EN: u32 = 8;
pub const PORT4_EN: u32 = 12;

pub const PORT1_RPIBOOT: u32 = 3;
pub const PORT2_RPIBOOT: u32 = 7;
pub const PORT3_RPIBOOT: u32 = 11;
pub const PORT4_RPIBOOT: u32 = 15;
