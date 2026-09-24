//! Fixture schema for the behavioural oracle (task P0-05, plan §2.2), as
//! `accordproject/concerto`'s `migration/oracle/README.md` documents it, and
//! loading of the corpus under `migration/oracle/fixtures/**`.
//!
//! Only the parts of the schema this harness (task P1-07) actually decodes
//! are modelled precisely; `inputs.args`/`inputs.target` stay as raw
//! [`serde_json::Value`] and are decoded op by op (see `decode.rs`).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// One recorded call, replayed against the Rust engine.
#[derive(Debug, Deserialize)]
pub struct Fixture {
    /// Content hash id, unique within the corpus.
    #[allow(dead_code)]
    pub id: String,
    /// Which of `unit`/`data`/`conformance`/`gaps`/`lifted` recorded it.
    pub source: String,
    /// The mocha test title that produced it, when the source has one.
    #[serde(default)]
    #[allow(dead_code)]
    pub source_test: Option<String>,
    /// `<Class>.<method>`, `<Class>.new` or `<Holder>.<staticFn>`.
    pub op: String,
    pub inputs: Inputs,
    pub outcome: Outcome,
    #[serde(default)]
    pub env: Env,
    /// How many raw records this fixture deduplicates.
    #[serde(default)]
    #[allow(dead_code)]
    pub occurrences: u64,
    /// Set by the loader, not part of the JSON: where the fixture came from,
    /// for the per-fixture report.
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
pub struct Inputs {
    /// Present for method ops only. Not read yet: the ops this harness runs
    /// (`ops.rs`) are all statics with no receiver; a method op family adds
    /// its use of this field alongside its own decoding support.
    #[serde(default)]
    #[allow(dead_code)]
    pub target: Option<Value>,
    #[serde(default)]
    pub args: Vec<Value>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Env {
    /// True when the op drew from `Math.random` (README "Fixture schema").
    /// The harness does not yet reproduce the JS seeded PRNG (that lands
    /// with the Factory/Serializer op families a later task adds), so these
    /// fixtures are reported `unsupported` rather than compared value for
    /// value.
    #[serde(default)]
    pub random: bool,
}

/// `outcome`: either `{"ok": <value>, "effects"?: <value>}` or
/// `{"error": {...}}`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Outcome {
    Ok {
        ok: Value,
        #[serde(default)]
        #[allow(dead_code)]
        effects: Option<Value>,
    },
    Err {
        error: ErrorOutcome,
    },
}

/// `outcome.error`: `{class, message, location, component}` (README
/// "Fixture schema"), the shape `ContractError` in `concerto-core`'s
/// `src/error` module also uses (PORTING.md section 2.1).
#[derive(Debug, Deserialize)]
pub struct ErrorOutcome {
    pub class: String,
    pub message: String,
    #[serde(default)]
    pub location: Option<Value>,
    #[serde(default)]
    pub component: Option<String>,
}

/// A fixture file that could not be loaded (malformed JSON, a missing blob,
/// or a shape this harness's [`Fixture`] does not model). README: "A
/// missing, corrupt or unreadable blob is a harness error" — the same verdict
/// applies here, and every such fixture is counted, never silently dropped.
#[derive(Debug)]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

/// Walks `root` (a `migration/oracle/fixtures` directory) for every fixture
/// JSON file, resolving `{"@@oracle":"blob","sha256":...}` references
/// against `root/blobs/**` (README "Large values ..."), and parses each one.
///
/// `root/manifest.json` and everything under `root/blobs/` are skipped: they
/// are corpus metadata and blob storage, not fixtures themselves.
pub fn load_all(root: &Path) -> (Vec<Fixture>, Vec<LoadError>) {
    let mut fixtures = Vec::new();
    let mut errors = Vec::new();
    walk(root, root, &mut fixtures, &mut errors);
    (fixtures, errors)
}

fn walk(dir: &Path, root: &Path, fixtures: &mut Vec<Fixture>, errors: &mut Vec<LoadError>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    // Deterministic order, so two runs report fixtures in the same order.
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("blobs") {
                continue;
            }
            walk(&path, root, fixtures, errors);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
            continue;
        }
        match load_one(&path, root) {
            Ok(fixture) => fixtures.push(fixture),
            Err(message) => errors.push(LoadError {
                path: path.clone(),
                message,
            }),
        }
    }
}

fn load_one(path: &Path, root: &Path) -> Result<Fixture, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let mut value: Value = serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))?;
    resolve_blobs(&mut value, root)?;
    let mut fixture: Fixture = serde_json::from_value(value).map_err(|e| format!("schema: {e}"))?;
    fixture.path = path.to_path_buf();
    Ok(fixture)
}

/// Replaces every `{"@@oracle":"blob","sha256":"<hash>"}` in `value`,
/// anywhere in the tree, with the parsed contents of
/// `root/blobs/<hash[..2]>/<hash>.json` (README "Large values over 1024
/// characters ..."). A blob can itself expand to a value containing further
/// blob references (nested large values), so the result is resolved again.
fn resolve_blobs(value: &mut Value, root: &Path) -> Result<(), String> {
    if let Value::Object(map) = value
        && let Some(Value::String(kind)) = map.get("@@oracle")
        && kind == "blob"
    {
        let sha = map
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| "blob reference missing sha256".to_string())?
            .to_string();
        let prefix = &sha[..sha.len().min(2)];
        let blob_path = root.join("blobs").join(prefix).join(format!("{sha}.json"));
        let text = fs::read_to_string(&blob_path)
            .map_err(|e| format!("blob {} unreadable: {e}", blob_path.display()))?;
        *value = serde_json::from_str(&text)
            .map_err(|e| format!("blob {} malformed: {e}", blob_path.display()))?;
        return resolve_blobs(value, root);
    }
    match value {
        Value::Object(map) => {
            for v in map.values_mut() {
                resolve_blobs(v, root)?;
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                resolve_blobs(v, root)?;
            }
        }
        _ => {}
    }
    Ok(())
}
