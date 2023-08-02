use super::{gpio_definitions::*, NodeId};
use crate::{gpio_output_array, gpio_output_lines};
use anyhow::Context;
use gpiod::{Active, Chip, Lines, Output};
use log::{debug, error, trace};
use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::Duration,
};
use tokio::time::sleep;

const NODE_COUNT: u8 = 4;
// This structure is a thin layer that abstracts away the interaction details
// towards the power subsystem and gpio devices.
pub struct PowerController {
    mode: [Lines<Output>; 4],
    reset: [Lines<Output>; 4],
    enable: [Lines<Output>; 4],
    atx: Lines<Output>,
    cache: AtomicU8,
}

impl PowerController {
    pub fn new() -> anyhow::Result<Self> {
        let chip0 = Chip::new("/dev/gpiochip0").context("gpiod chip0")?;

        let current_state = {
            let inputs = chip0.request_lines(gpiod::Options::input([
                PORT1_EN, PORT2_EN, PORT3_EN, PORT4_EN,
            ]))?;
            inputs.get_values(0b0000u8)?
        };

        let mode = gpio_output_array!(chip0, Active::High, MODE1_EN, MODE2_EN, MODE3_EN, MODE4_EN);
        let enable = gpio_output_array!(chip0, Active::Low, PORT1_EN, PORT2_EN, PORT3_EN, PORT4_EN);
        let reset = gpio_output_array!(
            chip0,
            Active::High,
            PORT1_RST,
            PORT2_RST,
            PORT3_RST,
            PORT4_RST
        );

        let atx = gpio_output_lines!(chip0, Active::High, [POWER_EN]);
        atx.set_values(0b1_u8)?;

        let cache = AtomicU8::new(current_state);
        debug!("cache value:{:#06b}", cache.load(Ordering::Relaxed));

        Ok(PowerController {
            mode,
            reset,
            enable,
            atx,
            cache,
        })
    }

    fn update_state_and_atx(&self, node_states: u8, node_mask: u8) -> anyhow::Result<u8> {
        let current = self
            .cache
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some((current & !node_mask) | (node_states & node_mask))
            })
            .expect("cas always returns Some");
        let new = self.cache.load(Ordering::Relaxed);

        if let Some(on) = need_atx_change(current, new) {
            self.atx.set_values([on])?;
        }

        Ok(new)
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
        if let Err(e) = self.update_state_and_atx(node_states, node_mask) {
            error!("error updating atx regulator {}", e);
        }

        let updates = (0..NODE_COUNT).filter_map(|n| {
            let mask = node_mask & (1 << n);
            let state = (node_states & mask) >> n;
            (mask != 0).then_some((n as usize, state))
        });

        for (idx, state) in updates {
            trace!("setting power of node {}. state:{}", idx + 1, state);
            self.mode[idx].set_values(state)?;
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

        self.reset[idx].set_values(0u8)?;
        sleep(Duration::from_secs(1)).await;
        self.reset[idx].set_values(1u8)?;
        Ok(())
    }
}

/// Helper function that returns the new state of ATX power
fn need_atx_change(current_node_state: u8, next_node_state: u8) -> Option<bool> {
    if current_node_state == 0 && next_node_state > 0 {
        Some(true)
    } else if current_node_state > 0 && next_node_state == 0 {
        Some(false)
    } else {
        None
    }
}

impl std::fmt::Debug for PowerController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PowerController")
    }
}
