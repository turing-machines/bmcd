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
use crate::utils::Sha256StreamValidator;
use crate::Path;
use anyhow::Context;
use async_compression::tokio::bufread::XzDecoder;
use bytes::Bytes;
use humansize::DECIMAL;
use nix::unistd::SysconfVar;
use std::ffi::OsStr;
use std::io::Seek;
use std::{io::ErrorKind, path::PathBuf};
use tokio::fs::OpenOptions;
use tokio::io;
use tokio::io::AsyncBufRead;
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
        sha256: Option<bytes::Bytes>,
        sender: Option<mpsc::Sender<bytes::Bytes>>,
        receiver: Option<mpsc::Receiver<bytes::Bytes>>,
    },
}

impl DataTransfer {
    pub fn local(path: PathBuf) -> Self {
        Self::Local { path }
    }

    pub fn remote(
        file_name: PathBuf,
        size: u64,
        buffer_size: usize,
        sha256: Option<bytes::Bytes>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(buffer_size);

        Self::Remote {
            file_name,
            size,
            sha256,
            sender: Some(sender),
            receiver: Some(receiver),
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
                sha256: _,
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
                sha256: _,
                sender: _,
                receiver: _,
            } => Ok(*size),
        }
    }

    pub async fn reader(&mut self) -> anyhow::Result<impl AsyncRead + Sync + Send + Unpin> {
        match self {
            DataTransfer::Local { path } => {
                let file = OpenOptions::new()
                    .read(true)
                    .open(&path)
                    .await
                    .with_context(|| path.to_string_lossy().to_string())?;

                Ok(with_decompression_support(path, BufReader::new(file)))
            }
            DataTransfer::Remote {
                file_name,
                size: _,
                sha256,
                sender: _,
                receiver,
            } => {
                let receiver_stream =
                    ReceiverStream::new(receiver.take().expect("cannot take reader twice"));

                if let Some(sha) = sha256.clone() {
                    log::info!(
                        "crc validator enabled. expected sha256:{}",
                        hex::encode(&sha)
                    );

                    Ok(with_decompression_support(
                        file_name,
                        StreamReader::new(Sha256StreamValidator::new(receiver_stream, sha)),
                    ))
                } else {
                    Ok(with_decompression_support(
                        file_name,
                        StreamReader::new(receiver_stream.map(Ok::<bytes::Bytes, io::Error>)),
                    ))
                }
            }
        }
    }

    pub fn sender_half(&mut self) -> Option<mpsc::Sender<Bytes>> {
        if let Self::Remote {
            file_name: _,
            size: _,
            sha256: _,
            sender,
            receiver: _,
        } = self
        {
            return sender.take();
        }
        None
    }
}

fn with_decompression_support(
    file: &Path,
    reader: impl AsyncBufRead + Send + Sync + Unpin + 'static,
) -> Box<dyn AsyncRead + Send + Sync + Unpin> {
    if file.extension().unwrap_or_default() == "xz" {
        let mem_limit = available_memory().unwrap_or(50 * 1024 * 1024);
        log::info!(
            "enabled xz decoder with limit {} for {}",
            humansize::format_size(mem_limit, DECIMAL),
            file.to_string_lossy()
        );

        let decoder = XzDecoder::with_mem_limit(reader, mem_limit);
        Box::new(decoder) as Box<dyn AsyncRead + Sync + Send + Unpin>
    } else {
        Box::new(reader) as Box<dyn AsyncRead + Sync + Send + Unpin>
    }
}

fn available_memory() -> io::Result<u64> {
    let physical_pages = nix::unistd::sysconf(SysconfVar::_AVPHYS_PAGES)?;
    let page_size = nix::unistd::sysconf(SysconfVar::PAGE_SIZE)?;
    Ok(physical_pages
        .and_then(|pages| page_size.map(|size| size as u64 * pages as u64))
        .unwrap_or(u64::MAX))
}
