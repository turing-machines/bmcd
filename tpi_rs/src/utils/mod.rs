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
mod event_listener;
use std::fmt::Display;

#[doc(inline)]
pub use event_listener::*;
mod io;
pub use io::*;
use tokio::sync::mpsc::Receiver;

// for now we print the status updates to console. In the future we would like to pass
// this back to the clients.
pub fn logging_sink<T: Display + Send + 'static>(
    mut receiver: Receiver<T>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(msg) = receiver.recv().await {
            log::info!("{}", msg);
        }
    })
}
