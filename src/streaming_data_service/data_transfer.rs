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
use anyhow::Context;
use bytes::Bytes;
use std::ffi::OsStr;
use std::io::Seek;
use std::{io::ErrorKind, path::PathBuf};
use tokio::fs::OpenOptions;
use tokio::io::AsyncRead;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;

#[derive(Debug)]
pub enum DataTransfer {
    Local {
        path: PathBuf,
    },
    Remote {
        file_name: PathBuf,
        size: u64,
        sender: Option<mpsc::Sender<bytes::Bytes>>,
        receiver: mpsc::Receiver<bytes::Bytes>,
    },
}

impl DataTransfer {
    pub fn local(path: PathBuf) -> Self {
        Self::Local { path }
    }

    pub fn remote(file_name: PathBuf, size: u64, buffer_size: usize) -> Self {
        let (sender, receiver) = mpsc::channel(buffer_size);

        Self::Remote {
            file_name,
            size,
            sender: Some(sender),
            receiver,
        }
    }
}

impl DataTransfer {
    pub fn file_name(&self) -> anyhow::Result<&OsStr> {
        match self {
            DataTransfer::Local { path } => Ok(path
                .file_name()
                .ok_or(std::io::Error::from(ErrorKind::InvalidInput))?),
            DataTransfer::Remote {
                file_name,
                size: _,
                sender: _,
                receiver: _,
            } => Ok(file_name.as_os_str()),
        }
    }

    pub fn size(&self) -> anyhow::Result<u64> {
        match self {
            DataTransfer::Local { path } => {
                let mut file = std::fs::OpenOptions::new()
                    .read(true)
                    .open(path)
                    .with_context(|| path.to_string_lossy().to_string())?;
                file.seek(std::io::SeekFrom::End(0)).context("seek error")
            }
            DataTransfer::Remote {
                file_name: _,
                size,
                sender: _,
                receiver: _,
            } => Ok(*size),
        }
    }

    pub async fn reader(self) -> anyhow::Result<impl AsyncRead + Sync + Send + Unpin> {
        match self {
            DataTransfer::Local { path } => OpenOptions::new()
                .read(true)
                .open(&path)
                .await
                .map(|x| Box::new(BufReader::new(x)) as Box<dyn AsyncRead + Sync + Send + Unpin>)
                .with_context(|| path.to_string_lossy().to_string()),
            DataTransfer::Remote {
                file_name: _,
                size: _,
                sender: _,
                receiver,
            } => Ok(Box::new(StreamReader::new(
                ReceiverStream::new(receiver).map(Ok::<bytes::Bytes, std::io::Error>),
            )) as Box<dyn AsyncRead + Sync + Send + Unpin>),
        }
    }

    pub fn sender_half(&mut self) -> Option<mpsc::Sender<Bytes>> {
        if let Self::Remote {
            file_name: _,
            size: _,
            sender,
            receiver: _,
        } = self
        {
            return sender.take();
        }
        None
    }
}
