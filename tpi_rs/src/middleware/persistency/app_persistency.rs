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
use std::future;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::binary_persistency::PersistencyStore;
use anyhow::Context;
use futures::future::Either;
use tokio::fs::{File, OpenOptions};
use tokio::time::sleep_until;

const BIN_DATA: &str = "/var/lib/bmcd/bmcd.bin";
const WRITE_BACK_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug)]
enum MonitorEvent {
    StoreChange,
    PersistencyWritten,
}

/// Builder to aid in configuring and setting up a [`ApplicationPersistency`], a
/// "key/value" store.
#[derive(Debug)]
pub struct PersistencyBuilder {
    keys: Vec<(&'static str, Vec<u8>)>,
    write_timeout: Option<Duration>,
}

impl PersistencyBuilder {
    /// Add a key to the key/value store. Attempting to access keys that are not
    /// registered with this function will result in an error.
    pub fn register_key<T>(mut self, key: &'static str, default: &T) -> Self
    where
        T: std::fmt::Debug + serde::Serialize,
    {
        self.keys.push((
            key,
            bincode::serialize(default)
                .with_context(|| format!("fatal serialization error of {:?}", default))
                .unwrap(),
        ));
        self
    }

    /// The [`ApplicationPersistency`] contains a write mechanism that writes the
    /// key/value store back to the file-system. This happens on a timeout
    /// occurrence started from the last write. This function disables the write
    /// on timeout. The key/value store only gets written when the
    /// [`ApplicationPersistency`] is dropped.
    pub fn disable_write_on_timeout(mut self) -> Self {
        self.write_timeout = None;
        self
    }

    /// Construct an [`ApplicationPersistency`] object.
    pub async fn build(self) -> anyhow::Result<ApplicationPersistency> {
        ApplicationPersistency::new(self.keys, BIN_DATA, self.write_timeout).await
    }
}

impl Default for PersistencyBuilder {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            write_timeout: Some(WRITE_BACK_TIMEOUT),
        }
    }
}

#[derive(Debug)]
struct MonitorContext {
    pub file: PathBuf,
    pub inner: PersistencyStore,
}

impl MonitorContext {
    pub async fn commit_to_file(&self) -> anyhow::Result<MonitorEvent> {
        let mut new = self.file.clone();
        new.set_extension("new");

        let pending = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&new)
            .await?;
        self.inner.write(pending.into_std().await).await?;

        tokio::fs::rename(&new, &self.file)
            .await
            .context("permanent damaged persistency binary")?;

        Ok(MonitorEvent::PersistencyWritten)
    }

    pub async fn sync_all(&self) -> anyhow::Result<()> {
        self.commit_to_file().await?;

        let file = File::open(&self.file).await?;
        file.sync_all().await?;
        Ok(())
    }
}

/// This struct represents a concrete key/value store used by the bmcd. It sets
/// up a key/value store on the given path. It monitors the store for changes
/// and writes back all changes when no update to the store was detected for a
/// given amount of time (`write_timeout`). When `None` is passed as
/// `write_timeout` the key/value store only gets written when the
/// [`ApplicationPersistency`] is dropped.
#[derive(Debug)]
pub struct ApplicationPersistency {
    context: Arc<MonitorContext>,
}

impl ApplicationPersistency {
    pub async fn new<I, P>(
        keys_with_default: I,
        path: P,
        write_timeout: Option<Duration>,
    ) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = (&'static str, Vec<u8>)>,
        P: Into<PathBuf>,
    {
        let path = path.into();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(&parent).await?;
        }

        let source = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .await
            .with_context(|| path.to_string_lossy().to_string())?;

        let inner = PersistencyStore::new(keys_with_default, &mut source.into_std().await)?;

        let context = Arc::new(MonitorContext { file: path, inner });
        if let Some(duration) = write_timeout {
            tokio::spawn(Self::filesystem_writer(duration, context.clone()));
        }
        Ok(Self { context })
    }

