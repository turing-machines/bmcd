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
use std::sync::Arc;
use std::{io::ErrorKind, path::PathBuf};

use super::bmc_application::BmcApplication;
use super::firmware_runner::FirmwareRunner;
use crate::api::streaming_data_service::TransferAction;
use crate::hal::NodeId;
use bytes::Bytes;
use futures::future::BoxFuture;
use serde::Serialize;
use std::io::Seek;
use tokio::sync::mpsc;
use tokio::{fs::OpenOptions, io::AsyncRead, sync::watch};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tokio_util::{io::StreamReader, sync::CancellationToken};

#[derive(Debug)]
pub struct UpgradeAction {
    transfer_type: TransferType,
    upgrade_type: UpgradeType,
}

impl UpgradeAction {
    pub fn new(upgrade_type: UpgradeType, transfer_type: TransferType) -> Self {
        Self {
            transfer_type,
            upgrade_type,
        }
    }
}

#[async_trait::async_trait]
impl TransferAction for UpgradeAction {
    async fn into_data_processor(
        self,
        channel_size: usize,
        written_sender: watch::Sender<u64>,
        cancel: CancellationToken,
    ) -> std::io::Result<(
        Option<mpsc::Sender<Bytes>>,
        BoxFuture<'static, anyhow::Result<()>>,
    )> {
        let file_name = self.transfer_type.file_name()?;
        let size = self.transfer_type.size()?;
        let (sender, receiver) = self.transfer_type.transfer_channel(channel_size).await?;

        let worker = self.upgrade_type.run(FirmwareRunner::new(
            receiver,
            file_name,
            size,
            cancel,
            written_sender,
        ));
        Ok((sender, worker))
    }

    fn total_size(&self) -> std::io::Result<u64> {
        self.transfer_type.size()
    }
}

#[derive(Debug)]
pub enum UpgradeType {
    OsUpgrade,
    Module(NodeId, Arc<BmcApplication>),
}

impl UpgradeType {
    pub fn run(
        self,
        firmware_runner: FirmwareRunner,
    ) -> BoxFuture<'static, Result<(), anyhow::Error>> {
        match self {
            UpgradeType::OsUpgrade => Box::pin(firmware_runner.os_update()),
            UpgradeType::Module(bmc, node) => Box::pin(firmware_runner.flash_node(node, bmc)),
        }
    }
}

#[derive(Debug, Serialize)]
pub enum TransferType {
    Local(String),
    Remote(String, u64),
}

impl TransferType {
    pub fn size(&self) -> std::io::Result<u64> {
        match self {
            TransferType::Local(path) => {
                let mut file = std::fs::OpenOptions::new().read(true).open(path)?;
                file.seek(std::io::SeekFrom::End(0))
            }
            TransferType::Remote(_, size) => Ok(*size),
        }
    }

    pub fn file_name(&self) -> std::io::Result<String> {
        match self {
            TransferType::Local(path) => {
                let file_name = PathBuf::from(path)
                    .file_name()
                    .ok_or(std::io::Error::from(ErrorKind::InvalidInput))?
                    .to_string_lossy()
                    .to_string();
                Ok(file_name)
            }
            TransferType::Remote(file_name, _) => Ok(file_name.clone()),
        }
    }

    pub async fn transfer_channel(
        self,
        items: usize,
    ) -> std::io::Result<(
        Option<mpsc::Sender<Bytes>>,
        Box<dyn AsyncRead + Sync + Send + Unpin>,
    )> {
        match self {
            TransferType::Local(path) => OpenOptions::new().read(true).open(&path).await.map(|x| {
                (
                    None,
                    Box::new(x) as Box<dyn AsyncRead + Sync + Send + Unpin>,
                )
            }),
            TransferType::Remote(_, _) => {
                let (sender, receiver) = mpsc::channel(items);
                Ok((
                    Some(sender),
                    Box::new(StreamReader::new(
                        ReceiverStream::new(receiver).map(Ok::<bytes::Bytes, std::io::Error>),
                    )),
                ))
            }
        }
    }
}
