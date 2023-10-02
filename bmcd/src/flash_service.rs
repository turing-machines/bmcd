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
use crate::into_legacy_response::LegacyResponse;
use actix_web::{http::StatusCode, web::Bytes};
use rand::Rng;
use serde::{Serialize, Serializer};
use std::{
    collections::hash_map::DefaultHasher,
    error::Error,
    fmt::Display,
    hash::{Hash, Hasher},
    ops::{Deref, DerefMut},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio::{
    io::AsyncRead,
    sync::mpsc::{channel, error::SendError, Sender},
};
use tokio_util::sync::CancellationToken;
use tpi_rs::{app::bmc_application::BmcApplication, middleware::NodeId};
use tpi_rs::{app::flash_context::FlashContext, utils::ReceiverReader};
const RESET_TIMEOUT: Duration = Duration::from_secs(10);

pub struct FlashService {
    status: Arc<Mutex<FlashStatus>>,
}

impl FlashService {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(FlashStatus::Ready)),
        }
    }

    /// Start a node flash command and initialize [`FlashService`] for chunked
    /// file transfer. Calling this function twice results in a
    /// `Err(FlashError::InProgress)`. Unless the first file transfer deemed to
    /// be stale. In this case the [`FlashService`] will be reset and initialize
    /// for a new transfer. A transfer is stale when the `RESET_TIMEOUT` is
    /// reached. Meaning no chunk has been received for longer as
    /// `RESET_TIMEOUT`.
    pub async fn start_transfer(
        &self,
        peer: &str,
        filename: String,
        size: u64,
        node: NodeId,
        bmc: Arc<BmcApplication>,
    ) -> Result<(), FlashError> {
        let mut status = self.status.lock().await;
        self.reset_transfer_on_timeout(peer, status.deref_mut())?;

        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let peer = hasher.finish();

        let (sender, receiver) = channel::<Bytes>(128);
        let transfer_context = TransferContext::new(peer, sender);
        let cancel = transfer_context.cancel.child_token();
        let id = transfer_context.id;

        let context = FlashContext::new(
            id,
            filename,
            size,
            node,
            ReceiverReader::new(receiver),
            bmc,
            cancel,
        );

        Self::run_flash_worker(context, self.status.clone());
        *status = FlashStatus::Transferring(transfer_context);
        log::info!("new transfer started. id: {}", id);

        Ok(())
    }

    /// When a 'start_transfer' call is made while we are still in a transfer
    /// state, assume that the current transfer is stale given the timeout limit
    /// is reached.
    fn reset_transfer_on_timeout(
        &self,
        peer: &str,
        mut status: impl DerefMut<Target = FlashStatus>,
    ) -> Result<(), FlashError> {
        if let FlashStatus::Transferring(context) = &*status {
            let duration = context.duration_since_last_chunk(peer)?;
            if duration < RESET_TIMEOUT {
                return Err(FlashError::InProgress);
            } else {
                log::warn!(
                    "Assuming transfer ({}) will never complete as last request was {}s ago. Resetting flash service",
                    context.id,
                    duration.as_secs()
                );
                *status = FlashStatus::Ready;
            }
        }
        Ok(())
    }

    /// Worker task that performs the actual node flash. This tasks finishes if
    /// one of the following scenario's is met:
    /// * flashing completed successfully
    /// * flashing was canceled
    /// * Error occurred during flashing.
    ///
    /// Note that the "global" status does not get updated when the task was
    /// canceled. Cancel can only be true on a state transition from
    /// `FlashStatus::Transferring`, meaning a state transition already
    /// happened. In this case we omit a state transition to
    /// `FlashSstatus::Error(_)`
    fn run_flash_worker<R: AsyncRead + Unpin + Send + Sync + 'static>(
        mut context: FlashContext<R>,
        status: Arc<Mutex<FlashStatus>>,
    ) {
        let start_time = Instant::now();
        tokio::spawn(async move {
            let (new_state, was_cancelled) = context.flash_node().await.map_or_else(
                |e| {
                    let error = e.to_string();
                    let is_cancelled = context.cancel.is_cancelled();
                    if is_cancelled {
                        log::error!("flashing stopped: {}. ({})", error, context.id);
                    }
                    (FlashStatus::Error(e.to_string()), is_cancelled)
                },
                |_| {
                    let duration = Instant::now().saturating_duration_since(start_time);
                    log::info!(
                        "flashing successful. took {}m{}s. ({})",
                        duration.as_secs() / 60,
                        duration.as_secs() % 60,
                        context.id,
                    );

                    (FlashStatus::Done(duration, context.size), false)
                },
            );

            let mut status_unlocked = status.lock().await;
            if let FlashStatus::Transferring(_) = &*status_unlocked {
                if !was_cancelled {
                    *status_unlocked = new_state;
                }
            }
        });
    }

    /// Write a chunk of bytes to the module that is selected for flashing.
    ///
    /// # Return
    ///
    /// This function returns:
    ///
    /// * 'Err(FlashError::WrongState)' if this function is called when
    /// ['FlashService'] is not in 'Transferring' state.
    /// * 'Err(FlashError::EmptyPayload)' when data == empty
    /// * 'Err(FlashError::Error(_)' when there is an internal error
    /// * Ok(()) on success
    pub async fn put_chunk(&self, peer: String, data: Bytes) -> Result<(), FlashError> {
        let mut status = self.status.lock().await;
        if let FlashStatus::Transferring(ref mut context) = *status {
            if data.is_empty() {
                *status = FlashStatus::Ready;
                return Err(FlashError::EmptyPayload);
            }

            if let Err(e) = context.push_bytes(peer, data).await {
                *status = FlashStatus::Error(e.to_string());
                return Err(e);
            }

            Ok(())
        } else {
            log::error!(
                "cannot put chunk. state is not transferring. state= {:?}",
                status
            );
            Err(FlashError::WrongState)
        }
    }

    /// Return a borrow to the current status of the flash service
    /// This object implements [`serde::Serialize`]
    pub async fn status(&self) -> impl Deref<Target = FlashStatus> + '_ {
        self.status.lock().await
    }
}

