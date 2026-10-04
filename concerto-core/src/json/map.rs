//! [`Map`]: `serde_json::Map` with its `preserve_order` semantics (an
//! `IndexMap`, `remove` being `swap_remove`), hashed with [`SeededState`].

use std::borrow::Borrow;
use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::mem;
use std::ops;

use indexmap::IndexMap;
use serde::de;

use super::Value;
use crate::hash::SeededState;

/// A JSON object: its entries in insertion order, keyed by strings hashed
/// with the process-wide secret keys ([`SeededState`]).
pub struct Map<K, V> {
    map: IndexMap<K, V, SeededState>,
}

/// An entry of a [`Map`] ([`Map::entry`]).
pub type Entry<'a> = indexmap::map::Entry<'a, String, Value>;
/// An iterator over a [`Map`]'s entries.
pub type Iter<'a> = indexmap::map::Iter<'a, String, Value>;
/// A mutable iterator over a [`Map`]'s entries.
pub type IterMut<'a> = indexmap::map::IterMut<'a, String, Value>;
/// An owning iterator over a [`Map`]'s entries.
pub type IntoIter = indexmap::map::IntoIter<String, Value>;
/// An iterator over a [`Map`]'s keys.
pub type Keys<'a> = indexmap::map::Keys<'a, String, Value>;
/// An iterator over a [`Map`]'s values.
pub type Values<'a> = indexmap::map::Values<'a, String, Value>;
/// A mutable iterator over a [`Map`]'s values.
pub type ValuesMut<'a> = indexmap::map::ValuesMut<'a, String, Value>;
/// An owning iterator over a [`Map`]'s values.
pub type IntoValues = indexmap::map::IntoValues<String, Value>;

impl Map<String, Value> {
    /// An empty map.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Map {
            map: IndexMap::with_hasher(SeededState::default()),
        }
    }

    /// An empty map with room for `capacity` entries.
    #[inline]
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Map {
            map: IndexMap::with_capacity_and_hasher(capacity, SeededState::default()),
        }
    }

    /// Removes every entry.
    #[inline]
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// The value of `key`.
    #[inline]
    pub fn get<Q>(&self, key: &Q) -> Option<&Value>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.map.get(key)
    }

    /// Whether the map has `key`.
    #[inline]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.map.contains_key(key)
    }

    /// The value of `key`, mutably.
    #[inline]
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut Value>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.map.get_mut(key)
    }

    /// The key and value of `key`.
    #[inline]
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&String, &Value)>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.map.get_key_value(key)
    }

    /// Sets `k` to `v`, returning the value it replaced. A new key goes
    /// last; an existing one keeps its place.
    #[inline]
    pub fn insert(&mut self, k: String, v: Value) -> Option<Value> {
        self.map.insert(k, v)
    }

    /// Sets `k` to `v` at `index`, moving it there when it is present.
    #[inline]
    pub fn shift_insert(&mut self, index: usize, k: String, v: Value) -> Option<Value> {
        self.map.shift_insert(index, k, v)
    }

    /// Removes `key`, as [`Map::swap_remove`] does (`serde_json`'s
    /// `preserve_order` semantics).
    #[inline]
    pub fn remove<Q>(&mut self, key: &Q) -> Option<Value>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.swap_remove(key)
    }

    /// Removes `key` and returns its entry, as [`Map::swap_remove_entry`]
    /// does.
    #[inline]
    pub fn remove_entry<Q>(&mut self, key: &Q) -> Option<(String, Value)>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.swap_remove_entry(key)
    }

    /// Removes `key`, moving the last entry into its place.
    #[inline]
    pub fn swap_remove<Q>(&mut self, key: &Q) -> Option<Value>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.map.swap_remove(key)
    }

    /// Removes `key` and returns its entry, moving the last entry into its
    /// place.
    #[inline]
    pub fn swap_remove_entry<Q>(&mut self, key: &Q) -> Option<(String, Value)>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.map.swap_remove_entry(key)
    }

    /// Removes `key`, keeping the order of the other entries.
    #[inline]
    pub fn shift_remove<Q>(&mut self, key: &Q) -> Option<Value>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.map.shift_remove(key)
    }

    /// Removes `key` and returns its entry, keeping the order of the other
    /// entries.
    #[inline]
    pub fn shift_remove_entry<Q>(&mut self, key: &Q) -> Option<(String, Value)>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord + Eq + Hash,
    {
        self.map.shift_remove_entry(key)
    }

    /// Moves every entry of `other` into this map, leaving `other` empty.
    #[inline]
    pub fn append(&mut self, other: &mut Self) {
        self.map.extend(mem::take(&mut other.map));
    }

    /// The entry of `key`, for in-place manipulation.
    pub fn entry<S>(&mut self, key: S) -> Entry<'_>
    where
        S: Into<String>,
    {
        self.map.entry(key.into())
    }

    /// The number of entries.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the map has no entries.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The entries, in order.
    #[inline]
    pub fn iter(&self) -> Iter<'_> {
        self.map.iter()
    }

    /// The entries, in order, with mutable values.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_> {
        self.map.iter_mut()
    }

    /// The keys, in order.
    #[inline]
    pub fn keys(&self) -> Keys<'_> {
        self.map.keys()
    }

    /// The values, in order.
    #[inline]
    pub fn values(&self) -> Values<'_> {
        self.map.values()
    }

    /// The values, in order, mutably.
    #[inline]
    pub fn values_mut(&mut self) -> ValuesMut<'_> {
        self.map.values_mut()
    }

    /// The values, in order, by value.
    #[inline]
    pub fn into_values(self) -> IntoValues {
        self.map.into_values()
    }

    /// Keeps only the entries `f` accepts, in order.
    #[inline]
    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&String, &mut Value) -> bool,
    {
        self.map.retain(f);
    }

    /// Sorts the entries by key.
    #[inline]
    pub fn sort_keys(&mut self) {
        self.map.sort_unstable_keys();
    }

    /// The hasher of the keys.
    #[cfg(test)]
    pub(crate) fn hasher(&self) -> &SeededState {
        self.map.hasher()
    }
}

