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
use super::{
    helpers::{bit_iterator, load_lines},
    NodeId,
};
use crate::gpio_output_array;
use anyhow::Context;
use gpiod::{Chip, Lines, Output};
use log::{debug, trace};
use std::time::Duration;
use tokio::time::sleep;

const PORT1_EN: &str = "node1-en";
const PORT2_EN: &str = "node2-en";
const PORT3_EN: &str = "node3-en";
const PORT4_EN: &str = "node4-en";

// This structure is a thin layer that abstracts away the interaction details
// with Linux's power subsystem.
pub struct PowerController {
    enable: [Lines<Output>; 4],
}

impl PowerController {
    pub fn new() -> anyhow::Result<Self> {
        let chip1 = Chip::new("/dev/gpiochip1").context("gpiod chip1")?;
        let lines = load_lines(&chip1);
        let port1 = *lines
            .get(PORT1_EN)
            .ok_or(anyhow::anyhow!("cannot find PORT1_EN"))?;
        let port2 = *lines
            .get(PORT2_EN)
            .ok_or(anyhow::anyhow!("cannot find PORT2_EN"))?;
        let port3 = *lines
            .get(PORT3_EN)
            .ok_or(anyhow::anyhow!("cannot find PORT3_EN"))?;
        let port4 = *lines
            .get(PORT4_EN)
            .ok_or(anyhow::anyhow!("cannot find PORT4_EN"))?;

        let enable = gpio_output_array!(chip1, port1, port2, port3, port4);

        Ok(PowerController { enable })
    }

    /// Function to power on/off given nodes. Powering of the nodes is controlled by
    /// the Linux subsystem.
    ///
    /// # Arguments
    ///
    /// * `node_states`     bit-field representing the nodes on the turing-pi board,
    /// where bit 1 is on and 0 equals off.
    /// * `node_mask`       bit-field to describe which nodes to control.
    ///
    /// # Returns
    ///
    /// * `Ok(())` when routine was executed successfully.
    /// * `Err(io error)` in the case there was a failure to write to the Linux
    /// subsystem that handles the node powering.
    pub async fn set_power_node(&self, node_states: u8, node_mask: u8) -> anyhow::Result<()> {
        let updates = bit_iterator(node_states, node_mask);

        for (idx, state) in updates {
            trace!("setting power of node {}. state:{}", idx + 1, state);
            set_mode(idx + 1, state).await?;
            sleep(Duration::from_millis(100)).await;
            self.enable[idx].set_values(state)?;
        }

        Ok(())
    }

    /// Reset a given node by setting the reset pin logically high for 1 second
    pub async fn reset_node(&self, node: NodeId) -> anyhow::Result<()> {
        debug!("reset node {:?}", node);
        let bits = node.to_bitfield();

        self.set_power_node(0u8, bits).await?;
        sleep(Duration::from_secs(1)).await;
        self.set_power_node(bits, bits).await?;
        Ok(())
    }

    pub async fn power_led(&self, on: bool) -> anyhow::Result<()> {
        const SYS_LED: &str = "/sys/class/leds/fp:sys/brightness";
        tokio::fs::write(SYS_LED, if on { "1" } else { "0" })
            .await
            .context(SYS_LED)
    }
}

impl std::fmt::Debug for PowerController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PowerController")
    }
}

async fn set_mode(node_id: usize, node_state: u8) -> std::io::Result<()> {
    let node_value = if node_state > 0 {
        "enabled"
    } else {
        "disabled"
    };

    let sys_path = format!("/sys/bus/platform/devices/node{}-power/state", node_id);
    tokio::fs::write(sys_path, node_value).await
}
