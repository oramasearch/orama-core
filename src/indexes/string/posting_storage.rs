use std::sync::{atomic::AtomicU64, Arc};

use dashmap::DashMap;
use thiserror::Error;

use super::Posting;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct PostingListId(pub u64);

#[derive(Debug, Error)]
pub enum PostingStorageError {
    #[error("PostingListId not found")]
    PostingListIdNotFound,
    #[error("Serialize error")]
    SerializationError(#[from] bincode::Error),
}

const POSTING_STORAGE_TAG: u8 = 0;

pub struct PostingStorage {
    storage: DashMap<PostingListId, Vec<Posting>>,
    id_generator: Arc<AtomicU64>,
}

impl PostingStorage {
    pub fn new(id_generator: Arc<AtomicU64>) -> Self {
        PostingStorage {
            storage: Default::default(),
            id_generator,
        }
    }

    pub fn get(&self, posting_list_id: PostingListId) -> Result<Vec<Posting>, PostingStorageError> {
        self.storage
            .get(&posting_list_id)
            // TODO: avoid cloning here
            .map(|x| x.clone())
            .ok_or(PostingStorageError::PostingListIdNotFound)
    }

    pub fn generate_new_id(&self) -> PostingListId {
        let id = self
            .id_generator
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        PostingListId(id)
    }

    pub fn add_or_create(
        &self,
        posting_list_id: PostingListId,
        postings: Vec<Vec<Posting>>,
    ) -> Result<(), PostingStorageError> {
        let mut v = self.storage.entry(posting_list_id).or_default();
        v.extend(postings.into_iter().flatten());

        Ok(())
    }
}

// TODO: benchmark this and find a more performant way to serialize and deserialize
fn unserialize(input: &[u8]) -> Result<Vec<Posting>, bincode::Error> {
    bincode::deserialize(input)
}
fn serialize(vec: &Vec<Posting>) -> Result<Vec<u8>, bincode::Error> {
    bincode::serialize(vec)
}
