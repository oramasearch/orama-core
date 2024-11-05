use std::{collections::HashMap, env, fs, path::Path};

fn main() {
    let stop_words: HashMap<String, String> = std::fs::read_dir("./src/stop_words")
        .unwrap()
        .filter_map(|dir| dir.ok())
        .filter_map(|dir| {
            println!("cargo::rerun-if-changed={:?}", dir.path());

            let filepath = dir.file_name().to_str()?.to_string();
            let (lang, _) = filepath.split_once(".").unwrap();
            let stop_words = std::fs::read_to_string(dir.path()).ok()?;

            Some((lang.to_owned(), stop_words))
        })
        .collect();

    let mut modules = "".to_string();
    let mut hash_map_insert = "".to_string();

    let out_dir = env::var_os("OUT_DIR").unwrap();

    if fs::metadata(Path::new(&out_dir).join("stop_words_gen")).is_ok() {
        fs::remove_dir_all(Path::new(&out_dir).join("stop_words_gen")).unwrap();
    }
    fs::create_dir(Path::new(&out_dir).join("stop_words_gen")).unwrap();
    for (lang, stop_words) in stop_words {
        let dest_path = Path::new(&out_dir).join(format!("stop_words_gen/{lang}.rs"));
        let stop_words = stop_words.lines();
        let stop_words = stop_words
            .map(|word| format!("\"{}\"", word))
            .collect::<Vec<String>>();
        let stop_words = format!(
            "pub const {}_STOP_WORDS: [&str; {}] = [\n{}\n];",
            lang.to_uppercase(),
            stop_words.len(),
            stop_words.join(",\n")
        );
        fs::write(&dest_path, stop_words).unwrap();

        modules.push_str(&format!("pub mod {};\n", lang));
        hash_map_insert.push_str(&format!("hash_map.insert(\"{}\".to_lowercase().parse().unwrap(), Some({lang}::{}_STOP_WORDS.into_iter().collect()));\n", lang.to_uppercase(), lang.to_uppercase()));
    }

    let dest_path = Path::new(&out_dir).join("stop_words_gen/mod.rs");
    fs::write(
        &dest_path,
        format!(
            r"
{modules}

use lazy_static::lazy_static;
use std::collections::HashSet;
use std::collections::HashMap;
use std::sync::RwLock;
use once_cell::sync::Lazy;
use std::sync::OnceLock;

pub type StopWords = HashSet<&'static str>;

static STOP_WORDS_CACHE: OnceLock<HashMap<Locale, Option<StopWords>>> = OnceLock::new();

pub fn load_stop_words() -> HashMap<Locale, Option<StopWords>> {{
    let mut hash_map = HashMap::new();
{hash_map_insert}
    hash_map
}}

"
        ),
    )
    .unwrap();

    println!("cargo::rerun-if-changed=build.rs");
}
