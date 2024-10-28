use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
};

use anyhow::Result;
use dictionary::{Dictionary, TermId};
use ordered_float::NotNan;
use posting_storage::{PostingListId, PostingStorage};
use radix_trie::Trie;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use string_ultils::{tokenize, tokenize_and_stem, Language, Parser};

mod dictionary;
mod posting_storage;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub struct DocumentId(pub usize);
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldId(pub usize);

#[derive(Debug, Deserialize, Serialize)]
pub struct Posting {
    pub document_id: DocumentId,
    pub field_id: FieldId,
    pub positions: Vec<usize>,
    pub term_frequency: f32,
    pub doc_length: u16,
}

#[derive(Debug, Clone)]
pub struct StringIndexValue {
    posting_list_id: PostingListId,
    term_frequency: usize,
}

pub struct StringIndex {
    index: Trie<String, StringIndexValue>,
    posting_storage: PostingStorage,
    parser: Parser,
    dictionary: Dictionary,
    // TODO: move those to AtomicUsize
    total_documents: usize,
    total_document_length: usize,
}

impl StringIndex {
    pub fn new(base_path: String) -> Self {
        StringIndex {
            index: Trie::new(),
            posting_storage: PostingStorage::new(format!("{}/posting_storage", base_path)).unwrap(),
            parser: Parser::from_language(Language::English),
            dictionary: Dictionary::new(),
            total_documents: 0,
            total_document_length: 0,
        }
    }

    pub fn search(&self, term: &str, limit: usize, boost: f32) -> Result<Vec<(DocumentId, f32)>> {
        let total_documents = match self.total_documents {
            0 => {
                return {
                    println!("total_documents == 0");
                    Ok(vec![])
                }
            }
            total_documents => total_documents as f32,
        };
        let total_document_length = match self.total_document_length {
            0 => {
                println!("total_document_length == 0");
                return Ok(vec![]);
            }
            total_document_length => total_document_length as f32,
        };

        let avg_doc_length = total_document_length / total_documents;

        let tokens = tokenize(term, &self.parser.tokenizer);

        let mut posting_list_ids_with_freq = Vec::<StringIndexValue>::new();
        for token in tokens {
            let string_index_value = self.index.get(&token);

            if let Some(string_index_value) = string_index_value {
                posting_list_ids_with_freq.push(string_index_value.clone());
            } else {
                eprintln!("Token not found inside index: {}", token);
            }
        }

        let scores = posting_list_ids_with_freq
            .into_par_iter()
            .filter_map(|string_index_value| {
                self.posting_storage
                    .get(
                        string_index_value.posting_list_id,
                        // BAD: term_frequency is not used inside posting_storage
                        // But we need after, so here forward it.
                        // TODO: We need to find a way to avoid this.
                        string_index_value.term_frequency,
                    )
                    .ok()
            })
            // Every thread perform on a separated hashmap
            .fold(
                HashMap::<DocumentId, f32>::new,
                |mut acc, (postings, freq)| {
                    let freq = freq as f32;
                    for posting in postings {
                        let term_frequency = posting.term_frequency;
                        let doc_length = posting.doc_length as f32;

                        let idf = ((total_documents - freq + 0.5_f32) / (freq + 0.5_f32)).ln_1p();
                        let score =
                            calculate_score(term_frequency, idf, doc_length, avg_doc_length, boost);

                        let doc_score = acc.entry(posting.document_id).or_default();
                        *doc_score += score;
                    }
                    acc
                },
            )
            // And later we merge all the hashmaps
            .reduce(HashMap::<DocumentId, f32>::new, |mut acc, item| {
                for (document_id, score) in item {
                    let doc_score = acc.entry(document_id).or_default();
                    *doc_score += score;
                }
                acc
            });

        let docs = top_n(scores, limit);

        Ok(docs)
    }

