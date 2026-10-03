//! [`Name`]: the `name` of a metamodel node, which borrows its text from
//! the JSON text the node was read from, where it can (P5-93,
//! accordproject/concerto-rust#443).

use std::borrow::{Borrow, Cow};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::{Arc, LazyLock};

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The `name` of a node of the `concerto.metamodel@1.0.0` namespace (but
/// an import's): a declaration's, a property's, a decorator's, a type
/// reference's or an identifier's.
///
/// It reads as a `str` ([`Deref`], [`Name::as_str`]) and compares, hashes
/// and orders as its text does. A name read from JSON text that is being
/// read inside [`with_source`] shares that text instead of copying it: it
/// is the text (an `Arc<str>`, which the reader keeps anyway) and where
/// the name is in it, so reading one allocates nothing, and cloning one
/// only counts a reference. Any other name holds its own copy. A name
/// that shares its source text keeps all of that text alive for as long
/// as the name (or a clone of it) lives.
#[derive(Clone)]
pub struct Name {
    /// The text the name is in.
    text: Arc<str>,
    /// Where the name starts in `text`, or [`WHOLE`] when it is all of
    /// `text`.
    start: u32,
    /// The name's length in bytes (unused for [`WHOLE`]).
    len: u32,
}

/// [`Name::start`] for a name that is all of its text.
const WHOLE: u32 = u32::MAX;

thread_local! {
    /// The text [`with_source`] reads names from, if any.
    static SOURCE: RefCell<Option<Arc<str>>> = const { RefCell::new(None) };
}

/// The empty name's text, shared.
static EMPTY: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from(""));

/// Runs `read`, which reads metamodel nodes from `source`, so that a name
/// read as a slice of `source` (a JSON string without escapes, which
/// `serde_json` hands over borrowed) shares `source` instead of copying
/// it. Restores the source it replaces, if any, when `read` returns (or
/// unwinds).
pub fn with_source<R>(source: &Arc<str>, read: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<str>>);

    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            SOURCE.with(|source| *source.borrow_mut() = previous);
        }
    }

    let previous = SOURCE.with(|current| current.borrow_mut().replace(Arc::clone(source)));
    let _restore = Restore(previous);
    read()
}

impl Name {
    /// A name holding its own copy of `name`.
    pub fn new(name: &str) -> Self {
        if name.is_empty() {
            return Self::default();
        }
        Name {
            text: Arc::from(name),
            start: WHOLE,
            len: 0,
        }
    }

    /// `name`, sharing the source text of the [`with_source`] it is read
    /// in when it is a slice of that text, or else a copy
    /// ([`Name::new`]).
    pub fn from_source(name: &str) -> Self {
        SOURCE
            .with(|source| {
                let source = source.borrow();
                let text = source.as_ref()?;
                let start = (name.as_ptr() as usize).checked_sub(text.as_ptr() as usize)?;
                let end = start.checked_add(name.len())?;
                if end > text.len() {
                    return None;
                }
                Some(Name {
                    text: Arc::clone(text),
                    start: u32::try_from(start).ok().filter(|start| *start != WHOLE)?,
                    len: u32::try_from(name.len()).ok()?,
                })
            })
            .unwrap_or_else(|| Name::new(name))
    }

    /// The name's text.
    pub fn as_str(&self) -> &str {
        if self.start == WHOLE {
            return &self.text;
        }
        let start = self.start as usize;
        self.text
            .get(start..start + self.len as usize)
            .unwrap_or_default()
    }

    /// The name's text, as a `String`.
    pub fn into_string(self) -> String {
        self.as_str().to_string()
    }
}

impl Default for Name {
    fn default() -> Self {
        Name {
            text: Arc::clone(&EMPTY),
            start: WHOLE,
            len: 0,
        }
    }
}

impl Deref for Name {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Name {}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for Name {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

macro_rules! eq_str {
    ($($other:ty),*) => {$(
        impl PartialEq<$other> for Name {
            fn eq(&self, other: &$other) -> bool {
                self.as_str() == &other[..]
            }
        }

        impl PartialEq<Name> for $other {
            fn eq(&self, other: &Name) -> bool {
                &self[..] == other.as_str()
            }
        }
    )*};
}

eq_str!(str, &str, String, Cow<'_, str>);

impl From<&str> for Name {
    fn from(name: &str) -> Self {
        Name::new(name)
    }
}

impl From<String> for Name {
    /// `name`'s text in the name's own `Arc<str>`: the one copy an
    /// `Arc<str>` needs (its counts sit in front of the text). An empty
    /// name shares the empty text, as [`Name::new`]'s does.
    fn from(name: String) -> Self {
        if name.is_empty() {
            return Self::default();
        }
        Name {
            text: Arc::from(name),
            start: WHOLE,
            len: 0,
        }
    }
}

impl From<&String> for Name {
    fn from(name: &String) -> Self {
        Name::new(name)
    }
}

impl From<Cow<'_, str>> for Name {
    fn from(name: Cow<'_, str>) -> Self {
        Name::new(&name)
    }
}

impl From<Name> for String {
    fn from(name: Name) -> Self {
        name.into_string()
    }
}

impl From<&Name> for String {
    fn from(name: &Name) -> Self {
        name.as_str().to_string()
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Name {
    /// Accepts and rejects exactly what deserializing a `String` does; a
    /// string borrowed from the source text of a [`with_source`] shares it.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NameVisitor;

        impl<'de> Visitor<'de> for NameVisitor {
            type Value = Name;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string")
            }

            fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Name, E> {
                Ok(Name::from_source(v))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Name, E> {
                Ok(Name::new(v))
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Name, E> {
                match std::str::from_utf8(v) {
                    Ok(s) => Ok(Name::new(s)),
                    Err(_) => Err(de::Error::invalid_value(de::Unexpected::Bytes(v), &self)),
                }
            }
        }

        deserializer.deserialize_string(NameVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_read_from_the_source_shares_it() {
        let source: Arc<str> = Arc::from(r#"{"name":"Person","other":"x\"y"}"#);
        let (shared, escaped) = with_source(&source, || {
            let value: std::collections::BTreeMap<String, Name> =
                serde_json::from_str(&source).unwrap();
            (value["name"].clone(), value["other"].clone())
        });
        assert_eq!(shared, "Person");
        assert!(Arc::ptr_eq(&shared.text, &source));
        assert_eq!(escaped, "x\"y");
        assert!(!Arc::ptr_eq(&escaped.text, &source));
        // Outside `with_source`, a name is a copy.
        let copy: Name = serde_json::from_str(r#""Person""#).unwrap();
        assert_eq!(copy, shared);
        assert!(!Arc::ptr_eq(&copy.text, &source));
    }

    #[test]
    fn a_name_reads_compares_and_round_trips_as_its_text() {
        let name = Name::from("abc");
        assert_eq!(&*name, "abc");
        assert_eq!(name, String::from("abc"));
        assert!("abc" == name);
        assert_eq!(name.to_string(), "abc");
        assert_eq!(format!("{name:?}"), "\"abc\"");
        assert_eq!(serde_json::to_string(&name).unwrap(), "\"abc\"");
        assert_eq!(Name::default(), "");
        let (a, b) = (Name::from("a"), Name::from("b"));
        assert!(a < b);
        assert!(serde_json::from_str::<Name>("1").is_err());
    }

    #[test]
    fn a_name_from_a_string_holds_its_text() {
        let name = Name::from(String::from("Person"));
        assert_eq!(name, "Person");
        assert_eq!(name.as_str(), "Person");
        let empty = Name::from(String::new());
        assert!(Arc::ptr_eq(&empty.text, &Name::default().text));
    }
}
