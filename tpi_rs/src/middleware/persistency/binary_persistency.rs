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
use std::collections::HashMap;
use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::ops::Deref;
use tokio::sync::watch;
use tokio::sync::watch::Receiver;
use tokio::sync::watch::Sender;
use tokio::sync::RwLock;
use tokio::time::Instant;

use super::default_hash;
use super::error::PersistencyError;

const BINARY_VERSION: u32 = 1;
const BINARY_MAGIC: &[u8; 7] = b"TMAPPDB";
const LEB_SIZE: u32 = 252 * 1024;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct PersistencyHeader {
    pub version: u32,
    pub magic: [u8; 7],
    pub data_size: u32,
    pub data_offset: u16,
}

impl<'a> PersistencyHeader {
    fn new() -> Result<Self, PersistencyError<'a>> {
        let mut header = PersistencyHeader {
            version: BINARY_VERSION,
            magic: *BINARY_MAGIC,
            data_size: 0,
            data_offset: 0,
        };

        header.data_offset = u16::try_from(
            bincode::serialized_size(&header)
                .map_err(|e| PersistencyError::serialization("header size", e))?,
        )
        .expect("current binary format does not support the given offset");
        Ok(header)
    }

    fn serialized_size() -> Result<u64, PersistencyError<'a>> {
        bincode::serialized_size(&PersistencyHeader::new()?)
            .map_err(|e| PersistencyError::serialization("header size", e))
    }
}

type Context = (HashMap<u64, Vec<u8>>, Option<Sender<Instant>>);

/// [`PersistencyStore`] is a in memory key-value store that is designed to
/// store application state. Its able to serialize and deserialize its store
/// from a binary file or memory buffer.
/// Passed `sources` that contain serialization errors are skipped in their
/// totality. A '.get()' would return this case the default value
#[derive(Debug)]
pub struct PersistencyStore {
    cache: RwLock<Context>,
}