    pub fn insert_multiple(
        &mut self,
        field_id: FieldId,
        data: Vec<(DocumentId, String)>,
    ) -> Result<()> {
        self.total_documents += data.len();

        let dictionary = &self.dictionary;
        let parser = &self.parser;

        let t = data
            .into_par_iter()
            // Parallel
            .fold(
                HashMap::<TermId, Vec<Posting>>::new,
                |mut acc, (document_id, s)| {
                    let mut term_freqs: HashMap<String, HashMap<FieldId, Vec<usize>>> =
                        HashMap::new();

                    for (position, (original, stemmed)) in tokenize_and_stem(&s, parser).enumerate()
                    {
                        let is_equal = original == stemmed;

                        let entry = term_freqs.entry(original).or_default();
                        let field_entry = entry.entry(field_id).or_default();
                        field_entry.push(position);

                        if !is_equal {
                            let entry = term_freqs.entry(stemmed).or_default();
                            let field_entry = entry.entry(field_id).or_default();
                            field_entry.push(position);
                        }
                    }

                    let doc_length = term_freqs
                        .values()
                        .map(|field_freqs| {
                            field_freqs
                                .values()
                                .map(|positions| positions.len())
                                .sum::<usize>()
                        })
                        .sum::<usize>();

                    for (term, field_positions) in term_freqs {
                        let term_id = dictionary.get_or_add(&term);
                        // println!("Term: {} -> {}", term, term_id.0);

                        let v = acc.entry(term_id).or_default();

                        let posting = field_positions.into_iter().map(|(field_id, positions)| {
                            let term_frequency = positions.len() as f32;

                            Posting {
                                document_id,
                                field_id,
                                positions,
                                // original_term: term.clone(),
                                term_frequency,
                                doc_length: doc_length as u16,
                            }
                        });
                        v.extend(posting);
                    }

                    acc
                },
            );

        let posting_per_term = t.reduce(
            HashMap::<TermId, Vec<Posting>>::new,
            // Merge the hashmap
            |mut acc, item| {
                for (term_id, postings) in item {
                    let vec = acc.entry(term_id).or_default();
                    vec.extend(postings.into_iter());
                }
                acc
            },
        );

        let mut postings_per_posting_list_id: HashMap<PostingListId, Vec<Vec<Posting>>> =
            HashMap::with_capacity(posting_per_term.len());
        // NB: We cannot parallelize the tree insertion yet :(
        // We could move the tree into a custom implementation to support parallelism
        // Once we resolve this issue, we StringIndex is thread safe!
        // TODO: move to custom implementation
        // For the time being, we can just use the sync tree
        for (term_id, postings) in posting_per_term {
            self.total_document_length += postings.iter().map(|p| p.positions.len()).sum::<usize>();
            let number_of_occurence_of_term = postings.len();

            // Due to this implementation, we have a limitation
            // because we "forgot" the term. Here we have just the term_id
            // This invocation shouldn't exist at all:
            // we have the term on the top of this function
            // TODO: find a way to avoid this invocation
            let term = dictionary.retrive(term_id);

            let value = self.index.get_mut(&term);
            if let Some(value) = value {
                value.term_frequency += number_of_occurence_of_term;
                let vec = postings_per_posting_list_id
                    .entry(value.posting_list_id)
                    .or_default();
                vec.push(postings);
            } else {
                let posting_list_id = self.posting_storage.generate_new_id();
                self.index.insert(
                    term,
                    StringIndexValue {
                        posting_list_id,
                        term_frequency: number_of_occurence_of_term,
                    },
                );

                let vec = postings_per_posting_list_id
                    .entry(posting_list_id)
                    .or_default();
                vec.push(postings);
            }
        }

        postings_per_posting_list_id
            .into_par_iter()
            .map(|(k, v)| self.posting_storage.add_or_create(k, v))
            // TODO: handle error
            .all(|_| true);

        Ok(())
    }
}

fn calculate_score(tf: f32, idf: f32, doc_length: f32, avg_doc_length: f32, boost: f32) -> f32 {
    let k1 = 1.5;
    let b = 0.75;
    let numerator = tf * (k1 + 1.0);
    let denominator = tf + k1 * (1.0 - b + b * (doc_length / avg_doc_length));
    idf * (numerator / denominator) * boost
}

fn top_n(map: HashMap<DocumentId, f32>, n: usize) -> Vec<(DocumentId, f32)> {
    // A min-heap of size `n` to keep track of the top N elements
    let mut heap: BinaryHeap<Reverse<(NotNan<f32>, DocumentId)>> = BinaryHeap::with_capacity(n);

    for (key, value) in map {
        // Insert into the heap if it's not full, or replace the smallest element if the current one is larger
        if heap.len() < n {
            heap.push(Reverse((NotNan::new(value).unwrap(), key)));
        } else if let Some(Reverse((min_value, _))) = heap.peek() {
            if value > *min_value.as_ref() {
                heap.pop();
                heap.push(Reverse((NotNan::new(value).unwrap(), key)));
            }
        }
    }

    // Collect results into a sorted Vec (optional sorting based on descending values)
    let mut result: Vec<(DocumentId, f32)> = heap
        .into_sorted_vec()
        .into_iter()
        .map(|Reverse((value, key))| (key, value.into_inner()))
        .collect();

    // TODO: check is this `reverse` is needed
    result.reverse();
    result
}

#[cfg(test)]
mod tests {
    use crate::{DocumentId, FieldId, StringIndex};

    #[test]
    fn test_foo() {
        let batch = vec![
            (
                DocumentId(1),
                "Yo, I'm from where Nicky Barnes got rich as fuck, welcome!".to_string(),
            ),
            (
                DocumentId(2),
                "Welcome to Harlem, where you welcome to problems".to_string(),
            ),
            (
                DocumentId(3),
                "Now bitches, they want to neuter me, niggas, they want to tutor me".to_string(),
            ),
        ];

        let mut string_index = StringIndex::new(".".to_owned());
        string_index.insert_multiple(FieldId(0), batch).unwrap();

        let output = string_index.search("welcome", 10, 1.0).unwrap();

        assert_eq!(output.len(), 2);
    }
}
