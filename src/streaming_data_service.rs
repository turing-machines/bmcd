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
pub mod data_transfer;
pub mod transfer_context;

use crate::api::into_legacy_response::LegacyResponse;
use crate::streaming_data_service::transfer_context::TransferContext;
use actix_web::http::StatusCode;
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::Future;
use humansize::{format_size, DECIMAL};
use rand::Rng;
use serde::Serialize;
use std::fmt::{Debug, Display};
use std::{
    ops::Deref,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

pub struct StreamingDataService {
    status: Arc<Mutex<StreamingState>>,
}

impl StreamingDataService {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(StreamingState::Ready)),
        }
    }

    ///  Initialize [`StreamingDataService`] for chunked file transfer. Calling
    ///  this function cancels any ongoing transfers and resets the internal
    ///  state of the service to `StreamingState::Ready` before going to
    ///  `StreamingState::Transferring` again.
    pub async fn request_transfer(
        &self,
        request: TransferRequest,
    ) -> Result<u32, StreamingServiceError> {
        let mut rng = rand::thread_rng();
        let id = rng.gen();

        let context = TransferContext::new(
            id,
            request.process_name,
            request.size,
            request.progress_watcher,
            request.sender,
            request.cancel,
        );

        tracing::info!(
            "#{} '{}' {} - started",
            context.id,
            context.process_name,
            format_size(context.size, DECIMAL),
        );

        self.execute_worker(&context, request.worker).await;
        Self::cancel_request_on_timeout(self.status.clone());
        *self.status.lock().await = StreamingState::Transferring(context);

        Ok(id)
    }

    pub async fn cancel_all(&self) {
        *self.status.lock().await = StreamingState::Error("cancelled by user".to_string());
    }

    fn cancel_request_on_timeout(status: Arc<Mutex<StreamingState>>) {
        tokio::spawn(async move {
            sleep(Duration::from_secs(10)).await;
            let mut status_unlocked = status.lock().await;

            if let StreamingState::Transferring(ctx) = status_unlocked.deref() {
                if ctx.data_sender.is_some() {
                    tracing::warn!("#{} got cancelled due to timeout", ctx.id);
                    *status_unlocked = StreamingState::Error("Send timeout".to_string());
                }
            }
        });
    }

    /// Worker task that performs the actual node flash. This tasks finishes if
    /// one of the following scenario's is met:
    /// * transfer & flashing completed successfully
    /// * transfer & flashing was canceled
    /// * Error occurred during transfer or flashing.
    ///
    /// Note that the "global" status (`StreamingState`) does not get updated to
    /// `StreamingState::Error(_)` when the worker was canceled as the cancel
    /// was an effect of a prior state change. In this case we omit the state
    /// transition to `FlashSstatus::Error(_)`
    ///
    async fn execute_worker(
        &self,
        context: &TransferContext,
        future: impl Future<Output = Result<(), anyhow::Error>> + Send + 'static,
    ) {
        let id = context.id;
        let cancel = context.get_child_token();
        let size = context.size;
        let start_time = Instant::now();
        let status = self.status.clone();

        tokio::spawn(async move {
            tracing::debug!("starting streaming data service worker");
            let (new_state, was_cancelled) = future.await.map_or_else(
                |error| {
                    tracing::error!("#{} stopped: {:#}.", id, error);
                    (
                        StreamingState::Error(error.to_string()),
                        cancel.is_cancelled(),
                    )
                },
                |_| {
                    let duration = Instant::now().saturating_duration_since(start_time);
                    tracing::info!(
                        "worker done. took {} (#{})",
                        humantime::format_duration(duration),
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
                tracing::debug!(
                    "last recorded transfer state: {:#?}",
                    serde_json::to_string(ctx)
                );
                tracing::debug!("state={new_state}(cancelled={})", was_cancelled);

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
    /// * 'Err(StreamingServiceError::WrongState)' if this function is called when
    ///     ['StreamingDataService'] is not in 'Transferring' state.
    /// * 'Err(StreamingServiceError::HandlesDoNotMatch)', the passed id is
    ///     unknown
    /// * 'Err(StreamingServiceError::SenderTaken(_)'
    /// * Ok(()) on success
    pub async fn take_sender(
        &self,
        id: u32,
    ) -> Result<(mpsc::Sender<Bytes>, u64), StreamingServiceError> {
        let mut status = self.status.lock().await;
        let StreamingState::Transferring(ref mut context) = *status else {
            return Err(StreamingServiceError::WrongState(
                status.to_string(),
                "Transferring".to_string(),
            ));
        };

        if id != context.id {
            return Err(StreamingServiceError::HandlesDoNotMatch);
        }

        let sender = context
            .data_sender
            .take()
            .ok_or(StreamingServiceError::SenderTaken)?;

        Ok((sender, context.size))
    }

    /// Return a borrow to the current status of the flash service
    /// This object implements [`serde::Serialize`]
    pub async fn status(&self) -> impl Deref<Target = StreamingState> + '_ {
        self.status.lock().await
    }

    pub async fn try_get_error(&self, timeout: Duration) -> Option<String> {
        let clone = self.status.clone();
        tokio::time::timeout(timeout, async move {
            loop {
                sleep(Duration::from_millis(100)).await;
                let lock = clone.lock().await;
                let message = lock.error_message();
                if let Some(msg) = message {
                    return msg.to_string();
                }
            }
        })
        .await
        .ok()
    }
}

#[derive(Error, Debug)]
pub enum StreamingServiceError {
    #[error("cannot execute command in current state. current={0}, expected={1}")]
    WrongState(String, String),
    #[error("unauthorized request for handle")]
    HandlesDoNotMatch,
    #[error("IO error")]
    IoError(#[from] std::io::Error),
    #[error(
        "cannot transfer bytes to worker. This is either because the transfer \
        happens locally, or is already ongoing."
    )]
    SenderTaken,
}

impl From<StreamingServiceError> for LegacyResponse {
    fn from(value: StreamingServiceError) -> Self {
        let status_code = match value {
            StreamingServiceError::WrongState(_, _) => StatusCode::BAD_REQUEST,
            StreamingServiceError::HandlesDoNotMatch => StatusCode::BAD_REQUEST,
            StreamingServiceError::IoError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            StreamingServiceError::SenderTaken => StatusCode::BAD_REQUEST,
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

impl StreamingState {
    /// returns the error message when self == `StreamingState::Error(msg)`.
    /// Otherwise returns `None`.
    pub fn error_message(&self) -> Option<&str> {
        if let StreamingState::Error(msg) = self {
            return Some(msg);
        }
        None
    }
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

pub struct TransferRequest {
    pub process_name: String,
    pub size: u64,
    pub sender: Option<mpsc::Sender<bytes::Bytes>>,
    pub progress_watcher: watch::Receiver<u64>,
    pub worker: BoxFuture<'static, anyhow::Result<()>>,
    pub cancel: CancellationToken,
}
