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
use crate::app::transfer_context::{TransferContext, TransferSource};
use actix_web::http::StatusCode;
use bytes::Bytes;
use futures::Future;
use humansize::{format_size, DECIMAL};
use serde::Serialize;
use std::fmt::{Debug, Display};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::fs::OpenOptions;
use tokio::io::AsyncSeekExt;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::{io::AsyncRead, sync::mpsc::error::SendError};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;
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

    /// Start a node flash command and initialize [`StreamingDataService`] for
    /// chunked file transfer. Calling this function twice results in a
    /// `Err(StreamingServiceError::InProgress)`. Unless the first file transfer
    /// deemed to be stale. In this case the [`StreamingDataService`] will be
    /// reset and initialize for a new transfer. A transfer is stale when the
    /// `RESET_TIMEOUT` is reached. Meaning no chunk has been received for
    /// longer as `RESET_TIMEOUT`.
    pub async fn request_transfer<M: Into<TransferType>>(
        &self,
        process_name: String,
        transfer_type: M,
    ) -> Result<ReaderContext, StreamingServiceError> {
        let transfer_type = transfer_type.into();
        let mut status = self.status.lock().await;
        self.reset_transfer_on_timeout(status.deref_mut())?;

        let (reader, context) = match transfer_type {
            TransferType::Local(path) => Self::local(process_name, path).await?,
            TransferType::Remote(peer, length) => Self::remote(process_name, peer, length)?,
        };

        log::info!(
            "new transfer initialized: '{}'({}) {}",
            context.process_name,
            context.id,
            format_size(context.size, DECIMAL),
        );

        *status = StreamingState::Transferring(context);
        Ok(reader)
    }

    pub fn remote(
        process_name: String,
        peer: String,
        size: u64,
    ) -> Result<(ReaderContext, TransferContext), StreamingServiceError> {
        let (bytes_sender, bytes_receiver) = mpsc::channel::<Bytes>(256);
        let reader = StreamReader::new(
            ReceiverStream::new(bytes_receiver).map(Ok::<bytes::Bytes, std::io::Error>),
        );
        let source: TransferSource = bytes_sender.into();

        Ok(Self::new_context_pair(
            process_name,
            peer,
            reader,
            source,
            size,
        ))
    }

    pub async fn local(
        process_name: String,
        path: String,
    ) -> Result<(ReaderContext, TransferContext), StreamingServiceError> {
        let mut file = OpenOptions::new().read(true).open(&path).await?;
        let file_size = file.seek(std::io::SeekFrom::End(0)).await?;
        file.seek(std::io::SeekFrom::Start(0)).await?;
        Ok(Self::new_context_pair(
            process_name,
            path,
            file,
            TransferSource::Local,
            file_size,
        ))
    }

    fn new_context_pair<R>(
        process_name: String,
        peer: String,
        reader: R,
        source: TransferSource,
        size: u64,
    ) -> (ReaderContext, TransferContext)
    where
        R: AsyncRead + Unpin + Send + Sync + 'static,
    {
        let (written_sender, written_receiver) = watch::channel(0u64);

        let transfer_ctx = TransferContext::new(peer, process_name, source, size, written_receiver);
        let reader_ctx = ReaderContext {
            reader: Box::new(reader),
            size,
            cancel: transfer_ctx.get_child_token(),
            written_sender,
        };

        (reader_ctx, transfer_ctx)
    }

    /// When a 'start_transfer' call is made while we are still in a transfer
    /// state, assume that the current transfer is stale given the timeout limit
    /// is reached.
    fn reset_transfer_on_timeout(
        &self,
        mut status: impl DerefMut<Target = StreamingState>,
    ) -> Result<(), StreamingServiceError> {
        if let StreamingState::Transferring(context) = &*status {
            let duration = context.duration_since_last_chunk();
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
        let status = self.status.lock().await;
        let StreamingState::Transferring(ctx) = &*status else {
            return Err(StreamingServiceError::WrongState(status.to_string(), "Transferring".to_string()));
        };

        let id = ctx.id;
        let cancel = ctx.get_child_token();
        let size = ctx.size;
        let start_time = Instant::now();
        let status = self.status.clone();

        tokio::spawn(async move {
            log::debug!("starting streaming data service worker");
            let (new_state, was_cancelled) = future.await.map_or_else(
                |error| {
                    log::error!("worker stopped: {}. ({})", error, id);
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
            if let StreamingState::Transferring(ctx) = &*status_unlocked {
                log::debug!("last recorded transfer state {:#?}", ctx);
                if !was_cancelled {
                    log::info!("state={new_state}");
                    *status_unlocked = new_state;
                } else {
                    log::debug!("state change ignored (cancelled=true)");
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
            Err(StreamingServiceError::WrongState(
                status.to_string(),
                "Transferring".to_string(),
            ))
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
    #[error("cannot execute command in current state. current={0}, expected={1}")]
    WrongState(String, String),
    #[error("received empty payload")]
    EmptyPayload,
    #[error("unauthorized request from peer {0}")]
    PeersDoNotMatch(String),
    #[error("{0} was aborted")]
    Aborted(String),
    #[error("error processing internal buffers")]
    MpscError(#[from] SendError<Bytes>),
    #[error("Received more bytes as negotiated")]
    LengthExceeded,
    #[error("IO error")]
    IoError(#[from] std::io::Error),
    #[error("not a remote transfer")]
    IsLocalTransfer,
}

impl From<StreamingServiceError> for LegacyResponse {
    fn from(value: StreamingServiceError) -> Self {
        let status_code = match value {
            StreamingServiceError::InProgress => StatusCode::SERVICE_UNAVAILABLE,
            StreamingServiceError::WrongState(_, _) => StatusCode::BAD_REQUEST,
            StreamingServiceError::MpscError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            StreamingServiceError::Aborted(_) => StatusCode::INTERNAL_SERVER_ERROR,
            StreamingServiceError::EmptyPayload => StatusCode::BAD_REQUEST,
            StreamingServiceError::PeersDoNotMatch(_) => StatusCode::BAD_REQUEST,
            StreamingServiceError::LengthExceeded => StatusCode::BAD_REQUEST,
            StreamingServiceError::IoError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            StreamingServiceError::IsLocalTransfer => StatusCode::BAD_REQUEST,
        };
        (status_code, value.to_string()).into()
    }
}

#[derive(Serialize, Debug)]
pub enum StreamingState {
    Ready,
    Transferring(TransferContext),
    Done(Duration, u64),
    Error(String),
}

impl Display for StreamingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamingState::Ready => f.write_str("Ready"),
            StreamingState::Transferring(_) => f.write_str("Transferring"),
            StreamingState::Done(_, _) => f.write_str("Done"),
            StreamingState::Error(_) => f.write_str("Error"),
        }
    }
}

pub struct ReaderContext {
    pub reader: Box<dyn AsyncRead + Send + Sync + Unpin>,
    pub size: u64,
    pub cancel: CancellationToken,
    pub written_sender: watch::Sender<u64>,
}

pub enum TransferType {
    Local(String),
    Remote(String, u64),
}
