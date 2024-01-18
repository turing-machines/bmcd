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
//! Handlers for UART connections to/from nodes
use std::ops::Index;

use super::serial_handler::Handler;
use crate::hal::NodeId;
use crate::serial_service::serial_handler::HandlerState;
use tokio_serial::{DataBits, Parity, StopBits};

/// Collection of [`crate::serial_service::serial_handler::Handler`]
#[derive(Debug)]
pub struct SerialConnections {
    handlers: Vec<Handler>,
}

impl SerialConnections {
    pub fn new() -> Self {
        let paths = ["/dev/ttyS2", "/dev/ttyS1", "/dev/ttyS4", "/dev/ttyS5"];

        let collection = paths.iter().enumerate().map(|(i, path)| {
            let mut handler = Handler::new(
                i + 1,
                path,
                115200,
                DataBits::Eight,
                Parity::None,
                StopBits::One,
            );
            handler.run().expect("handler run error");
            handler
        });

        SerialConnections {
            handlers: collection.collect(),
        }
    }

    pub fn get_state(&self) -> Vec<HandlerState> {
        self.handlers.iter().map(Handler::get_state).collect()
    }
}

impl Index<NodeId> for SerialConnections {
    type Output = Handler;

    fn index(&self, index: NodeId) -> &Self::Output {
        assert!(
            self.handlers.len() == 4,
            "Serial connections not initialized"
        );
        &self.handlers[index as usize]
    }
}
