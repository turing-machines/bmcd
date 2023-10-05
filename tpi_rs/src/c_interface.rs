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
//! This module acts as a glue layer for the legacy bmc application. It exports
//! relevant API functions over FFI. This FFI interface is temporary and will be
//! removed as soon as the bmc application is end of life.
use futures::future::BoxFuture;
use log::{error, LevelFilter};
use once_cell::sync::{Lazy, OnceCell};
use simple_logger::SimpleLogger;
use std::ffi::{c_char, c_int, CStr};
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::join;
use tokio::sync::mpsc::channel;
use tokio::sync::mpsc::Receiver;
use tokio::{runtime::Runtime, sync::Mutex};
use tokio_util::sync::CancellationToken;

use crate::app::bmc_application::{BmcApplication, UsbConfig};
use crate::app::event_application::run_event_listener;
use crate::app::flash_context::FlashContext;
use crate::middleware::firmware_update::FlashingError;
use crate::middleware::{UsbMode, UsbRoute};

/// we need means to synchronize async call to the outside. This runtime
/// enables us to execute async calls in a blocking fashion.
static RUNTIME: Lazy<Runtime> =
    Lazy::new(|| Runtime::new().expect("fatal error initializing runtime"));
static APP: OnceCell<Mutex<Arc<BmcApplication>>> = OnceCell::new();

#[no_mangle]
pub extern "C" fn tpi_initialize() {
    RUNTIME.block_on(async move {
        let level = if cfg!(debug_assertions) {
            LevelFilter::Trace
        } else {
            LevelFilter::Info
        };

        SimpleLogger::new()
            .with_level(level)
            .with_module_level("sqlx", LevelFilter::Warn)
            .with_colors(true)
            .env()
            .init()
            .expect("failed to initialize logger");

        log::info!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

        let bmc = Arc::new(
            BmcApplication::new()
                .await
                .expect("unable to initialize bmc app"),
        );
        run_event_listener(bmc.clone()).expect("event listener");
        APP.set(Mutex::new(bmc))
            .expect("initialize to be called once");
    });
}

fn execute_routine<F>(func: F)
where
    F: for<'a> Fn(&'a BmcApplication) -> BoxFuture<'a, anyhow::Result<()>>,
{
    RUNTIME.block_on(async move {
        let lock = APP.get().unwrap().lock();
        if let Err(e) = func(&*lock.await).await {
            error!("{}", e);
        }
    });
}

#[no_mangle]
pub extern "C" fn tpi_node_power(num: c_int, status: c_int) {
    num.try_into().map_or_else(
        |e| log::error!("{}", e),
        |x: u8| {
            // if status == 0, node == off.
            // if status == 1, node == on.
            let mut node = 1 << x;
            if status == 0 {
                node = !node;
            }
            execute_routine(|bmc| Box::pin(bmc.activate_slot(node, 1 << x)));
        },
    );
}

#[no_mangle]
pub extern "C" fn tpi_usb_mode_v2(mode: c_int, node: c_int, boot_pin: c_int) -> c_int {
    let Ok(node_id) = node.try_into().map_err(|e| log::error!("{}", e)) else {
        return -1;
    };
    let mode = UsbMode::from_api_mode(mode);

    let boot = boot_pin.try_into().map_or_else(
        |e| {
            log::error!("{}", e);
            false
        },
        |b: u32| b != 0,
    );

    let config = match mode {
        UsbMode::Device => UsbConfig::UsbA(node_id),
        UsbMode::Host => UsbConfig::Node(node_id, UsbRoute::UsbA),
    };
    execute_routine(|bmc| {
        Box::pin(async move {
            bmc.usb_boot(node_id, boot).await?;
            bmc.configure_usb(config).await?;
            Ok(())
        })
    });
    0
}

#[no_mangle]
pub extern "C" fn tpi_usb_mode(mode: c_int, node: c_int) -> c_int {
    // mimic legacy behavior where nodes put in device mode always have the boot
    // pin set
    tpi_usb_mode_v2(mode, node, 1)
}

