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

use bytes::Bytes;
use serde::{Serialize, Serializer};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

/// Context object for node flashing. This object acts as a "cancel-guard" for
/// the [`crate::StreamingDataService`]. If [`TransferContext`] gets dropped, it will
/// cancel its "cancel" token, effectively aborting the node flash task. This
/// typically happens on a state transition inside the [`crate::StreamingDataService`].
#[derive(Serialize)]
pub struct TransferContext {
    pub id: u32,
    pub process_name: String,
    pub size: u64,
    #[serde(skip)]
    pub data_sender: Option<mpsc::Sender<Bytes>>,
    #[serde(serialize_with = "serialize_cancellation_token")]
    cancelled: CancellationToken,
    #[serde(serialize_with = "serialize_written_bytes")]
    bytes_written: watch::Receiver<u64>,
}

impl TransferContext {
    pub fn new(
        id: u32,
        process_name: String,
        size: u64,
        written_receiver: watch::Receiver<u64>,
        data_sender: Option<mpsc::Sender<Bytes>>,
        cancel_token: CancellationToken,
    ) -> Self {
        TransferContext {
            id,
            size,
            process_name,
            cancelled: cancel_token,
            bytes_written: written_receiver,
            data_sender,
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

fn serialize_written_bytes<S>(receiver: &watch::Receiver<u64>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_u64(*receiver.borrow())
}
