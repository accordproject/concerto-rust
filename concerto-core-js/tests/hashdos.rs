//! P5-110 (accordproject/concerto-rust#477): an instance whose keys are
//! crafted to collide under an unseeded hash stays about as cheap to build,
//! validate and serialize as one with ordinary keys of the same size.
//!
//! The keys of a `JsObject` come from user-supplied JSON, so it hashes with
//! a seeded hasher (PORTING.md 3.7). With FxHash, the keys below all have the
//! same hash, and the round trip is quadratic in their number.

// The key crafting below follows rustc-hash's 64-bit byte mixing.
#![cfg(target_pointer_width = "64")]

use std::hash::BuildHasher;
use std::time::{Duration, Instant};

use concerto_core::ModelManager;
use concerto_core::instance::InstanceEnv;
use concerto_core::json;
use concerto_core::json::{Map, Value};
use concerto_core_js::{JsValue, Serializer};

/// How many map entries each instance has.
const KEYS: usize = 4000;

/// The colliding instance may take this many times as long as the ordinary
/// one, plus [`SLACK`]. Under FxHash it takes well over ten times as long;
/// under a seeded hasher, about as long.
const RATIO: u32 = 4;
/// Absolute slack on top of [`RATIO`], so a slow or busy machine does not
/// fail the test on timer noise.
const SLACK: Duration = Duration::from_millis(30);

struct Env;

impl InstanceEnv for Env {
    fn new_id(&mut self) -> String {
        "id".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

/// `rustc_hash::FxHasher`'s byte mixing (rustc-hash 2, 64-bit targets): the
/// high and low halves of the 128-bit product, XORed.
fn multiply_mix(x: u64, y: u64) -> u64 {
    let full = u128::from(x).wrapping_mul(u128::from(y));
    (full as u64) ^ ((full >> 64) as u64)
}

fn le(bytes: &[u8]) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(word)
}

/// `count` distinct 48-byte printable ASCII keys. With `collide`, they all
/// have the same FxHash.
///
/// For a 48-byte string, rustc-hash 2 mixes the 16-byte chunks at 0 and 16
/// into `t1` and `t2`, then returns `multiply_mix(t1 ^ w, t2 ^ v) ^ 48`,
/// where `w` and `v` are the words at 32 and 40. `t1` depends only on the
/// first chunk, so a fixed first chunk whose `t1` is printable, repeated as
/// the word at 32, makes the first factor zero, and the hash 48, whatever
/// the other bytes are. Without `collide`, the word at 32 is not `t1`.
fn keys(count: usize, collide: bool) -> Vec<String> {
    const SEED1: u64 = 0x243f_6a88_85a3_08d3;
    const PREVENT_TRIVIAL_ZERO_COLLAPSE: u64 = 0xa409_3822_299f_31d0;
    let printable = |word: u64| word.to_le_bytes().iter().all(|b| (0x20..0x7f).contains(b));
    let (prefix, t1) = (0u64..)
        .map(|n| format!("hashdos-{n:08}"))
        .map(|prefix| {
            let bytes = prefix.as_bytes();
            let t1 = multiply_mix(
                SEED1 ^ le(&bytes[..8]),
                PREVENT_TRIVIAL_ZERO_COLLAPSE ^ le(&bytes[8..]),
            );
            (prefix, t1)
        })
        .find(|(_, t1)| printable(*t1))
        .expect("a printable t1 within u64 range");
    let cancel = if collide {
        String::from_utf8(t1.to_le_bytes().to_vec()).expect("printable ASCII")
    } else {
        "ordinary".to_string()
    };
    (0..count)
        .map(|i| format!("{prefix}{i:016x}{cancel}--------"))
        .collect()
}

/// A concept with one `Map<String, String>` field.
fn model() -> ModelManager {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.hashdos@1.0.0",
        "imports": [],
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Bag",
                "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.ObjectProperty",
                    "name": "entries",
                    "isArray": false,
                    "isOptional": false,
                    "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Entries" }
                }]
            },
            {
                "$class": "concerto.metamodel@1.0.0.MapDeclaration",
                "name": "Entries",
                "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" }
            }
        ]
    });
    let mut mm = ModelManager::new().expect("a model manager");
    mm.add_model_with_definitions(&ast, None, Some("hashdos.cto".into()))
        .expect("the model loads");
    mm
}

/// The fastest of three runs of the JSON -> JS object -> `fromJSON` (with
/// validation) -> `toJSON` (with validation) round trip over `keys`.
fn round_trip(mm: &ModelManager, keys: &[String]) -> Duration {
    let entries: Map<String, Value> = keys
        .iter()
        .map(|key| (key.clone(), Value::String("v".into())))
        .collect();
    let json = json!({ "$class": "org.hashdos@1.0.0.Bag", "entries": entries });
    let serializer = Serializer::new(true, true, None).expect("a serializer");
    (0..3)
        .map(|_| {
            let start = Instant::now();
            let object = JsValue::from_json(&json);
            let bag = serializer
                .from_json(mm, &object, None, &mut Env)
                .expect("the instance validates");
            let JsValue::Map(read) = bag.get("entries") else {
                panic!("entries is a Map");
            };
            assert_eq!(read.len(), keys.len());
            let written = serializer
                .to_json(mm, &JsValue::Instance(Box::new(bag)), None)
                .expect("the instance serializes");
            let JsValue::Object(written) = written else {
                panic!("toJSON returns an object");
            };
            let Some(JsValue::Object(entries)) = written.get("entries") else {
                panic!("toJSON writes the entries as an object");
            };
            assert_eq!(entries.len(), keys.len());
            start.elapsed()
        })
        .min()
        .expect("three runs")
}

#[test]
fn crafted_keys_collide_under_fxhash() {
    let fx = rustc_hash::FxBuildHasher;
    let colliding = keys(KEYS, true);
    let first = fx.hash_one(colliding[0].as_str());
    assert!(
        colliding
            .iter()
            .all(|key| fx.hash_one(key.as_str()) == first),
        "the crafted keys no longer collide under FxHash: update keys() for this rustc-hash"
    );
    let ordinary = keys(KEYS, false);
    let distinct: std::collections::HashSet<u64> = ordinary
        .iter()
        .map(|key| fx.hash_one(key.as_str()))
        .collect();
    assert_eq!(distinct.len(), KEYS, "the ordinary keys do not collide");
}

#[test]
fn colliding_keys_stay_roughly_linear() {
    let mm = model();
    let ordinary = keys(KEYS, false);
    let colliding = keys(KEYS, true);
    // Warm up allocations and the model's caches.
    round_trip(&mm, &ordinary[..100]);
    let ordinary_time = round_trip(&mm, &ordinary);
    let colliding_time = round_trip(&mm, &colliding);
    assert!(
        colliding_time <= ordinary_time * RATIO + SLACK,
        "{KEYS} colliding keys took {colliding_time:?}, {KEYS} ordinary keys {ordinary_time:?}"
    );
}
