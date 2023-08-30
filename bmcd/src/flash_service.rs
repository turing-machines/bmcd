#![allow(dead_code, unused)]
use crate::into_legacy_response::LegacyResponse;
use actix_web::{http::StatusCode, web::Bytes};
use std::{error::Error, fmt::Display, sync::Arc};
use tokio::{
    io::{AsyncRead, BufReader},
    sync::mpsc::{channel, error::SendError, Receiver, Sender},
};
use tpi_rs::{
    app::bmc_application::BmcApplication,
    middleware::{firmware_update::SUPPORTED_DEVICES, NodeId, UsbRoute},
};
use tpi_rs::{app::flash_application::flash_node, middleware::firmware_update::FlashStatus};
use tpi_rs::{app::flash_application::FlashContext, utils::ReceiverReader};

pub struct FlashService {
    status: Option<Sender<Bytes>>,
    bmc: Arc<BmcApplication>,
}

impl FlashService {
    pub fn new(bmc: Arc<BmcApplication>) -> Self {
        Self { status: None, bmc }
    }

    pub async fn start_transfer(
        &mut self,
        filename: String,
        size: usize,
        node: NodeId,
    ) -> Result<(), FlashError> {
        if self.status.is_some() {
            return Err(FlashError::InProgress);
        }

        let (sender, receiver) = channel::<Bytes>(128);
        let (progress_sender, progress_receiver) = channel(32);
        let context = FlashContext {
            filename,
            size,
            node,
            byte_stream: ReceiverReader::new(receiver),
            bmc: self.bmc.clone(),
            progress_sender,
        };

        /// execute the flashing of the image.
        tokio::spawn(flash_node(context));

        self.status = Some(sender);
        Ok(())
    }

    pub async fn stream_chunk(&mut self, data: Bytes) -> Result<(), FlashError> {
        if let Some(sender) = &self.status {
            match sender.send(data).await {
                Ok(_) => Ok(()),
                Err(e) if sender.is_closed() => Err(FlashError::Aborted),
                Err(e) => Err(e.into()),
            }
        } else {
            Err(FlashError::UnexpectedCommand)
        }
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
