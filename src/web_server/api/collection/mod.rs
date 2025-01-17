use std::sync::Arc;

use axum::Router;

use crate::collection_manager::sides::{read::CollectionsReader, WriteSide};

mod admin;
mod answer;
mod search;

pub fn apis(write_side: Option<Arc<WriteSide>>, readers: Option<Arc<CollectionsReader>>) -> Router {
    let collection_router = Router::new();

    let collection_router = if let Some(write_side) = write_side {
        collection_router.merge(admin::apis(write_side))
    } else {
        collection_router
    };

    if let Some(readers) = readers {
        collection_router
            .merge(search::apis(readers.clone()))
            .merge(answer::apis(readers))
    } else {
        collection_router
    }
}