#[no_mangle]
pub extern "C" fn tpi_get_node_power(node: c_int) -> c_int {
    let Ok(node_id) = node.try_into().map_err(|e| log::error!("{}", e)) else {
        return -1;
    };

    RUNTIME.block_on(async move {
        let lock = APP.get().unwrap().lock().await;
        let status = lock.get_node_power(node_id).await;

        match status {
            Ok(st) => c_int::from(st),
            Err(_) => -1,
        }
    })
}

#[no_mangle]
pub extern "C" fn tpi_reset_node(node: c_int) {
    let Ok(node_id) = node.try_into().map_err(|e| log::error!("{}", e)) else {
        return ;
    };
    execute_routine(|bmc| Box::pin(bmc.reset_node(node_id)));
}

#[no_mangle]
pub extern "C" fn tpi_rtl_reset() {
    execute_routine(|bmc| Box::pin(bmc.rtl_reset()));
}

#[repr(C)]
pub enum FlashingResult {
    Success,
    InvalidArgs,
    DeviceNotFound,
    GpioError,
    UsbError,
    IoError,
    ChecksumMismatch,
    Other,
}

impl From<&FlashingError> for FlashingResult {
    fn from(value: &FlashingError) -> Self {
        match value {
            FlashingError::InvalidArgs => FlashingResult::InvalidArgs,
            FlashingError::DeviceNotFound => FlashingResult::DeviceNotFound,
            FlashingError::GpioError => FlashingResult::GpioError,
            FlashingError::UsbError => FlashingResult::UsbError,
            FlashingError::IoError => FlashingResult::IoError,
            FlashingError::ChecksumMismatch => FlashingResult::ChecksumMismatch,
        }
    }
}

#[no_mangle]
pub extern "C" fn tpi_clear_usbboot() {
    RUNTIME.block_on(async move {
        APP.get().unwrap().lock().await.clear_usb_boot().unwrap();
    });
}

#[no_mangle]
pub extern "C" fn tpi_node_to_msd(node: c_int) {
    RUNTIME.block_on(async move {
        let bmc = APP.get().unwrap().lock().await.clone();
        let (sender, receiver) = channel(64);
        let handle = tokio::spawn(async move {
            bmc.set_node_in_msd(
                node.try_into().unwrap(),
                crate::middleware::UsbRoute::Bmc,
                sender,
            )
            .await
        });
        let print_handle = logging_sink(receiver);
        let _ = join!(handle, print_handle);
    });
}

/// # Safety
///
/// `image_path` needs to contain a valid utf-8 string. Secondly note that inside this function the
/// `image_path` is used by reference, therefore never pass it in its current form to another
/// context.
#[no_mangle]
pub unsafe extern "C" fn tpi_flash_node(node: c_int, image_path: *const c_char) -> FlashingResult {
    let cstr = CStr::from_ptr(image_path);
    let Ok(bstr) = cstr.to_str() else {
        return FlashingResult::InvalidArgs;
    };
    let node_image = PathBuf::from(bstr);

    let Ok(node_id) = node.try_into() else {
        return FlashingResult::InvalidArgs
    };

    RUNTIME.block_on(async move {
        let bmc = APP.get().unwrap().lock().await;
        let (sender, receiver) = channel(64);
        let img_file = tokio::fs::File::open(&node_image).await.unwrap();
        let img_len = img_file.metadata().await.unwrap().len();
        let mut context = FlashContext {
            id: 123,
            filename: node_image
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
            size: img_len,
            node: node_id,
            byte_stream: img_file,
            bmc: bmc.clone(),
            progress_sender: sender,
            cancel: CancellationToken::new(),
        };

        let handle = tokio::spawn(async move { context.flash_node().await });

        let print_handle = logging_sink(receiver);
        let (res, _) = join!(handle, print_handle);

        match res.unwrap() {
            Ok(_) => FlashingResult::Success,
            Err(cause) => {
                if let Some(flashing_err) = cause.downcast_ref::<FlashingError>() {
                    flashing_err.into()
                } else {
                    FlashingResult::Other
                }
            }
        }
    })
}

// for now we print the status updates to console. In the future we would like to pass
// this back to the clients.
fn logging_sink<T: Display + Send + 'static>(
    mut receiver: Receiver<T>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(msg) = receiver.recv().await {
            log::info!("{}", msg);
        }
    })
}
