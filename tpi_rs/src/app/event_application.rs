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
use super::bmc_application::BmcApplication;
use crate::utils::EventListener;
use anyhow::Context;
use evdev::Key;
use std::{sync::Arc, time::Duration};
use tokio::sync::oneshot;

pub fn run_event_listener(instance: Arc<BmcApplication>) -> anyhow::Result<()> {
    EventListener::new(
        (instance, Option::<oneshot::Sender<()>>::None),
        "/dev/input/event0",
    )
    .add_action(Key::KEY_1, 1, |(app, s)| {
        let (sender, receiver) = oneshot::channel();
        *s = Some(sender);

        let bmc = app.clone();
        tokio::spawn(async move {
            let long_press = tokio::time::timeout(Duration::from_secs(3), receiver)
                .await
                .is_err();
            bmc.toggle_power_states(long_press).await
        });
    })
    .add_action(Key::KEY_1, 0, |(_, sender)| {
        let _ = sender.take().and_then(|s| s.send(()).ok());
    })
    .add_action(Key::KEY_POWER, 1, move |(app, _)| {
        let bmc = app.clone();
        tokio::spawn(async move { bmc.toggle_power_states(false).await });
    })
    .add_action(Key::KEY_RESTART, 1, |_| {
        tokio::spawn(BmcApplication::reboot());
    })
    .run()
    .context("event_listener error")
}
