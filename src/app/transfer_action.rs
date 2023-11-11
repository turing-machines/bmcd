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
use super::bmc_application::BmcApplication;
use super::upgrade_worker::UpgradeWorker;
use crate::hal::NodeId;
use crate::streaming_data_service::data_transfer::DataTransfer;
use crate::streaming_data_service::TransferRequest;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct InitializeTransfer {
    transfer_name: String,
    data_transfer: DataTransfer,
    upgrade_command: UpgradeCommand,
}

impl InitializeTransfer {
    pub fn new(
        transfer_name: String,
        upgrade_command: UpgradeCommand,
        data_transfer: DataTransfer,
    ) -> Self {
        Self {
            transfer_name,
            data_transfer,
            upgrade_command,
        }
    }
}

impl TryInto<TransferRequest> for InitializeTransfer {
    type Error = anyhow::Error;

    fn try_into(mut self) -> Result<TransferRequest, Self::Error> {
        let size = self.data_transfer.size()?;
        let sender = self.data_transfer.sender_half();
        let cancel = CancellationToken::new();
        let cancel_child = cancel.child_token();
        let (written_sender, written_receiver) = watch::channel(0u64);
        let worker = self.upgrade_command.run(UpgradeWorker::new(
            self.data_transfer,
            cancel_child,
            written_sender,
        ));

        Ok(TransferRequest {
            process_name: self.transfer_name,
            size,
            sender,
            progress_watcher: written_receiver,
            worker,
            cancel,
        })
    }
}

#[derive(Debug)]
pub enum UpgradeCommand {
    OsUpgrade,
    Module(NodeId, Arc<BmcApplication>),
}

impl UpgradeCommand {
    pub fn run(
        self,
        upgrade_worker: UpgradeWorker,
    ) -> BoxFuture<'static, Result<(), anyhow::Error>> {
        match self {
            UpgradeCommand::OsUpgrade => Box::pin(upgrade_worker.os_update()),
            UpgradeCommand::Module(bmc, node) => Box::pin(upgrade_worker.flash_node(node, bmc)),
        }
    }
}
