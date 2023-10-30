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
use crate::hal::{helpers::bit_iterator, NodeId};
use log::warn;

// This structure is a thin layer that abstracts away the interaction details
// with Linux's power subsystem.
pub struct PowerController;

impl PowerController {
    pub fn new() -> anyhow::Result<Self> {
        Ok(PowerController)
    }

    pub async fn set_power_node(&self, node_states: u8, node_mask: u8) -> anyhow::Result<()> {
        let updates = bit_iterator(node_states, node_mask);

        for (idx, state) in updates {
            warn!("setting power of node {}. state:{}", idx + 1, state);
        }

        Ok(())
    }

    /// Reset a given node by setting the reset pin logically high for 1 second
    pub async fn reset_node(&self, node: NodeId) -> anyhow::Result<()> {
        warn!("reset node {:?}", node);
        Ok(())
    }

    pub async fn power_led(&self, on: bool) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::fmt::Debug for PowerController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PowerController")
    }
}
