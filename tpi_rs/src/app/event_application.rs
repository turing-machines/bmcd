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
