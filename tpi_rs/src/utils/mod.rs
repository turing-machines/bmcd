mod event_listener;
pub mod ring_buf;

use std::fmt::Display;

#[doc(inline)]
pub use event_listener::*;
mod io;
pub use io::*;
use tokio::sync::mpsc::Receiver;

// for now we print the status updates to console. In the future we would like to pass
// this back to the clients.
pub fn logging_sink<T: Display + Send + 'static>(
    mut receiver: Receiver<T>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(msg) = receiver.recv().await {
            log::info!("{}", msg);
        }
    })
}
