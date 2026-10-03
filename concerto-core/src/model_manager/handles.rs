//! The arena's handle types ([`ModelFileId`], [`DeclId`], [`PropId`]) and
//! the graph [`Node`] the manager's `ResolutionContext` hands out.

/// Declares a handle type: a dense `u32` index into one of the arena's
/// tables.
macro_rules! handle {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub(super) u32);

        impl $name {
            js_compat_pub! {
                /// The handle with this raw index, as a binding gets it back from
                /// JS. An index the manager never handed out names nothing: every
                /// lookup of it answers `None` or an error.
                pub const fn from_index(index: u32) -> Self {
                    Self(index)
                }
            }

            js_compat_pub! {
                /// The raw index, as a binding passes it to JS (a plain number).
                pub const fn index(self) -> u32 {
                    self.0
                }
            }

            /// The position in the arena table this handle indexes.
            pub(super) fn slot(self) -> usize {
                self.0 as usize
            }
        }
    };
}

handle! {
    /// A handle to a model file loaded into a [`ModelManager`](super::ModelManager).
    ModelFileId
}

handle! {
    /// A handle to a declaration of a model file loaded into a
    /// [`ModelManager`](super::ModelManager).
    DeclId
}

handle! {
    /// A handle to a property of a class declaration, or a value of an enum
    /// declaration (P2-04), loaded into a [`ModelManager`](super::ModelManager). Both are
    /// [`Property`](crate::Property) values, addressed the same way, through the unified
    /// `ClassLike::own_properties`.
    PropId
}

js_compat_pub! {
    /// A node of the model graph, as the manager's [`ResolutionContext`](super::ResolutionContext) hands
    /// it out.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Node {
        /// A loaded model file.
        ModelFile(ModelFileId),
        /// A declaration.
        Declaration(DeclId),
        /// A property of a class declaration.
        Property(PropId),
        /// A primitive type name. `ModelFile.getType` answers a primitive type
        /// with its name, a JS string, rather than a declaration.
        Primitive(&'static str),
    }
}
