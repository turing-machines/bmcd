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
use anyhow::{anyhow, bail, Context};
use std::{
    ops::Deref,
    path::Path,
    process::{Command, Stdio},
};
use tokio::{
    fs::{symlink, OpenOptions},
    io::AsyncWriteExt,
    task::spawn_blocking,
};

use crate::utils::logging_sink_stdio;

const BMC_USB_OTG: &str = "/sys/kernel/config/usb_gadget/g1";

pub async fn append_msd_config_to_usb_gadget(block_device: &Path) -> anyhow::Result<()> {
    if is_gadget_running().await? {
        remove_msd_function_from_usb_gadget().await?;
        usb_gadget_service(GadgetCmd::Stop)
            .await
            .context("usb_gadget")?;
    }

    let usb_gadget = Path::new(BMC_USB_OTG);
    if !usb_gadget.exists() {
        bail!("{} does not exist", BMC_USB_OTG);
    }

    let config = usb_gadget.join("configs/c.1");

    let mass_storage_function = usb_gadget.join("functions/mass_storage.0");
    tokio::fs::create_dir_all(&mass_storage_function)
        .await
        .with_context(|| mass_storage_function.to_string_lossy().to_string())?;

    let lun0 = mass_storage_function.join("lun.0/file");
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(lun0)
        .await?;

    file.write_all(
        block_device
            .to_str()
            .ok_or(anyhow!(
                "{} not convertable to string",
                block_device.to_str().unwrap_or_default()
            ))?
            .as_bytes(),
    )
    .await?;

    symlink(&mass_storage_function, &config.join("mass_storage.0"))
        .await
        .with_context(|| {
            format!(
                "symlink {} to {}",
                mass_storage_function.to_string_lossy(),
                config.to_string_lossy()
            )
        })?;

    usb_gadget_service(GadgetCmd::Start)
        .await
        .context("usb_gadget")?;

    Ok(())
}

pub async fn remove_msd_function_from_usb_gadget() -> anyhow::Result<()> {
    let mass_storage_config = Path::new(BMC_USB_OTG).join("configs/c.1/mass_storage.0");
    if mass_storage_config.exists() {
        usb_gadget_service(GadgetCmd::Stop).await?;
        tokio::fs::remove_file(&mass_storage_config)
            .await
            .with_context(|| mass_storage_config.to_string_lossy().to_string())?;
        usb_gadget_service(GadgetCmd::Start).await?;
    }
    Ok(())
}

async fn is_gadget_running() -> anyhow::Result<bool> {
    let udc = Path::new(BMC_USB_OTG).join("UDC");
    Ok(tokio::fs::read_to_string(udc)
        .await
        .map(|s| !s.trim().is_empty())?)
}

enum GadgetCmd {
    Start,
    Stop,
}

impl Deref for GadgetCmd {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            GadgetCmd::Start => "start",
            GadgetCmd::Stop => "stop",
        }
    }
}

async fn usb_gadget_service(command: GadgetCmd) -> anyhow::Result<()> {
    let output = spawn_blocking(move || {
        Command::new("/etc/init.d/S11bmc-otg")
            .arg(command.deref())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    })
    .await??;

    logging_sink_stdio(&output).await?;

    if !output.status.success() {
        bail!("S11bmc-otg: {}", output.status);
    }

    Ok(())
}
