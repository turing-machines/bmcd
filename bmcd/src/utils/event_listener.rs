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
use evdev::{Device, EventSummary, KeyCode};
use std::collections::{HashMap, HashSet};
use tracing::{debug, trace, warn};

type ActionFn<T> = Box<dyn Fn(&'_ mut T) + Send + Sync>;

/// Structure that listens for incoming device events Using a simple callback mechanism.
pub struct EventListener<T> {
    context: T,
    map: HashMap<(KeyCode, i32), ActionFn<T>>,
    device_path: &'static str,
}

impl<T: Send + Sync + 'static> EventListener<T> {
    pub fn new(context: T, device_path: &'static str) -> Self {
        Self {
            map: HashMap::new(),
            context,
            device_path,
        }
    }

    pub fn add_action<F>(mut self, key: KeyCode, value: i32, action: F) -> Self
    where
        F: Fn(&'_ mut T) + Send + Sync + 'static,
    {
        self.map.insert((key, value), Box::new(action));
        self
    }

    /// non-blocking call to start listening for events of interest.
    pub fn run(mut self) -> std::io::Result<()> {
        let device = Device::open(self.device_path)?;
        self.verify_required_keys(&device);

        let mut event_stream = device.into_event_stream()?;
        tokio::spawn(async move {
            while let Ok(event) = event_stream.next_event().await {
                trace!("processing event {:?}", event);
                if let EventSummary::Key(_, code, value) = event.destructure() {
                    if let Some(action) = self.map.get(&(code, value)) {
                        action(&mut self.context);
                    } else {
                        debug!("no handler defined for event {:?}", event);
                    }
                }
            }
            tracing::info!("shutting down event listener");
        });
        Ok(())
    }

    fn verify_required_keys(&self, device: &Device) {
        let required_keys = self
            .map
            .keys()
            .map(|(k, _)| *k)
            .collect::<HashSet<KeyCode>>();
        let verified_keys = device.supported_keys().map_or(HashSet::new(), |k| {
            k.iter()
                .filter(|k| required_keys.contains(k))
                .collect::<HashSet<KeyCode>>()
        });

        if required_keys != verified_keys {
            warn!(
                "not all required keys are supported by linux subsystem. need {:?}, supported {:?}",
                required_keys,
                device.supported_keys()
            );
        } else {
            debug!(
                "keys {:#?} are all supported by the subsystem",
                required_keys
            );
        }
    }
}
