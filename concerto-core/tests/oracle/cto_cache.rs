//! Reads the CTO -> AST cache that task P1-07a's
//! `migration/oracle/bin/build-cto-cache.js` writes (PORTING.md OD-9;
//! `migration/oracle/README.md`, "CTO -> AST cache for the native harness").
//!
//! CTO parsing stays in JS (`concerto-cto`), so this harness never parses
//! CTO text. It looks each text up in the cache instead, exactly as the
//! README documents the format:
//!
//! - **Key**: the SHA-256 (lower-case hex) of
//!   `JSON.stringify([cto, fileName, skipLocationNodes])`, where `fileName`
//!   is the call's file-name argument when it is a string and `null`
//!   otherwise, and `skipLocationNodes` is the owning `ModelManager`'s
//!   constructor option when it is present and `null` otherwise
//!   (`keyFor` and `record` in `build-cto-cache.js`).
//! - **Location**: `cto-cache/<first two hex chars>/<key>.json`, next to
//!   `fixtures/` under `migration/oracle/` (or `$CONCERTO_CTO_CACHE`).
//! - **Entry**: `{"ast": <AST>}` for a successful parse, or
//!   `{"error": {"class", "message", "location"}}` for a recorded
//!   `ParseException`.
//!
//! A text with no entry is a harness error, never a skip or a pass
//! (plan §2.6, README).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// One cache entry.
pub enum CacheEntry {
    /// The parser's AST.
    Ast(Value),
    /// The `ParseException` the frozen parser raised: `{class, message,
    /// location}` exactly as the cache stores it.
    Error(Value),
}

/// The cache directory, `cto-cache/`.
pub struct CtoCache {
    dir: PathBuf,
}

impl CtoCache {
    /// The cache for a corpus: `$CONCERTO_CTO_CACHE` if set, otherwise the
    /// `cto-cache` directory that `build-cto-cache.js` writes next to
    /// `fixtures_dir` (both default to `migration/oracle/`). `None` when that
    /// directory does not exist; a lookup then fails as a harness error.
    pub fn locate(fixtures_dir: &Path) -> Option<Self> {
        let dir = match std::env::var("CONCERTO_CTO_CACHE") {
            Ok(configured) => PathBuf::from(configured),
            Err(_) => fixtures_dir.parent()?.join("cto-cache"),
        };
        dir.is_dir().then_some(Self { dir })
    }

    /// A cache rooted at `dir`, for the harness's own tests.
    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The key `build-cto-cache.js` computes for one parse.
    pub fn key(cto: &str, file_name: Option<&str>, skip_location_nodes: &Value) -> String {
        // `serde_json`'s compact writer escapes strings the way
        // `JSON.stringify` does for every string a Rust `&str` can hold:
        // `"`, `\` and the C0 controls (`\b \f \n \r \t`, otherwise
        // lower-case `\u00XX`), and nothing else.
        let text = serde_json::to_string(&json!([cto, file_name, skip_location_nodes]))
            .expect("a JSON array of strings, null and a JSON value always serialises");
        let digest = Sha256::digest(text.as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Looks one parse up. `Err` is a harness error.
    pub fn lookup(
        &self,
        cto: &str,
        file_name: Option<&str>,
        skip_location_nodes: &Value,
    ) -> Result<CacheEntry, String> {
        let key = Self::key(cto, file_name, skip_location_nodes);
        let path = self.dir.join(&key[..2]).join(format!("{key}.json"));
        let text = fs::read_to_string(&path).map_err(|e| {
            format!(
                "CTO cache has no entry {key} (file name {file_name:?}, skipLocationNodes \
                 {skip_location_nodes}): {e}; rebuild it with \
                 `node migration/oracle/bin/build-cto-cache.js`"
            )
        })?;
        let mut entry: Value = serde_json::from_str(&text)
            .map_err(|e| format!("CTO cache entry {} is malformed: {e}", path.display()))?;
        if let Some(ast) = entry.get_mut("ast") {
            return Ok(CacheEntry::Ast(ast.take()));
        }
        if let Some(error) = entry.get_mut("error")
            && error.is_object()
        {
            return Ok(CacheEntry::Error(error.take()));
        }
        Err(format!(
            "CTO cache entry {} has neither `ast` nor `error`",
            path.display()
        ))
    }
}