impl Default for Map<String, Value> {
    #[inline]
    fn default() -> Self {
        Map::new()
    }
}

impl Clone for Map<String, Value> {
    #[inline]
    fn clone(&self) -> Self {
        Map {
            map: self.map.clone(),
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.map.clone_from(&source.map);
    }
}

impl PartialEq for Map<String, Value> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.map.eq(&other.map)
    }
}

impl Eq for Map<String, Value> {}

impl Hash for Map<String, Value> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut kv = Vec::from_iter(&self.map);
        kv.sort_unstable_by(|a, b| a.0.cmp(b.0));
        kv.hash(state);
    }
}

impl<Q> ops::Index<&Q> for Map<String, Value>
where
    String: Borrow<Q>,
    Q: ?Sized + Ord + Eq + Hash,
{
    type Output = Value;

    fn index(&self, index: &Q) -> &Value {
        self.map.index(index)
    }
}

impl<Q> ops::IndexMut<&Q> for Map<String, Value>
where
    String: Borrow<Q>,
    Q: ?Sized + Ord + Eq + Hash,
{
    #[expect(clippy::expect_used, reason = "serde_json's IndexMut panics alike")]
    fn index_mut(&mut self, index: &Q) -> &mut Value {
        self.map.get_mut(index).expect("no entry found for key")
    }
}

impl Debug for Map<String, Value> {
    #[inline]
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        self.map.fmt(formatter)
    }
}

impl serde::ser::Serialize for Map<String, Value> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (k, v) in self {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> de::Deserialize<'de> for Map<String, Value> {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Map<String, Value>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map")
            }

            #[inline]
            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Map::new())
            }

            #[inline]
            fn visit_map<V>(self, mut visitor: V) -> Result<Self::Value, V::Error>
            where
                V: de::MapAccess<'de>,
            {
                let mut values = Map::new();
                while let Some((key, value)) = visitor.next_entry()? {
                    values.insert(key, value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

impl FromIterator<(String, Value)> for Map<String, Value> {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = (String, Value)>,
    {
        let mut map = Map::new();
        map.map.extend(iter);
        map
    }
}

impl Extend<(String, Value)> for Map<String, Value> {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = (String, Value)>,
    {
        self.map.extend(iter);
    }
}

impl<'de> de::IntoDeserializer<'de, serde_json::Error> for Map<String, Value> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

impl<'de> de::IntoDeserializer<'de, serde_json::Error> for &'de Map<String, Value> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

impl<'a> IntoIterator for &'a Map<String, Value> {
    type Item = (&'a String, &'a Value);
    type IntoIter = Iter<'a>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

impl<'a> IntoIterator for &'a mut Map<String, Value> {
    type Item = (&'a String, &'a mut Value);
    type IntoIter = IterMut<'a>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.map.iter_mut()
    }
}

impl IntoIterator for Map<String, Value> {
    type Item = (String, Value);
    type IntoIter = IntoIter;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.map.into_iter()
    }
}