impl<'a> PersistencyStore {
    pub(super) fn new<I, S>(keys: I, source: S) -> Result<Self, PersistencyError<'a>>
    where
        I: IntoIterator<Item = (&'static str, Vec<u8>)>,
        S: Read + Seek + 'a,
    {
        let iter = keys.into_iter().map(|(k, v)| (default_hash(k), v));
        let mut cache = HashMap::from_iter(iter);

        if let Err(e) = Self::try_deserialize_source(source, &mut cache) {
            log::error!("coninue-ing without loading persistency: {}", e);
        }

        Ok(Self {
            cache: RwLock::new((cache, None)),
        })
    }

    fn try_deserialize_source(
        mut source: impl Read + Seek + 'a,
        destination: &mut HashMap<u64, Vec<u8>>,
    ) -> Result<(), PersistencyError<'a>> {
        source.rewind()?;
        let size = source.seek(SeekFrom::End(0))?;
        let header_size = PersistencyHeader::serialized_size()?;
        match size {
            x if x >= header_size => {
                source.rewind()?;
                destination.extend(Self::try_load_data(source)?);
                Ok(())
            }
            0 => {
                log::info!("new storage");
                Ok(())
            }
            _ => Err(PersistencyError::UnknownFormat),
        }
    }

    fn try_load_data(
        mut source: impl Read + Seek,
    ) -> Result<impl IntoIterator<Item = (u64, Vec<u8>)>, PersistencyError<'a>> {
        let header: PersistencyHeader = bincode::deserialize_from(&mut source)
            .map_err(|e| PersistencyError::serialization("header deserialization", e))?;

        if &header.magic != BINARY_MAGIC {
            return Err(PersistencyError::UnknownFormat);
        }

        if header.version != BINARY_VERSION {
            return Err(PersistencyError::UnsupportedVersion(header.version));
        }

        if header.data_size > LEB_SIZE {
            log::warn!("internal persistency grew over the size of one logical erase block");
        }

        source.seek(io::SeekFrom::Start(header.data_offset.into()))?;
        let data: HashMap<u64, Vec<u8>> = bincode::deserialize_from(source)
            .map_err(|e| PersistencyError::serialization("cache load", e))?;
        Ok(data)
    }

    pub async fn get_watcher(&self) -> Receiver<Instant> {
        let (sender, watcher) = watch::channel(Instant::now());
        self.cache.write().await.1 = Some(sender);
        watcher
    }

    pub(super) async fn write(
        &self,
        mut source: impl Write + Seek + 'a,
    ) -> Result<(), PersistencyError<'a>> {
        let cache = self.cache.read().await;
        let data = bincode::serialize(&cache.deref().0)
            .map_err(|e| PersistencyError::SerializationError("data serialization".into(), e))?;

        let mut header = PersistencyHeader::new()?;
        header.data_size =
            u32::try_from(data.len()).expect("persistency size > 4.2GB not supported");

        let header_bytes = bincode::serialize(&header)
            .map_err(|e| PersistencyError::SerializationError("header serialization".into(), e))?;

        source.rewind()?;
        source.write_all(&header_bytes)?;
        source.write_all(&data)?;
        Ok(())
    }

    pub async fn get<T>(&self, key: &str) -> T
    where
        for<'b> T: serde::Deserialize<'b>,
    {
        self.try_get(key).await.unwrap()
    }

    pub async fn try_get<T>(&self, key: &'a str) -> Result<T, PersistencyError<'a>>
    where
        for<'b> T: serde::Deserialize<'b>,
    {
        self.cache
            .read()
            .await
            .0
            .get(&default_hash(key))
            .ok_or(PersistencyError::UnknownKey(key.into()))
            .and_then(|bytes| {
                bincode::deserialize(bytes).map_err(|e| PersistencyError::serialization(key, e))
            })
    }

    pub async fn set<T>(&self, key: &str, value: T)
    where
        T: serde::Serialize,
    {
        self.try_set(key, value).await.unwrap()
    }

    pub async fn try_set<T>(&self, key: &'a str, value: T) -> Result<(), PersistencyError<'a>>
    where
        T: serde::Serialize,
    {
        let encoded =
            bincode::serialize(&value).map_err(|e| PersistencyError::serialization(key, e))?;

        let mut cache = self.cache.write().await;

        let k = default_hash(key);
        if !cache.0.contains_key(&k) {
            return Err(PersistencyError::UnknownKey(key.to_string()));
        }

        let previous = cache.0.insert(k, encoded.clone());

        if previous.as_ref() != Some(&encoded)
            && cache.1.is_some()
            && cache.1.as_ref().unwrap().send(Instant::now()).is_err()
        {
            log::info!("persistency watcher dropped");
            cache.1 = None;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_invalid_header_length() {
        let arr = [0u8; 13];
        let cursor = Cursor::new(arr);
        let mut map = HashMap::new();
        let result = PersistencyStore::try_deserialize_source(cursor, &mut map);
        assert!(matches!(result, Err(PersistencyError::UnknownFormat)));
    }

    #[test]
    fn test_invalid_header_magic() {
        let mut header = PersistencyHeader::new().unwrap();
        header.magic = b"invalid".to_owned();
        let vec = bincode::serialize(&header).unwrap();
        let cursor = Cursor::new(vec);
        assert!(matches!(
            PersistencyStore::try_load_data(cursor),
            Err(PersistencyError::UnknownFormat)
        ));
    }

    #[test]
    fn test_invalid_header_version() {
        let mut header = PersistencyHeader::new().unwrap();
        header.version = 3;
        let vec = bincode::serialize(&header).unwrap();
        let cursor = Cursor::new(vec);
        assert!(matches!(
            PersistencyStore::try_load_data(cursor),
            Err(PersistencyError::UnsupportedVersion(3))
        ));
    }

    #[test]
    fn test_invalid_data() {
        let header = PersistencyHeader::new().unwrap();
        let mut vec = bincode::serialize(&header).unwrap();
        let data = [0xffu8, 2];
        vec.extend_from_slice(&data);

        let cursor = Cursor::new(vec);
        assert!(matches!(
            PersistencyStore::try_load_data(cursor),
            Err(PersistencyError::SerializationError(_, _))
        ));
    }

    #[tokio::test]
    async fn test_write_data() {
        let mut data = HashMap::<u64, Vec<u8>>::new();
        let mut header = PersistencyHeader::new().unwrap();
        data.insert(default_hash("test"), bincode::serialize(&222u128).unwrap());
        header.data_size = bincode::serialized_size(&data).unwrap() as u32;
        let mut vec = bincode::serialize(&header).unwrap();
        vec.append(&mut bincode::serialize(&data).unwrap());

        let mut cursor = Cursor::new(vec);
        let store = PersistencyStore::new(
            [("test", bincode::serialize(&123u128).unwrap())],
            &mut cursor,
        )
        .unwrap();

        let buffer = [0u8; 123];
        let mut write_cursor = Cursor::new(buffer);
        store.write(&mut write_cursor).await.unwrap();
        assert_eq!(
            cursor.get_ref()[..cursor.position() as usize],
            write_cursor.get_ref()[..write_cursor.position() as usize]
        );
    }

    #[tokio::test]
    async fn read_write_test() {
        let mut cursor = Cursor::new(Vec::with_capacity(128));
        let store = PersistencyStore::new(
            [("test", bincode::serialize(&123u128).unwrap())],
            &mut cursor,
        )
        .unwrap();

        // nothing is written on init
        assert_eq!(cursor.position(), 0);
        assert!(cursor.get_ref().is_empty());

        let get_unknown = store.try_get::<u128>("doesnotexist").await;
        assert!(matches!(get_unknown, Err(PersistencyError::UnknownKey(k)) if k == "doesnotexist"));

        let set_unknown = store.try_set("doesnotexist", 1u128).await;
        assert!(matches!(set_unknown, Err(PersistencyError::UnknownKey(k)) if k == "doesnotexist"));

        let watcher = store.get_watcher().await;
        assert_eq!(store.get::<u128>("test").await, 123u128);
        store.set("test", 333u128).await;
        assert!(watcher.has_changed().unwrap());
        assert_eq!(store.get::<u128>("test").await, 333u128);
    }
}
