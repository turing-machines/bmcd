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
const BINARY_MAGIC: u32 = 0xdeadbeef;
const LEB_SIZE: u32 = 252 * 1024;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct PersistencyHeader {
    pub version: u32,
    pub magic: u32,
    pub data_size: u32,
    pub data_offset: u16,
}

impl PersistencyHeader {
    fn new() -> Result<Self, PersistencyError> {
        let mut header = PersistencyHeader {
            version: BINARY_VERSION,
            magic: BINARY_MAGIC,
            data_size: 0,
            data_offset: 0,
        };

        header.data_offset = bincode::serialized_size(&header)? as u16;
        Ok(header)
    }

    fn serialized_size() -> Result<u64, PersistencyError> {
        Ok(bincode::serialized_size(&PersistencyHeader::new()?)?)
    }
}

type Context = (HashMap<u64, Vec<u8>>, Option<Sender<Instant>>);
#[derive(Debug)]
pub struct PersistencyStore {
    cache: RwLock<Context>,
}

impl PersistencyStore {
    pub(super) fn new<I, S>(keys: I, source: &mut S) -> Result<Self, PersistencyError>
    where
        I: IntoIterator<Item = (&'static str, Vec<u8>)>,
        S: ?Sized + Read + Write + Seek,
    {
        let iter = keys.into_iter().map(|(k, v)| (default_hash(k), v));
        let mut cache = HashMap::from_iter(iter);

        source.rewind()?;
        let size = source.seek(SeekFrom::End(0))?;
        let header_size = PersistencyHeader::serialized_size()?;
        match size {
            x if x >= header_size => {
                source.rewind()?;
                cache.extend(Self::load_data(source)?);
            }
            0 => log::debug!("new storage"),
            _ => return Err(PersistencyError::UnknownFormat),
        }

        Ok(Self {
            cache: RwLock::new((cache, None)),
        })
    }

    fn load_data(
        mut source: impl Read + Seek,
    ) -> Result<impl IntoIterator<Item = (u64, Vec<u8>)>, PersistencyError> {
        let header: PersistencyHeader = bincode::deserialize_from(&mut source)?;

        if header.magic != BINARY_MAGIC {
            return Err(PersistencyError::UnknownFormat);
        }

        if header.version != BINARY_VERSION {
            return Err(PersistencyError::UnsupportedVersion(header.version));
        }

        if header.data_size > LEB_SIZE {
            log::warn!("internal persistency grew over the size of one logical erase block");
        }

        source.seek(io::SeekFrom::Start(header.data_offset.into()))?;
        let data: HashMap<u64, Vec<u8>> = bincode::deserialize_from(&mut source)?;
        Ok(data)
    }

    pub async fn get_watcher(&self) -> Receiver<Instant> {
        let (sender, watcher) = watch::channel(Instant::now());
        self.cache.write().await.1 = Some(sender);
        watcher
    }

    pub(super) async fn write(
        &self,
        mut source: impl Write + Seek,
    ) -> Result<(), PersistencyError> {
        let cache = self.cache.read().await;
        let data = bincode::serialize(&cache.deref().0)?;

        let mut header = PersistencyHeader::new()?;
        header.data_size = data.len() as u32;

        let header_bytes = bincode::serialize(&header)?;
        source.rewind()?;
        source.write_all(&header_bytes)?;
        source.write_all(&data)?;
        Ok(())
    }

    pub async fn get<T>(&self, key: &str) -> T
    where
        for<'a> T: Send + serde::Deserialize<'a>,
    {
        self.try_get(key).await.unwrap()
    }

    pub async fn try_get<T>(&self, key: &str) -> Result<T, PersistencyError>
    where
        for<'a> T: Send + serde::Deserialize<'a>,
    {
        self.cache
            .read()
            .await
            .0
            .get(&default_hash(key))
            .ok_or(PersistencyError::UnknownKey(key.into()))
            .and_then(|bytes| Ok(bincode::deserialize(bytes)?))
    }

    pub async fn set<T>(&self, key: &'static str, value: T)
    where
        T: Send + serde::Serialize,
    {
        self.try_set(key, value).await.unwrap()
    }

    pub async fn try_set<T>(&self, key: &str, value: T) -> Result<(), PersistencyError>
    where
        T: Send + serde::Serialize,
    {
        let encoded = bincode::serialize(&value)?;
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
        let mut cursor = Cursor::new(arr);
        let store = PersistencyStore::new(
            [("test", bincode::serialize(&123u128).unwrap())],
            &mut cursor,
        );
        assert!(matches!(store, Err(PersistencyError::UnknownFormat)));
    }

    #[test]
    fn test_invalid_header_magic() {
        let mut header = PersistencyHeader::new().unwrap();
        header.magic = 0xffaaffaa;
        let vec = bincode::serialize(&header).unwrap();
        let mut cursor = Cursor::new(vec);
        let store = PersistencyStore::new(
            [("test", bincode::serialize(&123u128).unwrap())],
            &mut cursor,
        );
        assert!(matches!(store, Err(PersistencyError::UnknownFormat)));
    }

    #[test]
    fn test_invalid_header_version() {
        let mut header = PersistencyHeader::new().unwrap();
        header.version = 3;
        let vec = bincode::serialize(&header).unwrap();
        let mut cursor = Cursor::new(vec);
        let store = PersistencyStore::new(
            [("test", bincode::serialize(&123u128).unwrap())],
            &mut cursor,
        );
        assert!(matches!(
            store,
            Err(PersistencyError::UnsupportedVersion(3))
        ));
    }

    #[test]
    fn test_invalid_data() {
        let header = PersistencyHeader::new().unwrap();
        let mut vec = bincode::serialize(&header).unwrap();
        let data = [0xffu8, 2];
        vec.extend_from_slice(&data);

        let mut cursor = Cursor::new(vec);
        let store = PersistencyStore::new(
            [("test", bincode::serialize(&123u128).unwrap())],
            &mut cursor,
        );

        assert!(matches!(
            store,
            Err(PersistencyError::SerializationError(_))
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
