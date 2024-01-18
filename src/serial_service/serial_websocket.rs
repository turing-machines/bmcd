// Copyright 2024 Turing Machines
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

use actix::prelude::*;
use actix_ws::{CloseCode, CloseReason, Message, ProtocolError};
use bytes::Bytes;
use futures::{Sink, SinkExt};
use std::io;
use std::time::{Duration, Instant};
use tokio::pin;
use tokio::time::interval;
use tokio_stream::StreamExt;

/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// This function is responsible for handling communication with a given
/// client over a websocket. All data is send over the `bytes` type in the
/// socket transport. A watchdog is running which monitors the heartbeat of the
/// client. When no 'ping' response is seen from the client for more as
/// [`CLIENT_TIMEOUT`] the server will close down the websocket.
pub async fn run_websocket(
    mut session: actix_ws::Session,
    mut msg_stream: actix_ws::MessageStream,
    serial_stream: impl Stream<Item = io::Result<Bytes>>,
    serial_sink: impl Sink<bytes::Bytes, Error = io::Error>,
) {
    let mut last_heartbeat = Instant::now();
    let mut interval = interval(HEARTBEAT_INTERVAL);

    pin!(serial_stream);
    pin!(serial_sink);

    let close_reason = loop {
        let tick = interval.tick();
        pin!(tick);

        // Await one of the three tasks.
        // * msg_stream awaits any data coming from the compute module
        // * serial_stream awaits any data from the websocket
        // * tick awaits the tick interval. a heartbeat to let the websocket
        // client know we are still alive.
        let task = tokio::select! {
            cmd = msg_stream.next() => cmd.map_or_else(||TaskResult::Close, TaskResult::Command),
            data = serial_stream.next() => data.map_or_else(||TaskResult::Close, TaskResult::Data),
            _ = tick => TaskResult::Tick,
        };

        if let Err(e) = handle_task(task, &mut session, &mut last_heartbeat, &mut serial_sink).await
        {
            break e;
        }
    };

    if let Err(e) = serial_sink.flush().await {
        log::error!("error flushing serial sink {}", e);
    }

    if session.close(Some(close_reason)).await.is_err() {
        log::warn!("could not close websocket session gracefully");
    }
}

async fn handle_task(
    result: TaskResult,
    session: &mut actix_ws::Session,
    last_heartbeat: &mut Instant,
    serial_sink: &mut (impl Sink<bytes::Bytes, Error = io::Error> + Unpin),
) -> Result<(), CloseReason> {
    match result {
        TaskResult::Command(msg) => {
            message_stream_handler(last_heartbeat, session, msg, serial_sink).await
        }
        TaskResult::Tick => verify_heartbeat(*last_heartbeat, session).await,
        TaskResult::Data(cmd) => handle_command(cmd, session).await,
        TaskResult::Close => Err(CloseReason {
            code: CloseCode::Normal,
            description: Some("websocket channel closed".to_string()),
        }),
    }
}

enum TaskResult {
    Command(Result<Message, ProtocolError>),
    Tick,
    Data(io::Result<Bytes>),
    Close,
}

/// Lines that are being send to the serial console are being passed as binary
/// types over the web socket. No intermediate trans-coding of the actual data
/// happens here.
async fn handle_command(
    bytes: io::Result<Bytes>,
    session: &mut actix_ws::Session,
) -> Result<(), CloseReason> {
    let bytes = bytes.map_err(map_io_error)?;
    session.binary(bytes).await.map_err(map_normal_close)
}

/// If no heartbeat ping/pong received recently, close the connection
async fn verify_heartbeat(
    last_heartbeat: Instant,
    session: &mut actix_ws::Session,
) -> Result<(), CloseReason> {
    if Instant::now().duration_since(last_heartbeat) > CLIENT_TIMEOUT {
        let message =
            format!("client has not sent heartbeat in over {CLIENT_TIMEOUT:?}; disconnecting");
        return Err(CloseReason {
            code: CloseCode::Error,
            description: Some(message),
        });
    }

    if let Err(e) = session.ping(b"").await {
        log::warn!("client ping unsuccessful: {}", e);
    }
    Ok(())
}

async fn message_stream_handler(
    last_heartbeat: &mut Instant,
    session: &mut actix_ws::Session,
    message: Result<Message, ProtocolError>,
    serial_sink: &mut (impl Sink<bytes::Bytes, Error = io::Error> + Unpin),
) -> Result<(), CloseReason> {
    let msg = message.map_err(|e| CloseReason {
        code: CloseCode::Protocol,
        description: Some(e.to_string()),
    })?;

    match msg {
        actix_ws::Message::Text(text) => serial_sink
            .send(Bytes::copy_from_slice(text.as_bytes()))
            .await
            .map_err(map_io_error),
        actix_ws::Message::Binary(bytes) => serial_sink.send(bytes).await.map_err(map_io_error),
        actix_ws::Message::Continuation(_) => Err(CloseReason {
            code: CloseCode::Unsupported,
            description: Some("Continuation frames are not supported".to_string()),
        }),
        actix_ws::Message::Ping(bytes) => {
            *last_heartbeat = Instant::now();
            session.pong(&bytes).await.map_err(map_normal_close)
        }
        actix_ws::Message::Pong(_) => {
            *last_heartbeat = Instant::now();
            Ok(())
        }
        actix_ws::Message::Close(reason) => {
            Err(reason.unwrap_or(map_normal_close(actix_ws::Closed)))
        }
        actix_ws::Message::Nop => Ok(()),
    }
}

fn map_normal_close(close: actix_ws::Closed) -> CloseReason {
    CloseReason {
        code: CloseCode::Normal,
        description: Some(close.to_string()),
    }
}

fn map_io_error(e: io::Error) -> CloseReason {
    CloseReason {
        code: CloseCode::Error,
        description: Some(e.to_string()),
    }
}
