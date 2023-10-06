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
use crate::api::into_legacy_response::LegacyResponse;
use crate::utils::ReceiverReader;
use actix_web::http::StatusCode;
use bytes::Bytes;
use futures::Future;
use rand::Rng;
use serde::{Serialize, Serializer};
use std::fmt::Debug;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::{Deref, DerefMut},
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::{
    io::AsyncRead,
    sync::mpsc::{channel, error::SendError},
};
use tokio_util::sync::CancellationToken;

const RESET_TIMEOUT: Duration = Duration::from_secs(10);

pub struct StreamingDataService {
    status: Arc<Mutex<StreamingState>>,
}

impl StreamingDataService {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(StreamingState::Ready)),
        }
    }

    /// Start a node flash command and initialize [`StreamingDataService`] for chunked
    /// file transfer. Calling this function twice results in a
    /// `Err(StreamingServiceError::InProgress)`. Unless the first file transfer deemed to
    /// be stale. In this case the [`StreamingDataService`] will be reset and initialize
    /// for a new transfer. A transfer is stale when the `RESET_TIMEOUT` is
    /// reached. Meaning no chunk has been received for longer as
    /// `RESET_TIMEOUT`.
    pub async fn request_transfer(
        &self,
        peer: &str,
        process_name: String,
        size: u64,
    ) -> Result<(impl AsyncRead, CancellationToken), StreamingServiceError> {
        let mut status = self.status.lock().await;
        self.reset_transfer_on_timeout(peer, status.deref_mut())?;

        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let peer = hasher.finish();

        let (sender, receiver) = channel::<Bytes>(128);
        let transfer_context = TransferContext::new(peer, process_name, sender, size);
        let cancel = transfer_context.cancel.child_token();
        let id = transfer_context.id;
        *status = StreamingState::Transferring(transfer_context);
        log::info!("new transfer initiated. id: {}", id);
        Ok((ReceiverReader::new(receiver), cancel))
    }

    /// When a 'start_transfer' call is made while we are still in a transfer
    /// state, assume that the current transfer is stale given the timeout limit
    /// is reached.
    fn reset_transfer_on_timeout(
        &self,
        peer: &str,
        mut status: impl DerefMut<Target = StreamingState>,
    ) -> Result<(), StreamingServiceError> {
        if let StreamingState::Transferring(context) = &*status {
            let duration = context.duration_since_last_chunk(peer)?;
            if duration < RESET_TIMEOUT {
                return Err(StreamingServiceError::InProgress);
            } else {
                log::warn!(
                    "Assuming transfer ({}) will never complete as last request was {}s ago. Resetting flash service",
                    context.id,
                    duration.as_secs()
                );
                *status = StreamingState::Ready;
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
    /// `StreamingState::Transferring`, meaning a state transition already
    /// happened. In this case we omit a state transition to
    /// `FlashSstatus::Error(_)`
    pub async fn execute_worker(
        &self,
        future: impl Future<Output = Result<(), anyhow::Error>> + Send + 'static,
    ) -> Result<(), StreamingServiceError> {
        let StreamingState::Transferring(ctx) = &*self.status.lock().await else {
            return Err(StreamingServiceError::WrongState);
        };

        let id = ctx.id;
        let cancel = ctx.cancel.child_token();
        let size = ctx.size;
        let start_time = Instant::now();
        let status = self.status.clone();

        tokio::spawn(async move {
            let (new_state, was_cancelled) = future.await.map_or_else(
                |error| {
                    if cancel.is_cancelled() {
                        log::error!("flashing stopped: {}. ({})", error, id);
                    }
                    (
                        StreamingState::Error(error.to_string()),
                        cancel.is_cancelled(),
                    )
                },
                |_| {
                    let duration = Instant::now().saturating_duration_since(start_time);
                    log::info!(
                        "flashing successful. took {}m{}s. ({})",
                        duration.as_secs() / 60,
                        duration.as_secs() % 60,
                        id
                    );

                    (StreamingState::Done(duration, size), false)
                },
            );

            // Ignore state changes due to cancellation. This only happens on a state transition
            // from `StreamingState::Transferring` (see `TransferContext::drop()`). The state is
            // already correct, therefore we omit a state transition in this scenario.
            let mut status_unlocked = status.lock().await;
            if let StreamingState::Transferring(_) = &*status_unlocked {
                if !was_cancelled {
                    *status_unlocked = new_state;
                }
            }
        });
        Ok(())
    }

    /// Write a chunk of bytes to the module that is selected for flashing.
    ///
    /// # Return
    ///
    /// This function returns:
    ///
    /// * 'Err(StreamingServiceError::WrongState)' if this function is called when
    /// ['StreamingDataService'] is not in 'Transferring' state.
    /// * 'Err(StreamingServiceError::EmptyPayload)' when data == empty
    /// * 'Err(StreamingServiceError::Error(_)' when there is an internal error
    /// * Ok(()) on success
    pub async fn put_chunk(&self, peer: String, data: Bytes) -> Result<(), StreamingServiceError> {
        let mut status = self.status.lock().await;
        if let StreamingState::Transferring(ref mut context) = *status {
            context.is_equal_peer(&peer)?;

            if data.is_empty() {
                *status = StreamingState::Ready;
                return Err(StreamingServiceError::EmptyPayload);
            }

            if let Err(e) = context.push_bytes(data).await {
                *status = StreamingState::Error(e.to_string());
                return Err(e);
            }

            Ok(())
        } else {
            log::error!("cannot put chunk. state is not transferring",);
            Err(StreamingServiceError::WrongState)
        }
    }

    /// Return a borrow to the current status of the flash service
    /// This object implements [`serde::Serialize`]
    pub async fn status(&self) -> impl Deref<Target = StreamingState> + '_ {
        self.status.lock().await
    }
}

#[derive(Error, Debug)]
pub enum StreamingServiceError {
    #[error("another flashing operation in progress")]
    InProgress,
    #[error("cannot execute command in current state")]
    WrongState,
    #[error("received empty payload")]
    EmptyPayload,
    #[error("unauthorized request from peer {0}")]
    PeersDoNotMatch(String),
    #[error("{0} was aborted")]
    Aborted(String),
    #[error("error processing internal buffers")]
    MpscError(#[from] SendError<Bytes>),
}

impl From<StreamingServiceError> for LegacyResponse {
    fn from(value: StreamingServiceError) -> Self {
        let status_code = match value {
            StreamingServiceError::InProgress => StatusCode::SERVICE_UNAVAILABLE,
            StreamingServiceError::WrongState => StatusCode::BAD_REQUEST,
            StreamingServiceError::MpscError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            StreamingServiceError::Aborted(_) => StatusCode::INTERNAL_SERVER_ERROR,
            StreamingServiceError::EmptyPayload => StatusCode::BAD_REQUEST,
            StreamingServiceError::PeersDoNotMatch(_) => StatusCode::BAD_REQUEST,
        };
        (status_code, value.to_string()).into()
    }
}

#[derive(Serialize)]
pub enum StreamingState {
    Ready,
    Transferring(TransferContext),
    Done(Duration, u64),
    Error(String),
}

/// Context object for node flashing. This object will cancel the node flash
/// cancel-token when it goes out of scope, Aborting the node flash task.
/// Typically happens on a state transition inside the [`StreamingDataService`].
#[derive(Serialize)]
pub struct TransferContext {
    pub id: u64,
    pub peer: u64,
    pub process_name: String,
    pub size: u64,
    #[serde(skip)]
    bytes_sender: Sender<Bytes>,
    #[serde(skip)]
    cancel: CancellationToken,
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
    pub fn new(peer: u64, process_name: String, bytes_sender: Sender<Bytes>, size: u64) -> Self {
        let mut rng = rand::thread_rng();
        let id = rng.gen();

        TransferContext {
            id,
            peer,
            process_name,
            size,
            bytes_sender,
            cancel: CancellationToken::new(),
            last_recieved_chunk: Instant::now(),
        }
    }

    pub fn duration_since_last_chunk(&self, peer: &str) -> Result<Duration, StreamingServiceError> {
        self.is_equal_peer(peer)?;
        Ok(Instant::now().saturating_duration_since(self.last_recieved_chunk))
    }

    pub fn is_equal_peer(&self, peer: &str) -> Result<(), StreamingServiceError> {
        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let hashed_peer = hasher.finish();
        if self.peer != hashed_peer {
            return Err(StreamingServiceError::PeersDoNotMatch(peer.into()));
        }

        Ok(())
    }

    async fn push_bytes(&mut self, data: Bytes) -> Result<(), StreamingServiceError> {
        match self.bytes_sender.send(data).await {
            Ok(_) => {
                self.last_recieved_chunk = Instant::now();
                Ok(())
            }
            Err(_) if self.bytes_sender.is_closed() => {
                Err(StreamingServiceError::Aborted(self.process_name.clone()))
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for TransferContext {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