#[derive(Debug)]
pub enum FlashError {
    InProgress,
    WrongState,
    EmptyPayload,
    PeersDoNotMatch(String),
    Aborted,
    MpscError(SendError<Bytes>),
}

impl Error for FlashError {}

impl Display for FlashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlashError::InProgress => write!(f, "another flashing operation in progress"),
            FlashError::WrongState => {
                write!(f, "cannot execute command in current state")
            }
            FlashError::Aborted => write!(f, "flash operation was aborted"),
            FlashError::MpscError(e) => write!(f, "internal error sending buffers: {}", e),
            FlashError::EmptyPayload => write!(f, "received emply payload"),
            FlashError::PeersDoNotMatch(peer) => {
                write!(f, "no flash service in progress for {}", peer)
            }
        }
    }
}

impl From<SendError<Bytes>> for FlashError {
    fn from(value: SendError<Bytes>) -> Self {
        FlashError::MpscError(value)
    }
}

impl From<FlashError> for LegacyResponse {
    fn from(value: FlashError) -> Self {
        let status_code = match value {
            FlashError::InProgress => StatusCode::SERVICE_UNAVAILABLE,
            FlashError::WrongState => StatusCode::BAD_REQUEST,
            FlashError::MpscError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            FlashError::Aborted => StatusCode::INTERNAL_SERVER_ERROR,
            FlashError::EmptyPayload => StatusCode::BAD_REQUEST,
            FlashError::PeersDoNotMatch(_) => StatusCode::BAD_REQUEST,
        };
        (status_code, value.to_string()).into()
    }
}

#[derive(Debug, Serialize)]
pub enum FlashStatus {
    Ready,
    Transferring(TransferContext),
    Done(Duration, u64),
    Error(String),
}

/// Context object for node flashing. This object will cancel the node flash
/// cancel-token when it goes out of scope, Aborting the node flash task.
/// Typically happens on a state transition inside the [`FlashService`].
#[derive(Debug, Serialize)]
pub struct TransferContext {
    pub id: u64,
    pub peer: u64,
    #[serde(skip)]
    pub bytes_sender: Sender<Bytes>,
    #[serde(skip)]
    pub cancel: CancellationToken,
    #[serde(serialize_with = "serialize_seconds_until_now")]
    last_recieved_chunk: Instant,
}

fn serialize_seconds_until_now<S>(instant: &Instant, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let secs = Instant::now().saturating_duration_since(*instant).as_secs();
    s.serialize_u64(secs)
}

impl TransferContext {
    pub fn new(peer: u64, bytes_sender: Sender<Bytes>) -> Self {
        let mut rng = rand::thread_rng();
        let id = rng.gen();

        TransferContext {
            id,
            peer,
            bytes_sender,
            cancel: CancellationToken::new(),
            last_recieved_chunk: Instant::now(),
        }
    }

    pub fn duration_since_last_chunk(&self, peer: &str) -> Result<Duration, FlashError> {
        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let hashed_peer = hasher.finish();
        if self.peer != hashed_peer {
            return Err(FlashError::PeersDoNotMatch(peer.into()));
        }

        Ok(Instant::now().saturating_duration_since(self.last_recieved_chunk))
    }

    async fn push_bytes(&mut self, peer: String, data: Bytes) -> Result<(), FlashError> {
        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let hashed_peer = hasher.finish();
        if self.peer != hashed_peer {
            return Err(FlashError::PeersDoNotMatch(peer));
        }

        match self.bytes_sender.send(data).await {
            Ok(_) => {
                self.last_recieved_chunk = Instant::now();
                Ok(())
            }
            Err(_) if self.bytes_sender.is_closed() => Err(FlashError::Aborted),
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for TransferContext {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
