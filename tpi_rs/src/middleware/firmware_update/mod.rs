mod rockusb_fwudate;
use self::rockusb_fwudate::new_rockusb_transport;
mod rpi_fwupdate;
use rpi_fwupdate::new_rpi_transport;
pub mod transport;
use self::transport::FwUpdateTransport;
use futures::future::BoxFuture;
use once_cell::sync::Lazy;
use rusb::GlobalContext;
use std::{
    collections::HashMap,
    fmt::{self, Display},
};
use tokio::sync::mpsc::Sender;

#[allow(clippy::vec_init_then_push)]
pub static SUPPORTED_MSD_DEVICES: Lazy<Vec<(u16, u16)>> = Lazy::new(|| {
    let mut supported = Vec::<(u16, u16)>::new();
    supported.push(rpi_fwupdate::VID_PID);
    supported
});

pub static SUPPORTED_DEVICES: Lazy<HashMap<(u16, u16), FactoryItemCreator>> = Lazy::new(|| {
    let mut creators = HashMap::<(u16, u16), FactoryItemCreator>::new();
    creators.insert(
        rpi_fwupdate::VID_PID,
        Box::new(|_, logging| {
            Box::pin(async move { new_rpi_transport(&logging).await.map(Into::into) })
        }),
    );

    creators.insert(
        rockusb_fwudate::RK3588_VID_PID,
        Box::new(|device, logging| {
            let clone = device.clone();
            Box::pin(async move { new_rockusb_transport(clone, &logging).await.map(Into::into) })
        }),
    );

    creators
});

pub type FactoryItem = BoxFuture<'static, Result<Box<dyn FwUpdateTransport>, FlashingError>>;
pub type FactoryItemCreator =
    Box<dyn Fn(&rusb::Device<GlobalContext>, Sender<FlashProgress>) -> FactoryItem + Send + Sync>;

pub fn fw_update_transport(
    device: &rusb::Device<GlobalContext>,
    logging: Sender<FlashProgress>,
) -> anyhow::Result<FactoryItem> {
    let descriptor = device.device_descriptor()?;
    let vid_pid = (descriptor.vendor_id(), descriptor.product_id());

    SUPPORTED_DEVICES
        .get(&vid_pid)
        .map(|creator| creator(device, logging))
        .ok_or(anyhow::anyhow!("no driver available for {:?}", device))
}

#[derive(Debug, Clone, Copy)]
pub enum FlashingError {
    InvalidArgs,
    DeviceNotFound,
    GpioError,
    UsbError,
    IoError,
    ChecksumMismatch,
}

impl fmt::Display for FlashingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlashingError::InvalidArgs => {
                write!(f, "A specified node does not exist or image is not valid")
            }
            FlashingError::DeviceNotFound => write!(f, "Device not found"),
            FlashingError::GpioError => write!(f, "Error toggling GPIO lines"),
            FlashingError::UsbError => write!(f, "Error enumerating USB devices"),
            FlashingError::IoError => write!(f, "File IO error"),
            FlashingError::ChecksumMismatch => {
                write!(f, "Failed to verify image after writing to the node")
            }
        }
    }
}

impl std::error::Error for FlashingError {}

pub trait FlashingErrorExt<T, E: Display> {
    fn map_err_into_logged_usb(self, logging: &Sender<FlashProgress>) -> Result<T, FlashingError>;
    fn map_err_into_logged_io(self, logging: &Sender<FlashProgress>) -> Result<T, FlashingError>;
}

impl<T, E: Display> FlashingErrorExt<T, E> for Result<T, E> {
    fn map_err_into_logged_usb(self, logging: &Sender<FlashProgress>) -> Result<T, FlashingError> {
        self.map_err(|e| {
            logging
                .try_send(FlashProgress {
                    status: FlashStatus::Error(FlashingError::UsbError),
                    message: format!("{}", e),
                })
                .expect("logging channel to be open");
            FlashingError::UsbError
        })
    }
    fn map_err_into_logged_io(self, logging: &Sender<FlashProgress>) -> Result<T, FlashingError> {
        self.map_err(|e| {
            logging
                .try_send(FlashProgress {
                    status: FlashStatus::Error(FlashingError::IoError),
                    message: format!("{}", e),
                })
                .expect("logging channel to be open");
            FlashingError::IoError
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FlashStatus {
    Idle,
    Setup,
    Progress {
        read_percent: usize,
        est_minutes: u64,
        est_seconds: u64,
    },
    Error(FlashingError),
    Done,
}

#[derive(Debug, Clone)]
pub struct FlashProgress {
    pub status: FlashStatus,
    pub message: String,
}

impl Display for FlashProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
