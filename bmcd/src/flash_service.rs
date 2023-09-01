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
};
use tokio::sync::mpsc::{channel, error::SendError, Sender};
use tpi_rs::app::flash_application::flash_node;
use tpi_rs::{app::bmc_application::BmcApplication, middleware::NodeId, utils::logging_sink};
use tpi_rs::{app::flash_application::FlashContext, utils::ReceiverReader};

pub type FlashDoneFut = BoxFuture<'static, anyhow::Result<()>>;
pub struct FlashService {
    status: Option<(u64, Sender<Bytes>)>,
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
        if self.status.is_some() {
            return Err(FlashError::InProgress);
        }

        let (sender, receiver) = channel::<Bytes>(128);
        let (progress_sender, progress_receiver) = channel(32);
        logging_sink(progress_receiver);

        let context = FlashContext {
            filename,
            size,
            node,
            byte_stream: ReceiverReader::new(receiver),
            bmc: self.bmc.clone(),
            progress_sender,
        };

        // execute flashing of the image.
        let flash_handle = tokio::spawn(flash_node(context));

        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        self.status = Some((hasher.finish(), sender));
        Ok(Box::pin(async move {
            flash_handle
                .await
                .context("join error waiting for flashing to complete")?
        }))
    }

    pub async fn put_chunk(&mut self, peer: &str, data: Bytes) -> Result<(), FlashError> {
        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let hashed_peer = hasher.finish();

        if let Some((hash, sender)) = &self.status {
            if hash != &hashed_peer {
                return Err(FlashError::UnexpectedCommand);
            }

            match sender.send(data).await {
                Ok(_) => Ok(()),
                Err(_) if sender.is_closed() => Err(FlashError::Aborted),
                Err(e) => Err(e.into()),
            }
        } else {
            Err(FlashError::UnexpectedCommand)
        }
    }

    pub fn reset(&mut self) {
        self.status = None;
    }
}

#[derive(Debug, PartialEq)]
pub enum FlashError {
    InProgress,
    UnexpectedCommand,
    Aborted,
    MpscError(SendError<Bytes>),
}

impl Error for FlashError {}

impl Display for FlashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlashError::InProgress => write!(f, "flashing operation in progress"),
            FlashError::UnexpectedCommand => write!(f, "did not expect that command"),
            FlashError::Aborted => write!(f, "flash operation was aborted"),
            FlashError::MpscError(_) => write!(f, "internal error sending buffers"),
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
        match value {
            FlashError::InProgress => (
                StatusCode::SERVICE_UNAVAILABLE,
                "another flash operation is in progress",
            )
                .into(),
            FlashError::UnexpectedCommand => {
                (StatusCode::BAD_REQUEST, "did not expect given request").into()
            }
            FlashError::MpscError(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into(),
            FlashError::Aborted => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Flashing process aborted",
            )
                .into(),
        }
    }
}
