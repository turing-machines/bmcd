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

pub fn string_from_utf16(bytes: &[u8], little_endian: bool) -> String {
    let u16s = bytes.chunks_exact(2).map(|pair| {
        let Ok(owned) = pair.try_into() else {
            unreachable!()
        };

        if little_endian {
            u16::from_le_bytes(owned)
        } else {
            u16::from_be_bytes(owned)
        }
    });

    let mut string = char::decode_utf16(u16s)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect::<String>();

    if bytes.len() % 2 == 1 {
        string.push(char::REPLACEMENT_CHARACTER)
    }

    string
}

pub fn string_from_utf32(bytes: &[u8], little_endian: bool) -> String {
    bytes
        .chunks(4)
        .map(|slice| {
            let Ok(owned) = slice.try_into() else {
                return char::REPLACEMENT_CHARACTER;
            };

            let scalar = if little_endian {
                u32::from_le_bytes(owned)
            } else {
                u32::from_be_bytes(owned)
            };

            char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect()
}
