use std::time::Duration;

use anyhow::Context;
use gpiod::{Active, Chip, Lines, Output};
use log::trace;
use tokio::time::sleep;

use crate::setup_output_array;

use super::{gpio_definitions::*, NodeId};

const NODE_COUNT: u8 = 4;
// This structure is a thin layer that abstracts away the interaction details
// towards the power subsystem and gpio devices.
pub struct PowerController {
    reset: [Lines<Output>; 4],
    enable: [Lines<Output>; 4],
}

impl PowerController {
    pub fn new() -> anyhow::Result<Self> {
        let chip1 = Chip::new("/dev/gpiochip1").context("gpiod chip1")?;
        let enable =
            setup_output_array!(chip1, arr [PORT1_EN, PORT2_EN, PORT3_EN, PORT4_EN], Active::Low);

        let reset = setup_output_array!(
            chip1,
            arr [PORT1_RST, PORT2_RST, PORT3_RST, PORT4_RST],
            Active::High
        );

        Ok(PowerController { reset, enable })
    }

    /// Function to power on/off given nodes. powering of the nodes is controlled by
    /// the linux subsystem.
    ///
    /// # Arguments
    ///
    /// * `node_states`     bitfield representing the nodes on the turing-pi board,
    /// where bit 1 is on and 0 equals off.
    /// * `node_mask`       bitfield to describe which nodes to control.
    ///
    /// # Returns
    ///
    /// * `Ok(())` when routine was executed successfully.
    /// * `Err(io error)` in the case there was a failure to write to the linux
    /// subsystem that handles the node powering.
    pub async fn set_power_node(&self, node_states: u8, node_mask: u8) -> anyhow::Result<()> {
        let updates = (0..NODE_COUNT).filter_map(|n| {
            let mask = node_mask & (1 << n);
            let state = (node_states & mask) >> n;
            (mask != 0).then_some((n as usize, state))
        });

        for (idx, state) in updates {
            trace!("setting power of node {}. state:{}", idx + 1, state);
            set_mode(idx + 1, state).await?;
            sleep(Duration::from_millis(100)).await;
            self.enable[idx].set_values(state)?;
            sleep(Duration::from_millis(100)).await;
            self.reset[idx].set_values(state)?;
        }

        Ok(())
    }

    /// Reset a given node by setting the reset pin logically high for 1 second
    pub async fn reset_node(&self, node: NodeId) -> anyhow::Result<()> {
        trace!("reset node {:?}", node);
        let idx = node as usize;

        self.reset[idx].set_values(1u8)?;
        sleep(Duration::from_secs(1)).await;
        self.reset[idx].set_values(0u8)?;
        Ok(())
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

impl std::fmt::Debug for PowerController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PowerController")
    }
}
