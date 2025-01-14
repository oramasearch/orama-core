use std::{fs, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use tempdir::TempDir;

use crate::{
    collection_manager::{
        dto::FieldId,
        sides::{
            CollectionWriteOperation, DocumentFieldIndexOperation, FieldIndexer, StringField,
            WriteOperation,
        },
    },
    indexes::string::{
        CommittedStringFieldIndex, StringIndex, StringIndexConfig, UncommittedStringFieldIndex,
    },
    nlp::TextParser,
    types::{CollectionId, Document, DocumentId},
};

pub fn generate_new_path() -> PathBuf {
    let tmp_dir = TempDir::new("test").expect("Cannot create temp dir");
    let dir = tmp_dir.path().to_path_buf();
    fs::create_dir_all(dir.clone()).expect("Cannot create dir");
    dir
}

pub async fn create_string_index(
    fields: Vec<(FieldId, String)>,
    documents: Vec<Document>,
) -> Result<StringIndex> {
    let index = StringIndex::new(StringIndexConfig {});

    let string_fields: Vec<_> = fields
        .into_iter()
        .map(|(field_id, field_name)| {
            (
                field_id,
                field_name,
                StringField::new(Arc::new(TextParser::from_locale(
                    crate::nlp::locales::Locale::EN,
                ))),
            )
        })
        .collect();

    let (sx, mut rx) = tokio::sync::broadcast::channel(1_0000);

    for (id, doc) in documents.into_iter().enumerate() {
        let document_id = DocumentId(id as u64);
        let flatten = doc.into_flatten();

        for (field_id, field_name, string_field) in &string_fields {
            string_field
                .get_write_operations(
                    CollectionId("collection".to_string()),
                    document_id,
                    field_name.clone(),
                    *field_id,
                    &flatten,
                    sx.clone(),
                )
                .await
                .unwrap()
        }
    }

    drop(sx);

    while let Ok(operation) = rx.recv().await {
        match operation {
            WriteOperation::Collection(
                _,
                CollectionWriteOperation::Index(
                    doc_id,
                    field_id,
                    DocumentFieldIndexOperation::IndexString {
                        field_length,
                        terms,
                    },
                ),
            ) => {
                index.insert(doc_id, field_id, field_length, terms)?;
            }
            _ => unreachable!(),
        };
    }

    Ok(index)
}

pub async fn create_uncommitted_string_field_index(
    documents: Vec<Document>,
) -> Result<UncommittedStringFieldIndex> {
    create_uncommitted_string_field_index_from(documents, 0).await
}

pub async fn create_uncommitted_string_field_index_from(
    documents: Vec<Document>,
    starting_doc_id: u64,
) -> Result<UncommittedStringFieldIndex> {
    let index = UncommittedStringFieldIndex::new();

    let string_field = StringField::new(Arc::new(TextParser::from_locale(
        crate::nlp::locales::Locale::EN,
    )));

    let (sx, mut rx) = tokio::sync::broadcast::channel(1_0000);

    for (id, doc) in documents.into_iter().enumerate() {
        let document_id = DocumentId(starting_doc_id + id as u64);
        let flatten = doc.into_flatten();
        string_field
            .get_write_operations(
                CollectionId("collection".to_string()),
                document_id,
                "field".to_string(),
                FieldId(1),
                &flatten,
                sx.clone(),
            )
            .await
            .with_context(|| {
                format!("Test get_write_operations {:?} {:?}", document_id, flatten)
            })?;
    }

    drop(sx);

    while let Ok(operation) = rx.recv().await {
        match operation {
            WriteOperation::Collection(
                _,
                CollectionWriteOperation::Index(
                    document_id,
                    _,
                    DocumentFieldIndexOperation::IndexString {
                        field_length,
                        terms,
                    },
                ),
            ) => {
                index
                    .insert(document_id, field_length, terms)
                    .with_context(|| {
                        format!("test cannot insert index_string {:?}", document_id)
                    })?;
            }
            _ => unreachable!(),
        };
    }

    Ok(index)
}

pub async fn create_committed_string_field_index(
    documents: Vec<Document>,
) -> Result<Option<CommittedStringFieldIndex>> {
    let index = create_string_index(vec![(FieldId(1), "field".to_string())], documents).await?;

    index.commit(generate_new_path())?;

    Ok(index.remove_committed_field(FieldId(1)))
}
