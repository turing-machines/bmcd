use crate::into_legacy_response::LegacyResponse;
use actix_web::{http::StatusCode, web::Bytes};
use anyhow::Context;
use futures::future::BoxFuture;
use std::{
    collections::hash_map::DefaultHasher,
    error::Error,
    fmt::Display,
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{channel, error::SendError, Sender};
use tokio_util::sync::CancellationToken;
use tpi_rs::app::flash_application::flash_node;
use tpi_rs::{app::bmc_application::BmcApplication, middleware::NodeId, utils::logging_sink};
use tpi_rs::{app::flash_application::FlashContext, utils::ReceiverReader};

pub type FlashDoneFut = BoxFuture<'static, anyhow::Result<()>>;
const RESET_TIMEOUT: Duration = Duration::from_secs(10);

struct TransferContext {
    pub peer: u64,
    pub bytes_sender: Sender<Bytes>,
    pub cancel: CancellationToken,
    last_recieved_chunk: Instant,
}

impl TransferContext {
    pub fn duration_since_last_chunk(&self) -> Duration {
        Instant::now().saturating_duration_since(self.last_recieved_chunk)
    }
}

pub struct FlashService {
    status: Option<TransferContext>,
    bmc: Arc<BmcApplication>,
}

impl FlashService {
    pub fn new(bmc: Arc<BmcApplication>) -> Self {
        Self { status: None, bmc }
    }

    pub async fn start_transfer(
        &mut self,
        peer: &str,
        filename: String,
        size: usize,
        node: NodeId,
    ) -> Result<FlashDoneFut, FlashError> {
        if let Some(context) = &self.status {
            if context.duration_since_last_chunk() < RESET_TIMEOUT {
                return Err(FlashError::InProgress);
            } else {
                log::warn!(
                    "Assuming last transfer will never complete as last request was {}s ago. Resetting flash service",
                    context.duration_since_last_chunk().as_secs()
                );
                self.reset();
            }
        }

        let (sender, receiver) = channel::<Bytes>(128);
        let (progress_sender, progress_receiver) = channel(32);
        let done_token = CancellationToken::new();
        let context = FlashContext {
            filename,
            size,
            node,
            byte_stream: ReceiverReader::new(receiver),
            bmc: self.bmc.clone(),
            progress_sender,
            cancel: done_token.clone(),
        };

        // execute flashing of the image.
        let flash_handle = tokio::spawn(flash_node(context));
        logging_sink(progress_receiver);

        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);

        let context = TransferContext {
            peer: hasher.finish(),
            bytes_sender: sender,
            cancel: done_token.clone(),
            last_recieved_chunk: Instant::now(),
        };

        self.status = Some(context);

        Ok(Box::pin(async move {
            let result = flash_handle
                .await
                .context("join error waiting for flashing to complete");
            done_token.cancel();
            result?
        }))
    }

    pub async fn put_chunk(&mut self, peer: String, data: Bytes) -> Result<(), FlashError> {
        if data.is_empty() {
            self.reset();
            return Err(FlashError::EmptyPayload);
        }

        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let hashed_peer = hasher.finish();

        let result = if let Some(context) = &mut self.status {
            if context.peer != hashed_peer {
                return Err(FlashError::PeersDoNotMatch(peer));
            }

            match context.bytes_sender.send(data).await {
                Ok(_) => {
                    context.last_recieved_chunk = Instant::now();
                    Ok(())
                }
                Err(_) if context.bytes_sender.is_closed() => Err(FlashError::Aborted),
                Err(e) => Err(e.into()),
            }
        } else {
            Err(FlashError::TransferNotStarted)
        };

        if result.is_err() {
            self.reset();
        }

        result
    }

    fn reset(&mut self) {
        if let Some(context) = &self.status {
            context.cancel.cancel();
        }
        self.status = None;
    }
}

#[derive(Debug, PartialEq)]
pub enum FlashError {
    InProgress,
    TransferNotStarted,
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
            FlashError::TransferNotStarted => {
                write!(f, "transfer not started yet, did not expect that command")
            }
            FlashError::Aborted => write!(f, "flash operation was aborted"),
            FlashError::MpscError(_) => write!(f, "internal error sending buffers"),
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
            FlashError::TransferNotStarted => StatusCode::BAD_REQUEST,
            FlashError::MpscError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            FlashError::Aborted => StatusCode::INTERNAL_SERVER_ERROR,
            FlashError::EmptyPayload => StatusCode::BAD_REQUEST,
            FlashError::PeersDoNotMatch(_) => StatusCode::BAD_REQUEST,
        };
        (status_code, value.to_string()).into()
    }
}
