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
use crate::hal::helpers::bit_iterator;
use crate::hal::NodeId;
use crate::hal::UsbMode;
use crate::hal::UsbRoute;
use tracing::warn;

pub struct PinController;

impl PinController {
    /// create a new Pin controller
    pub fn new() -> anyhow::Result<Self> {
        Ok(PinController)
    }

    pub fn select_usb(&self, node: NodeId, mode: UsbMode) -> std::io::Result<()> {
        warn!("select USB for node {:?}, mode:{:?}", node, mode);
        Ok(())
    }

    pub async fn set_usb_route(&self, route: UsbRoute) -> std::io::Result<()> {
        warn!("select USB route {:?}", route);
        Ok(())
    }

    pub fn set_usb_boot(&self, nodes_state: u8, nodes_mask: u8) -> std::io::Result<()> {
        let updates = bit_iterator(nodes_state, nodes_mask);

        for (idx, state) in updates {
            warn!(
                "updating usb_boot state of node {} to {}",
                idx + 1,
                if state != 0 { "enable" } else { "disable" }
            );
        }
        Ok(())
    }
}

impl std::fmt::Debug for PinController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PinController")
    }
}
