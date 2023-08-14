use crate::middleware::usbboot::{FlashProgress, FlashStatus, FlashingError};

use std::{path::PathBuf, time::Duration};
use tokio::{fs::File, sync::mpsc::Sender, time::sleep};

pub const VID_PID: (u16, u16) = (0x0a5c, 0x2711);
pub async fn new_rpi_transport(logging: &Sender<FlashProgress>) -> Result<File, FlashingError> {
    let options = rustpiboot::Options {
        delay: 500 * 1000,
        ..Default::default()
    };

    let _ = logging
        .send(FlashProgress {
            status: FlashStatus::Setup,
            message: "Rebooting as a USB mass storage device...".to_string(),
        })
        .await;

    rustpiboot::boot(options).map_err(|err| {
        logging
            .try_send(FlashProgress {
                status: FlashStatus::Error(FlashingError::IoError),
                message: format!("Failed to reboot {:?} as USB MSD: {:?}", VID_PID, err),
            })
            .unwrap();
        FlashingError::UsbError
    })?;

    sleep(Duration::from_secs(3)).await;

    let _ = logging
        .send(FlashProgress {
            status: FlashStatus::Setup,
            message: "Checking for presence of a device file...".to_string(),
        })
        .await;

    let device_path = get_device_path(["RPi-MSD-"]).await?;
    let msd_device = tokio::fs::OpenOptions::new()
        .write(true)
        .read(true)
        .open(&device_path)
        .await
        .map_err(|e| {
            logging
                .try_send(FlashProgress {
                    status: FlashStatus::Error(FlashingError::DeviceNotFound),
                    message: format!("cannot open {:?} : {:?}", device_path, e),
                })
                .unwrap();
            FlashingError::DeviceNotFound
        })?;

    Ok(msd_device)
}

async fn get_device_path<I: IntoIterator<Item = &'static str>>(
    allowed_vendors: I,
) -> Result<PathBuf, FlashingError> {
    let mut contents = tokio::fs::read_dir("/dev/disk/by-id")
        .await
        .map_err(|err| {
            log::error!("Failed to list devices: {}", err);
            FlashingError::IoError
        })?;

    let target_prefixes = allowed_vendors
        .into_iter()
        .map(|vendor| format!("usb-{}_", vendor))
        .collect::<Vec<String>>();

    let mut matching_devices = vec![];

    while let Some(entry) = contents.next_entry().await.map_err(|err| {
        log::warn!("Intermittent IO error while listing devices: {}", err);
        FlashingError::IoError
    })? {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };

        for prefix in &target_prefixes {
            if file_name.starts_with(prefix) {
                matching_devices.push(file_name.clone());
            }
        }
    }

    // Exclude partitions, i.e. turns [ "x-part2", "x-part1", "x", "y-part2", "y-part1", "y" ]
    // into ["x", "y"].
    let unique_root_devices = matching_devices
        .iter()
        .filter(|this| {
            !matching_devices
                .iter()
                .any(|other| this.starts_with(other) && *this != other)
        })
        .collect::<Vec<&String>>();

    let symlink = match unique_root_devices[..] {
        [] => {
            log::error!("No supported devices found");
            return Err(FlashingError::DeviceNotFound);
        }
        [device] => device.clone(),
        _ => {
            log::error!(
                "Several supported devices found: found {}, expected 1",
                unique_root_devices.len()
            );
            return Err(FlashingError::GpioError);
        }
    };

    tokio::fs::canonicalize(format!("/dev/disk/by-id/{}", symlink))
        .await
        .map_err(|err| {
            log::error!("Failed to read link: {}", err);
            FlashingError::IoError
        })
}
