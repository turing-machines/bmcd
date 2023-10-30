//! Handlers for UART connections to/from nodes
use crate::hal::NodeId;
use anyhow::Result;
use bytes::{Bytes, BytesMut};
use std::error::Error;
use std::fmt::Display;
#[derive(Debug)]
pub struct SerialConnections;

impl SerialConnections {
    pub fn new() -> Result<Self> {
        Ok(SerialConnections)
    }

    pub async fn run(&self) -> Result<(), SerialError> {
        Ok(())
    }

    pub async fn read(&self, _: NodeId) -> Result<Bytes, SerialError> {
        let data: &'static [u8] = b"this is a stub implementation";
        Ok(data.into())
    }

    pub async fn write<B: Into<BytesMut> + std::fmt::Debug>(
        &self,
        node: NodeId,
        data: B,
    ) -> Result<(), SerialError> {
        log::warn!("writing {}: {:?}", node, data);
        Ok(())
    }
}

#[derive(Debug)]
pub enum SerialError {
    NotStarted,
    AlreadyRunning,
    InternalError(String),
}

impl Error for SerialError {}

impl Display for SerialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SerialError::NotStarted => write!(f, "serial worker not started"),
            SerialError::AlreadyRunning => write!(f, "already running"),
            SerialError::InternalError(e) => e.fmt(f),
        }
    }
}
