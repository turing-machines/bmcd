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

use crate::api::streaming_data_service::StreamingServiceError;
use bytes::Bytes;
use rand::Rng;
use serde::{Serialize, Serializer};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

/// Context object for node flashing. This object acts as a "cancel-guard" for
/// the [`StreamingDataService`]. If [`TransferContext`] gets dropped, it will
/// cancel its "cancel" token, effectively aborting the node flash task. This
/// typically happens on a state transition inside the [`StreamingDataService`].
#[derive(Serialize, Debug)]
pub struct TransferContext {
    pub id: u64,
    pub peer: String,
    #[serde(skip)]
    pub peer_hash: u64,
    pub process_name: String,
    pub size: u64,
    #[serde(serialize_with = "serialize_source", rename = "bytes_sent")]
    reader: TransferSource,
    #[serde(serialize_with = "serialize_cancellation_token")]
    cancelled: CancellationToken,
    #[serde(serialize_with = "serialize_seconds_until_now")]
    last_recieved_chunk: Instant,
    #[serde(serialize_with = "serialize_written_bytes")]
    bytes_written: watch::Receiver<u64>,
}

impl TransferContext {
    pub fn new<S: Into<TransferSource>>(
        peer: String,
        process_name: String,
        transfer_source: S,
        size: u64,
        written_receiver: watch::Receiver<u64>,
    ) -> Self {
        let mut rng = rand::thread_rng();
        let id = rng.gen();

        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let peer_hash = hasher.finish();

        TransferContext {
            id,
            peer,
            peer_hash,
            process_name,
            size,
            reader: transfer_source.into(),
            cancelled: CancellationToken::new(),
            last_recieved_chunk: Instant::now(),
            bytes_written: written_receiver,
        }
    }

    pub fn duration_since_last_chunk(&self) -> Duration {
        Instant::now().saturating_duration_since(self.last_recieved_chunk)
    }

    pub fn is_equal_peer(&self, peer: &str) -> Result<(), StreamingServiceError> {
        let mut hasher = DefaultHasher::new();
        peer.hash(&mut hasher);
        let hashed_peer = hasher.finish();
        if self.peer_hash != hashed_peer {
            return Err(StreamingServiceError::PeersDoNotMatch(peer.to_string()));
        }

        Ok(())
    }

    /// Send given bytes through a channel towards the object that is
    /// processing the file transfer ([`FirmwareRunner`]). This function should
    /// defer from making any application and state transitions. This is up to
    /// to the receiver side. This function does however contain some conveniences
    /// to book-keep transfer meta-data.
    /// This function is only relevant for cases where source ==
    /// TransferSource::Peer(_)`
    pub async fn push_bytes(&mut self, data: Bytes) -> Result<(), StreamingServiceError> {
        let (bytes_sender, bytes_sent) = match &mut self.reader {
            TransferSource::Local => return Err(StreamingServiceError::IsLocalTransfer),
            TransferSource::Peer(None, _) => return Err(StreamingServiceError::LengthExceeded),
            TransferSource::Peer(sender, b) => (sender, b),
        };

        let len = data.len();
        match bytes_sender.as_mut().unwrap().send(data).await {
            Ok(_) => {
                self.last_recieved_chunk = Instant::now();
                *bytes_sent += len as u64;
                // Close the channel to signal to the other side that the last
                // chunk was sent. We cannot however switch yet to "Done" state.
                // As its up to the receiving side to signal (see
                // 'StreamingDataService::execute_worker') when its done
                // processing this data.
                if *bytes_sent >= self.size {
                    log::info!("{}:{}", bytes_sent, self.size);
                    *bytes_sender = None;
                }
                Ok(())
            }
            Err(_) if bytes_sender.as_ref().unwrap().is_closed() => {
                Err(StreamingServiceError::Aborted(self.process_name.clone()))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_child_token(&self) -> CancellationToken {
        self.cancelled.child_token()
    }
}

impl Drop for TransferContext {
    fn drop(&mut self) {
        self.cancelled.cancel();
    }
}

fn serialize_cancellation_token<S>(
    cancel_token: &CancellationToken,
    s: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_bool(cancel_token.is_cancelled())
}

fn serialize_seconds_until_now<S>(instant: &Instant, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let secs = Instant::now().saturating_duration_since(*instant).as_secs();
    s.serialize_u64(secs)
}

fn serialize_written_bytes<S>(receiver: &watch::Receiver<u64>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_u64(*receiver.borrow())
}

fn serialize_source<S>(source: &TransferSource, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match source {
        TransferSource::Local => s.serialize_none(),
        TransferSource::Peer(_, bytes_sent) => s.serialize_u64(*bytes_sent),
    }
}

#[derive(Debug)]
pub enum TransferSource {
    Local,
    Peer(Option<mpsc::Sender<Bytes>>, u64),
}

impl From<mpsc::Sender<Bytes>> for TransferSource {
    fn from(value: mpsc::Sender<Bytes>) -> Self {
        TransferSource::Peer(Some(value), 0)
    }
}
