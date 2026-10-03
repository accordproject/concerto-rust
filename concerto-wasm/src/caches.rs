//! The binding's process-global state (P5-101, D-7,
//! accordproject/concerto-rust#455), in one place: every `thread_local` the
//! crate keeps outside a [`ModelManagerHandle`](crate::ModelManagerHandle),
//! with what it is keyed on and what invalidates it. WASM is
//! single-threaded, so "thread-local" means "for the module instance": all
//! of it is shared by every handle.
//!
//! | State | Key | Invalidated by |
//! |---|---|---|
//! | [`HOST`] | none | [`crate::set_host`], which replaces it |
//! | [`SERIALIZER_OPTIONS`] | the options text of the last serializer call | the next call with other options text, which replaces it |
//! | [`SYSTEM_MODEL_HEADERS`] | the fixed system model texts | never: those texts are constants |
//! | [`SUPER_WALKS`] | the running super type walks | each walk step's drop ([`crate::SuperWalk`]) |
//! | [`LAST_ERROR`] | none | [`crate::validate_resource`]'s next non-zero code, or its take |
//!
//! What belongs to one manager (its model files, epoch, staging slot and
//! DCS extract memo) is a field of its handle instead; the epoch rule
//! (`ModelManagerHandle::epoch`) covers those only.

use std::cell::{OnceCell, RefCell};

use wasm_bindgen::JsValue;

use crate::{Error, Host, SerializerOptionsEntry};

thread_local! {
    /// The JS functions the shim registers at load ([`crate::set_host`]).
    pub(crate) static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };

    /// The options of the last serializer call (`serializerFromJson`,
    /// `serializerFromJsonCompact`, `serializerToJson`, `validateInstance`)
    /// as read from its options text ([`SerializerOptionsEntry`], P5-16,
    /// P5-101 D-3). Keyed by that text; any call with other text replaces
    /// it. Taken out for the length of a call
    /// ([`crate::with_serializer_options`]).
    pub(crate) static SERIALIZER_OPTIONS: RefCell<Option<SerializerOptionsEntry>> =
        const { RefCell::new(None) };

    /// P5-73: for each fixed system model text
    /// ([`concerto_core::rootmodel::system_model_json_texts`]), the header
    /// text `ModelManagerHandle::system_model_file_header` returns, or
    /// `None` when its checked load failed (never expected; that text is
    /// then loaded and checked every time, as any other). Filled on first
    /// use, never invalidated.
    pub(crate) static SYSTEM_MODEL_HEADERS: OnceCell<Vec<(&'static str, Option<String>)>> =
        const { OnceCell::new() };

    /// The declarations whose `getProperties`, `getProperty` or
    /// `getIdentifierFieldName` binding is running, with the binding's name,
    /// outermost first: each recurses into its super type through JS, so a
    /// declaration met again by the same binding is a cyclic inheritance
    /// chain (BC-11). Each entry is removed when its walk step ends.
    pub(crate) static SUPER_WALKS: RefCell<Vec<(&'static str, JsValue)>> =
        const { RefCell::new(Vec::new()) };

    /// The error behind the last non-zero code of the
    /// [`crate::validate_resource`] bindings, until it is read
    /// (`validateErrorMessage`) or taken (`validateTakeError`).
    pub(crate) static LAST_ERROR: RefCell<Option<Error>> = const { RefCell::new(None) };
}