    async fn filesystem_writer(
        write_timeout: Duration,
        context: Arc<MonitorContext>,
    ) -> anyhow::Result<()> {
        let mut watcher = context.inner.get_watcher().await;
        let mut write_filesystem = Either::Left(future::pending());

        loop {
            // Both items yield different `Result` types. Therefore use the
            // question mark operator on the results independently.
            let event = tokio::select! {
                result = watcher.changed() => {
                    result?;
                    MonitorEvent::StoreChange
                },
                result = write_filesystem => result?,
            };

            let new_future = match event {
                MonitorEvent::PersistencyWritten => {
                    Either::Left(future::pending::<anyhow::Result<MonitorEvent>>())
                }
                MonitorEvent::StoreChange => {
                    // When there is a change in the key/value store. reload the
                    // pending write_file-system task so that the next attempt
                    // will be in "now" + write_timeout
                    let new_deadline = watcher
                        .borrow_and_update()
                        .deref()
                        .checked_add(write_timeout)
                        .ok_or(anyhow::anyhow!("time structure internal error"))?;

                    let clone = context.clone();
                    Either::Right(async move {
                        sleep_until(new_deadline).await;
                        clone.commit_to_file().await
                    })
                }
            };

            write_filesystem = new_future;
        }
    }
}

impl Deref for ApplicationPersistency {
    type Target = PersistencyStore;

    fn deref(&self) -> &Self::Target {
        &self.context.inner
    }
}

impl Drop for ApplicationPersistency {
    fn drop(&mut self) {
        let context = self.context.clone();
        tokio::spawn(async move {
            if let Err(e) = context.sync_all().await {
                log::error!("{}", e);
            }
        });
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use tempdir::TempDir;
    use tokio::time::sleep;

    #[tokio::test]
    async fn file_write_on_drop() {
        let tmp_dir = TempDir::new("persistency_test1").unwrap();
        let bin_file = tmp_dir.path().join("bmcd.bin");
        let keys_with_default = [("test", bincode::serialize(&123u128).unwrap())];

        let persistency = ApplicationPersistency::new(keys_with_default.clone(), &bin_file, None)
            .await
            .unwrap();
        persistency.set("test", &777u128).await;
        drop(persistency);

        // workaround to give the spawned task for syncing the file to disk some time
        sleep(Duration::from_millis(100)).await;
        let persistency = ApplicationPersistency::new(keys_with_default, bin_file, None)
            .await
            .unwrap();
        assert_eq!(persistency.get::<u128>("test").await, 777u128);
    }

    #[tokio::test]
    async fn persistency_monitor_test() {
        let tmp_dir = TempDir::new("persistency_test2").unwrap();
        let bin_file = tmp_dir.path().join("bmcd.bin");
        let keys_with_default = [("test", bincode::serialize(&123u128).unwrap())];

        let persistency = ApplicationPersistency::new(
            keys_with_default.clone(),
            &bin_file,
            Some(Duration::from_millis(200)),
        )
        .await
        .unwrap();

        persistency
            .write(File::create(&bin_file).await.unwrap().into_std().await)
            .await
            .unwrap();

        for n in 0..6u128 {
            sleep(Duration::from_millis(100)).await;
            persistency.set("test", &n).await;
        }

        let keys_with_default = [("test", bincode::serialize(&1u128).unwrap())];

        let persistency2 = ApplicationPersistency::new(keys_with_default.clone(), &bin_file, None)
            .await
            .unwrap();

        // check that the previous persistency instance did not write to file during the updates of
        // the test value. We expect to see the default value of the previous persistency
        assert_eq!(persistency2.get::<u128>("test").await, 123u128);
    }

    #[tokio::test]
    async fn persistency_monitor_timeout_test() {
        let tmp_dir = TempDir::new("persistency_test3").unwrap();
        let bin_file = tmp_dir.path().join("bmcd.bin");
        let keys_with_default = [("test", bincode::serialize(&123u128).unwrap())];

        assert!(!bin_file.exists());
        let _ = ApplicationPersistency::new(
            keys_with_default.clone(),
            &bin_file,
            Some(Duration::from_millis(200)),
        )
        .await
        .unwrap();

        sleep(Duration::from_millis(200)).await;
        assert!(bin_file.exists());
    }
}
