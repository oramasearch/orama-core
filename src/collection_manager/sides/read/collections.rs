use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    sync::Arc,
};

use crate::{
    ai::AIService,
    collection_manager::sides::Offset,
    file_utils::{create_if_not_exists, create_if_not_exists_async, create_or_overwrite, BufferedFile},
    nlp::NLPService,
    offset_storage::OffsetStorage,
    types::CollectionId,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, RwLockReadGuard};
use tracing::{info, instrument, warn};

use super::{collection::CollectionReader, IndexesConfig};

#[derive(Debug)]
pub struct CollectionsReader {
    ai_service: Arc<AIService>,
    nlp_service: Arc<NLPService>,
    collections: RwLock<HashMap<CollectionId, CollectionReader>>,
    indexes_config: IndexesConfig,

    offset_storage: OffsetStorage,
}
impl CollectionsReader {
    pub fn try_new(
        ai_service: Arc<AIService>,
        nlp_service: Arc<NLPService>,
        indexes_config: IndexesConfig,
    ) -> Result<Self> {
        Ok(Self {
            ai_service,
            nlp_service,

            collections: Default::default(),
            indexes_config,

            offset_storage: OffsetStorage::new(),
        })
    }

    pub fn get_ai_service(&self) -> Arc<AIService> {
        self.ai_service.clone()
    }

    pub async fn get_collection<'s, 'coll>(
        &'s self,
        id: CollectionId,
    ) -> Option<CollectionReadLock<'coll>>
    where
        's: 'coll,
    {
        let r = self.collections.read().await;
        CollectionReadLock::try_new(r, id)
    }

    #[instrument(skip(self))]
    pub async fn load(&mut self) -> Result<()> {
        let data_dir = &self.indexes_config.data_dir;
        info!("Loading collections from disk '{:?}'.", data_dir);

        create_if_not_exists(data_dir).context("Cannot create data directory")?;

        let collections_info: CollectionsInfo = match BufferedFile::open(data_dir.join("info.json"))
            .and_then(|f| f.read_json_data())
            .context("Cannot deserialize info.json file")
        {
            Ok(info) => info,
            Err(e) => {
                warn!(
                    "Cannot read info.json file: {:?}. Skip loading collections",
                    e
                );
                return Ok(());
            }
        };

        let CollectionsInfo::V1(collections_info) = collections_info;

        let base_dir_for_collections = data_dir.join("collections");

        for collection_id in collections_info.collection_ids {
            let collection_dir = base_dir_for_collections.join(&collection_id.0);
            info!("Loading collection {:?}", collection_dir);

            let mut collection = CollectionReader::try_new(
                collection_id.clone(),
                self.ai_service.clone(),
                self.nlp_service.clone(),
                self.indexes_config.clone(),
            )?;

            collection
                .load(base_dir_for_collections.join(&collection.get_id().0))
                .await
                .with_context(|| format!("Cannot load {:?} collection", collection_id))?;

            let mut guard = self.collections.write().await;
            guard.insert(collection_id, collection);
        }

        info!("Collections loaded from disk.");

        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn commit(&self) -> Result<()> {
        let data_dir = &self.indexes_config.data_dir;

        create_if_not_exists_async(data_dir).await.context("Cannot create data directory")?;

        let col = self.collections.read().await;
        let col = &*col;

        let collections_dir = data_dir.join("collections");
        create_if_not_exists_async(&collections_dir).await.context("Cannot create 'collections' directory")?;

        let collection_ids: Vec<_> = col.keys().cloned().collect();

        for (id, reader) in col {
            info!("Committing collection {:?}", id);

            let collection_dir = collections_dir.join(&id.0);

            create_if_not_exists_async(&collection_dir)
                .await
                .with_context(|| format!("Cannot create directory for collection '{}'", id.0))?;

            reader.commit(collection_dir).await?;

            info!("Collection {:?} committed", id);
        }

        let collections_info = CollectionsInfo::V1(CollectionsInfoV1 {
            collection_ids: collection_ids.into_iter().collect(),
        });

        create_or_overwrite(data_dir.join("info.json"), &collections_info)
            .await
            .context("Cannot create info.json file")?;

        Ok(())
    }

    pub async fn create_collection(&self, offset: Offset, id: CollectionId) -> Result<()> {
        info!("Creating collection {:?}", id);

        let collection = CollectionReader::try_new(
            id.clone(),
            self.ai_service.clone(),
            self.nlp_service.clone(),
            self.indexes_config.clone(),
        )?;

        let mut guard = self.collections.write().await;
        guard.insert(id, collection);

        self.offset_storage.set_offset(offset);

        Ok(())
    }
}

pub struct CollectionReadLock<'guard> {
    lock: RwLockReadGuard<'guard, HashMap<CollectionId, CollectionReader>>,
    id: CollectionId,
}

impl<'guard> CollectionReadLock<'guard> {
    pub fn try_new(
        lock: RwLockReadGuard<'guard, HashMap<CollectionId, CollectionReader>>,
        id: CollectionId,
    ) -> Option<Self> {
        let guard = lock.get(&id);
        match &guard {
            Some(_) => {
                let _ = guard;
                Some(CollectionReadLock { lock, id })
            }
            None => None,
        }
    }
}

impl Deref for CollectionReadLock<'_> {
    type Target = CollectionReader;

    fn deref(&self) -> &Self::Target {
        // safety: the collection contains the id because we checked it before
        // no one can remove the collection from the map because we hold a read lock
        self.lock.get(&self.id).unwrap()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "version")]
enum CollectionsInfo {
    #[serde(rename = "1")]
    V1(CollectionsInfoV1),
}
#[derive(Deserialize, Serialize)]
struct CollectionsInfoV1 {
    collection_ids: HashSet<CollectionId>,
}
