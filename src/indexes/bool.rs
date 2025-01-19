use std::collections::HashSet;

use anyhow::Result;
use dashmap::DashMap;

use crate::{collection_manager::dto::FieldId, types::DocumentId};

#[derive(Debug, Default)]
struct BoolIndexPerField {
    true_docs: HashSet<DocumentId>,
    false_docs: HashSet<DocumentId>,
}

#[derive(Debug, Default)]
pub struct BoolIndex {
    maps: DashMap<FieldId, BoolIndexPerField>,
}

impl BoolIndex {
    pub fn new() -> Self {
        Self {
            maps: Default::default(),
        }
    }

    pub fn add(&self, doc_id: DocumentId, field_id: FieldId, value: bool) -> Result<()> {
        let mut btree = self.maps.entry(field_id).or_default();
        if value {
            btree.true_docs.insert(doc_id);
        } else {
            btree.false_docs.insert(doc_id);
        }

        Ok(())
    }

    pub fn filter(&self, field_id: FieldId, val: bool) -> Result<HashSet<DocumentId>> {
        let btree = match self.maps.get(&field_id) {
            Some(btree) => btree,
            // This should never happen: if the field is not in the index, it means that the field
            // was not indexed, and the filter should not have been created in the first place.
            None => return Ok(HashSet::new()),
        };

        if val {
            Ok(btree.true_docs.clone())
        } else {
            Ok(btree.false_docs.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use anyhow::Result;

    use crate::{collection_manager::dto::FieldId, types::DocumentId};

    use super::BoolIndex;

    #[test]
    fn test_bool_index_filter() -> Result<()> {
        let index = BoolIndex::new();

        index.add(DocumentId(0), FieldId(0), true)?;
        index.add(DocumentId(1), FieldId(0), false)?;
        index.add(DocumentId(2), FieldId(0), true)?;
        index.add(DocumentId(3), FieldId(0), false)?;
        index.add(DocumentId(4), FieldId(0), true)?;
        index.add(DocumentId(5), FieldId(0), false)?;

        let true_docs = index.filter(FieldId(0), true).unwrap();
        assert_eq!(
            true_docs,
            HashSet::from([DocumentId(0), DocumentId(2), DocumentId(4)])
        );

        let false_docs = index.filter(FieldId(0), false).unwrap();
        assert_eq!(
            false_docs,
            HashSet::from([DocumentId(1), DocumentId(3), DocumentId(5)])
        );

        Ok(())
    }

    #[test]
    fn test_bool_index_filter_unknown_field() -> Result<()> {
        let index = BoolIndex::new();

        index.add(DocumentId(0), FieldId(0), true)?;
        index.add(DocumentId(1), FieldId(0), false)?;
        index.add(DocumentId(2), FieldId(0), true)?;
        index.add(DocumentId(3), FieldId(0), false)?;
        index.add(DocumentId(4), FieldId(0), true)?;
        index.add(DocumentId(5), FieldId(0), false)?;

        let true_docs = index.filter(FieldId(1), true).unwrap();
        assert_eq!(true_docs, HashSet::from([]));

        let false_docs = index.filter(FieldId(1), false).unwrap();
        assert_eq!(false_docs, HashSet::from([]));

        Ok(())
    }
}
