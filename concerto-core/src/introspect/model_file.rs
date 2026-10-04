//! One parsed Concerto model file.
//!
//! A [`ModelFile`] owns the declarations and imports of a single namespace and
//! indexes its declarations by short name. It resolves a short name to a
//! fully-qualified one from what it declares or imports: the primitives, its
//! own declarations, and its named imports. (Wildcard imports are rejected
//! while parsing, per strict mode in Concerto v4.)
//!
//! A model file also keeps the JSON AST it was built from, unchanged, as
//! [`ModelFile::ast`]. That AST is the source of truth for what the model
//! says: its key order, its `null`s and its numbers exactly as given. The typed
//! declarations and imports are a view of it, used for the runtime's logic.

use std::borrow::Cow;
use std::sync::{Arc, LazyLock, OnceLock};

use indexmap::IndexMap;

use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::hash::{FastSeededHashMap, FastSeededState};
use crate::introspect::declaration::{ClassDeclaration, Declaration};
use crate::introspect::decorator::{Decorated, Decorator, parse_decorator_list};
use crate::introspect::import::Import;
use crate::introspect::shape;
use crate::introspect::typed_ast::{self, ModelHeader, TypedDeclaration};
use crate::model_util::{self, is_primitive_type, is_valid_identifier, qualify, short_name};

/// The key of a declaration name in [`ModelFile`]'s `local_types`, hashed
/// with the per-process seeded foldhash ([`FastSeededState`], PORTING.md
/// 3.7): the names come from user models, and with an unseeded hash crafted
/// names could share one hash and turn every lookup into a scan.
fn name_hash(name: &str) -> u64 {
    use std::hash::BuildHasher;
    FastSeededState::default().hash_one(name)
}

/// `local_types`' value for a hash two different declaration names share.
const SHARED_HASH: usize = usize::MAX;

/// The most declarations a [`ModelFile`] finds a name among by scanning
/// them, with no `local_types` map.
const LOCAL_SCAN_MAX: usize = 8;

/// A parsed model file for one namespace.
#[derive(Debug, Clone)]
pub struct ModelFile {
    namespace: String,
    /// Where the version starts in `namespace` (the version is the
    /// namespace's own suffix, not a copy of it).
    version_start: usize,
    /// The imports; a file that imports nothing else shares one copy of the
    /// built-in import alone ([`built_in_imports`]).
    imports: Cow<'static, [Import]>,
    /// TS `ModelFile.importShortNames`: every name an import makes visible,
    /// to the import and the position in its `imported_names`, built with one
    /// forward pass over `imports` so a later import of a local name replaces
    /// an earlier one, as `Map.set` does ([`ModelFile::import_target`]).
    /// Shared like `imports` for a file whose only import is the built-in.
    import_short_names: Cow<'static, ImportShortNames>,
    declarations: Vec<Declaration>,
    /// Declaration names to their index, for `getLocalType`, keyed by the
    /// name's seeded hash ([`name_hash`]); a hash two names share maps to
    /// [`SHARED_HASH`]. Empty for a file of at most [`LOCAL_SCAN_MAX`]
    /// declarations, which are scanned ([`ModelFile::local_index`]).
    local_types: rustc_hash::FxHashMap<u64, usize>,
    file_name: Option<String>,
    ast: Ast,
    decorators: Vec<Decorator>,
    /// TS `ModelFile.concertoVersion`: the AST's own `concertoVersion` range
    /// (e.g. `"^3.0.0"`), once [`check_compatible_version`] has checked it
    /// against this runtime, or `None` when the AST carries none at all
    /// (`this.concertoVersion` stays its constructor default, `null`).
    concerto_version: Option<String>,
    /// TS `ModelFile.definitions`: the optional CTO source text a caller gave
    /// with the AST, kept verbatim and never parsed here (PORTING.md 1.1).
    definitions: Option<String>,
    /// TS `ModelFile.external`: [`ModelFile::file_name`] starts with `@`, a
    /// model downloaded from an external URI.
    external: bool,
}

impl Decorated for ModelFile {
    fn decorators(&self) -> &[Decorator] {
        &self.decorators
    }
}

impl ModelFile {
    /// Builds a model file from the JSON AST of a `concerto.metamodel@….Model`,
    /// with no CTO source text (`ModelFile::get_definitions` will answer
    /// `None`). TS: `new ModelFile(modelManager, ast, definitions, fileName)`
    /// with `definitions` omitted.
    pub fn from_json(value: &serde_json::Value, file_name: Option<String>) -> Result<Self> {
        Self::from_json_with_definitions(value, None, file_name)
    }

    /// [`ModelFile::from_json`], keeping the given CTO source text verbatim
    /// for `ModelFile::get_definitions`, never parsed.
    pub fn from_json_with_definitions(
        value: &serde_json::Value,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<Self> {
        let mut model_file = Self::load_value(value, definitions, file_name)?;
        model_file.ast = Ast::from_value(value.clone());
        Ok(model_file)
    }

    /// [`ModelFile::from_json_with_definitions`], taking ownership of the
    /// AST so it is kept without being copied. Same result and errors.
    pub fn from_owned_json_with_definitions(
        value: serde_json::Value,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<Self> {
        let mut model_file = Self::load_value(&value, definitions, file_name)?;
        model_file.ast = Ast::from_value(value);
        Ok(model_file)
    }

    /// [`ModelFile::from_json_with_definitions`] for an AST given as JSON
    /// text, with the same result and errors. The outer `Err` is the parse
    /// error for text that is not JSON. The text is read straight into the
    /// typed model; [`ModelFile::ast`] is parsed from it on first use.
    pub fn from_json_text(
        text: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<Self>, serde_json::Error> {
        Ok(Self::load_text_with_imports(text, definitions, file_name)?
            .map(|(model_file, _)| model_file))
    }

    /// [`ModelFile::from_json_text`], also returning the AST's own `imports`
    /// node as the text holds it (`None` without an `imports` key), so the
    /// binding reads the TS `ModelFile` header without a second decode.
    #[cfg(feature = "js-compat")]
    pub fn from_json_text_with_imports(
        text: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        Self::load_text_with_imports(text, definitions, file_name)
    }

    /// [`ModelFile::from_json_text_with_imports`] with BC-19's AST shape check
    /// first, as the TS constructor runs it, from one parse and one strict
    /// decode. A rejected AST is the check's error, before any part of the load.
    /// The check is folded into the typed read; only an AST it cannot vouch for
    /// gets the full check, so the verdict and error are always that check's.
    #[cfg(feature = "js-compat")]
    pub fn from_json_text_checked_with_imports(
        text: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        Self::load_text(text, definitions, file_name, true)
    }

    /// [`ModelFile::from_json_text_with_imports`] for the AST in the compact
    /// binary layout (`introspect::compact`), which the TS `ModelFile`
    /// constructor writes from an AST object. The result and error are those
    /// for `JSON.stringify`'s text of the same AST; the outer `Err` is for
    /// bytes not in the layout. [`ModelFile::ast`] is decoded from the kept
    /// bytes on first use.
    #[cfg(feature = "js-compat")]
    pub fn from_compact_with_imports(
        bytes: &[u8],
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        Self::load_compact(bytes, definitions, file_name, false)
    }

    /// [`ModelFile::from_compact_with_imports`] with BC-19's AST shape check
    /// first, folded as [`ModelFile::from_json_text_checked_with_imports`]
    /// folds it.
    #[cfg(feature = "js-compat")]
    pub fn from_compact_checked_with_imports(
        bytes: &[u8],
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        Self::load_compact(bytes, definitions, file_name, true)
    }

    /// [`ModelFile::load_text`] for the AST in the compact binary layout
    /// (`from_compact_with_imports`), step by step: where `load_text`
    /// parses its text into a `Value`, this decodes the bytes into one
    /// (`compact::to_value`), and an error there is the outer one.
    #[cfg(feature = "js-compat")]
    fn load_compact(
        bytes: &[u8],
        definitions: Option<String>,
        file_name: Option<String>,
        checked: bool,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        use crate::introspect::compact;
        let model = match typed_ast::from_compact(bytes) {
            Ok(model) => model,
            Err(err) => {
                // Bytes not in the layout are the outer error; any other
                // error is the read's, as `load_text` handles it.
                let value = compact::to_value(bytes)?;
                if checked
                    && let Err(shape) = crate::instance::metamodel::check_ast_shape_exact(&value)
                {
                    return Ok(Err(shape));
                }
                return Ok(Err(unreadable_ast(&err, file_name.as_deref())));
            }
        };
        if checked && !shape::conforms(&model) {
            let value = compact::to_value(bytes)?;
            if let Err(shape) = crate::instance::metamodel::check_ast_shape_exact(&value) {
                return Ok(Err(shape));
            }
        }
        let mut header = model.header;
        let result = Self::load(&mut header, model.declarations, definitions, file_name).map(
            |mut model_file| {
                model_file.ast = Ast::from_compact(bytes);
                (model_file, header.imports)
            },
        );
        Ok(result)
    }

    /// The body of [`ModelFile::from_json_text`] and of the `js-compat`
    /// `from_json_text_with_imports`: the loaded file plus the AST's own
    /// `imports` node.
    fn load_text_with_imports(
        text: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        Self::load_text(text, definitions, file_name, false)
    }

    /// [`ModelFile::load_text_with_imports`], with BC-19's shape check first
    /// when `checked` (`from_json_text_checked_with_imports`).
    pub(crate) fn load_text(
        text: &str,
        definitions: Option<String>,
        file_name: Option<String>,
        checked: bool,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        // The kept copy of the text is made first, so the names the read
        // reads share it rather than each being copied.
        let source: Arc<str> = Arc::from(text);
        let model = match concerto_metamodel::with_source(&source, || typed_ast::parse(&source)) {
            Ok(model) => model,
            Err(err) => {
                // Text that is JSON, but not a model the reader can read,
                // is a load error; text that is not JSON at all (even past
                // the point the reader stopped at) is the parse error.
                if !err.is_data() {
                    return Err(err);
                }
                let value = serde_json::from_str::<serde_json::Value>(text)?;
                // The check decides first: an AST it accepts but the read cannot
                // read is the load's error.
                if checked
                    && let Err(shape) = crate::instance::metamodel::check_ast_shape_exact(&value)
                {
                    return Ok(Err(shape));
                }
                return Ok(Err(unreadable_ast(&err, file_name.as_deref())));
            }
        };
        if checked && !shape::conforms(&model) {
            let value = serde_json::from_str::<serde_json::Value>(text)?;
            if let Err(shape) = crate::instance::metamodel::check_ast_shape_exact(&value) {
                return Ok(Err(shape));
            }
        }
        let mut header = model.header;
        let result = Self::load(&mut header, model.declarations, definitions, file_name).map(
            |mut model_file| {
                model_file.ast = Ast::from_text(source);
                (model_file, header.imports)
            },
        );
        Ok(result)
    }

    /// Reads `value` into the typed model and loads it, leaving
    /// [`ModelFile::ast`] `Null` for the caller to fill in.
    fn load_value(
        value: &serde_json::Value,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<Self> {
        let mut model = typed_ast::from_value(value)
            .map_err(|err| unreadable_ast(&err, file_name.as_deref()))?;
        Self::load(
            &mut model.header,
            model.declarations,
            definitions,
            file_name,
        )
    }

    /// Builds the model file from the model the typed read gave: `header`
    /// is the AST's header (every top-level key but `declarations`; its
    /// `namespace` is taken), and `declarations` its declarations, already
    /// read. Leaves [`ModelFile::ast`] `Null` for the caller to fill in.
    fn load(
        header: &mut ModelHeader,
        mut typed: Vec<TypedDeclaration>,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<Self> {
        // The model's own keys, read as strictly as every other node's
        // (`typed_ast`'s module doc, "Unknown keys").
        if let Some(key) = &header.unknown {
            return Err(unreadable_ast(
                &serde::de::Error::custom(format_args!("unknown field `{key}`")),
                file_name.as_deref(),
            ));
        }
        // The decorators taken from the value as it is checked.
        let decorators = match header.decorators.take() {
            None => Vec::new(),
            Some(Ok(decorators)) => decorators.list,
            Some(Err(value)) => {
                let decorators = parse_decorator_list(Some(&value));
                value
                    .into_decorators()
                    .map_err(|err| unreadable_ast(&err, file_name.as_deref()))?;
                decorators
            }
        };

        // The header's own string, taken rather than copied.
        let namespace = match header.namespace.take() {
            Some(serde_json::Value::String(namespace)) => namespace,
            _ => {
                return Err(Error::illegal_model(
                    "model missing 'namespace'",
                    file_name,
                    None,
                ));
            }
        };

        // TS: `ModelFile.fromAst`'s namespace handling: `parseNamespace`,
        // valid identifiers, and a version for every model file (BC-02).
        let is_system_namespace = namespace.starts_with("concerto@") || namespace == "concerto";
        let version_start =
            namespace.len() - parse_namespace_version(&namespace, &file_name)?.len();

        let mut imports = match &header.imports {
            None => Vec::new(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(Import::try_from)
                .collect::<Result<Vec<_>>>()?,
            Some(_) => {
                return Err(Error::illegal_model(
                    "model 'imports' must be an array",
                    file_name,
                    None,
                ));
            }
        };

        // TS: `ModelFile.fromAst`'s `imports.forEach` loop checks every
        // import: an alias may not name a primitive, and the namespace must
        // carry a version (`enforceImportVersioning`), each a plain `Error`.
        // The built-in import passes both, so only the AST's own are
        // checked, before it is appended.
        for imp in &imports {
            for alias in imp.aliased_types() {
                if is_primitive_type(&alias.aliased_name) {
                    return Err(plain_error(
                        "Types cannot be aliased to primitive type".to_string(),
                    ));
                }
            }
            // `parseNamespace`'s checks, without its owned result.
            let versioned = model_util::split_namespace(imp.namespace())?.1.is_some();
            if !versioned {
                return Err(plain_error(format!(
                    "Cannot use an unversioned import {}.",
                    imp.namespace()
                )));
            }
        }

        // Every non-system model file imports the system types implicitly
        // (TS: `ModelFile.fromAst`).
        let is_system = is_system_namespace;
        let imports: Cow<'static, [Import]> = if is_system {
            Cow::Owned(imports)
        } else if imports.is_empty() {
            built_in_imports()?
        } else {
            imports.push(built_in_import_typed()?);
            Cow::Owned(imports)
        };
        let import_short_names = match &imports {
            Cow::Borrowed(_) => Cow::Borrowed(&*BUILT_IN_SHORT_NAMES),
            Cow::Owned(imports) => Cow::Owned(import_short_names(imports)),
        };

        // TS: the constructor's `localTypes` loop is a plain `Map.set`, so a
        // second declaration of a name replaces the first in the lookup,
        // while `getAllDeclarations()` lists both; `ModelFile.validate()`'s
        // duplicate-name scan rejects it.
        let mut declarations: Vec<Declaration> = Vec::with_capacity(typed.len());
        let mut local_types = rustc_hash::FxHashMap::default();
        let indexed = typed.len() > LOCAL_SCAN_MAX;
        if indexed {
            local_types.reserve(typed.len());
        }
        for raw in typed.drain(..) {
            let decl = Declaration::from_typed(raw, &namespace, file_name.as_deref())
                .map_err(|e| annotate(e, &file_name))?;
            let index = declarations.len();
            if indexed {
                local_types
                    .entry(name_hash(decl.name()))
                    .and_modify(|slot: &mut usize| {
                        *slot = if *slot != SHARED_HASH && declarations[*slot].name() == decl.name()
                        {
                            index
                        } else {
                            SHARED_HASH
                        };
                    })
                    .or_insert(index);
            }
            declarations.push(decl);
        }
        typed_ast::recycle_declarations(typed);

        // TS: `ModelFile.isCompatibleVersion` runs after `fromAst`, so a bad
        // declaration is reported ahead of an incompatible `concertoVersion`.
        let concerto_version = check_compatible_version(header.concerto_version.as_ref())?;

        let external = file_name.as_deref().is_some_and(|n| n.starts_with('@'));

        Ok(Self {
            namespace,
            version_start,
            imports,
            import_short_names,
            declarations,
            local_types,
            file_name,
            decorators,
            ast: Ast::from_value(serde_json::Value::Null),
            concerto_version,
            definitions,
            external,
        })
    }

    js_compat_pub! {
        /// TS: the argument checks `new ModelFile(modelManager, ast,
        /// definitions, fileName)` runs before it reads the AST, for a caller
        /// holding arbitrary JS values (`None` is `undefined`). In order, each
        /// a plain `Error`: a falsy `ast` (`ast not specified`), an `ast` that
        /// is not an object, a truthy non-string `definitions`, a truthy
        /// non-string `fileName`.
        #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
        pub fn check_constructor_arguments(
            ast: Option<&serde_json::Value>,
            definitions: Option<&serde_json::Value>,
            file_name: Option<&serde_json::Value>,
        ) -> Result<()> {
            let truthy = |v: Option<&serde_json::Value>| v.is_some_and(crate::ecma::is_truthy);
            if !truthy(ast) {
                return Err(plain_error("ast not specified".into()));
            }
            // `typeof ast !== 'object'`: an array is an object too; `null` is
            // already rejected above as falsy.
            if !matches!(
                ast,
                Some(serde_json::Value::Object(_) | serde_json::Value::Array(_))
            ) {
                return Err(plain_error(
                    "ModelFile expects a Concerto model AST as input.".into(),
                ));
            }
            if truthy(definitions) && !matches!(definitions, Some(serde_json::Value::String(_))) {
                return Err(plain_error(
                    "ModelFile expects an (optional) Concerto model definition as a string.".into(),
                ));
            }
            if truthy(file_name) && !matches!(file_name, Some(serde_json::Value::String(_))) {
                return Err(plain_error(
                    "ModelFile expects an (optional) filename as a string.".into(),
                ));
            }
            Ok(())
        }
    }

    /// The full namespace, including the version, e.g. `org.example@1.0.0`.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The version part of the namespace.
    pub fn version(&self) -> &str {
        self.namespace.get(self.version_start..).unwrap_or_default()
    }

    /// The JSON AST this model file was built from, exactly as it was given.
    pub fn ast(&self) -> &serde_json::Value {
        self.ast.get()
    }

    /// [`ModelFile::ast`]'s compact JSON text (`serde_json::to_string`). A
    /// file built from a parsed AST then keeps that text in its place,
    /// parsed again on first use into an equal AST (`float_roundtrip`,
    /// `preserve_order`); a file read from text is left as it is.
    #[cfg(feature = "js-compat")]
    pub fn compact_ast(&mut self) -> serde_json::Result<Arc<str>> {
        let text: Arc<str> = Arc::from(serde_json::to_string(self.ast())?);
        if self.ast.text().is_none() {
            self.ast = Ast::from_text(Arc::clone(&text));
        }
        Ok(text)
    }

    /// Whether this file was built by the typed AST path.
    #[cfg(test)]
    pub(crate) fn built_by_typed_path(&self) -> bool {
        self.ast.text().is_some()
    }

    /// Whether this file and `other` were built from equal ASTs
    /// (`self.ast() == other.ast()`), without parsing either one's kept
    /// text when both were built from the same text
    /// ([`ModelFile::from_json_text`]).
    pub(crate) fn same_ast(&self, other: &ModelFile) -> bool {
        match (&self.ast.source, &other.ast.source) {
            (AstSource::Text(a), AstSource::Text(b)) if a == b => return true,
            (AstSource::Compact(a), AstSource::Compact(b)) if a == b => return true,
            _ => {}
        }
        self.ast() == other.ast()
    }

    /// The originating file name, if one was supplied.
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    /// Every declaration, in the order they appear in the file.
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    /// The imports.
    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    /// Finds a declaration by its short name.
    pub fn local_declaration(&self, short: &str) -> Option<&Declaration> {
        self.local_index(short).map(|i| &self.declarations[i])
    }

    /// The position in [`ModelFile::declarations`] of the declaration with
    /// this short name. The model manager's arena addresses a declaration by
    /// its file and this position.
    pub(crate) fn local_index(&self, short: &str) -> Option<usize> {
        if self.local_types.is_empty() {
            // At most `LOCAL_SCAN_MAX` declarations (or none): the last
            // declaration of the name.
            return self.declarations.iter().rposition(|d| d.name() == short);
        }
        match *self.local_types.get(&name_hash(short))? {
            // Two different names share this hash: the last declaration of
            // the name, as the map of names held it.
            SHARED_HASH => self.declarations.iter().rposition(|d| d.name() == short),
            index => (self.declarations[index].name() == short).then_some(index),
        }
    }

    /// True if this is the built-in `concerto` system namespace.
    pub fn is_system_namespace(&self) -> bool {
        self.namespace.starts_with("concerto@")
    }

    /// Resolves a short name from the primitives, this file's named imports and
    /// its own declarations, imports first, as TS `getType`/`resolveType` do.
    /// A name is both only under
    /// `dangerouslyAllowReservedSystemTypeNamesInUserModels`, where a local
    /// `Asset` must still resolve its implicit super type to the system one.
    pub fn resolve_local_type(&self, short: &str) -> Option<String> {
        if is_primitive_type(short) {
            return Some(short.to_string());
        }
        if let Some(fqn) = self.find_import(short) {
            return Some(fqn);
        }
        if self.local_index(short).is_some() {
            return Some(qualify(&self.namespace, short));
        }
        None
    }

    /// TS: `ModelFile.getConcertoVersion` — the AST's own `concertoVersion`
    /// range (`"^3.0.0"`), once checked; `None` when the AST carries none.
    pub fn concerto_version(&self) -> Option<&str> {
        self.concerto_version.as_deref()
    }

    /// TS: `ModelFile.getDefinitions` — the CTO source text a caller gave
    /// alongside the AST, verbatim ([`ModelFile::from_json_with_definitions`]),
    /// or `None` when built without one ([`ModelFile::from_json`]).
    pub fn definitions(&self) -> Option<&str> {
        self.definitions.as_deref()
    }

    /// TS: `ModelFile.isExternal` — `true` when this file's name starts with
    /// `@`, meaning it was downloaded from an external URI rather than given
    /// directly.
    pub fn is_external(&self) -> bool {
        self.external
    }

    /// TS: `ModelFile.getImportURI`: the URI an import was given (`import
    /// ns.Name from 'uri'`), keyed as TS's `importUriMap` is, by the first
    /// fully-qualified name the import brings in
    /// (`ModelUtil.importFullyQualifiedNames(imp)[0]`).
    pub fn import_uri(&self, key: &str) -> Option<&str> {
        self.imports.iter().find_map(|imp| {
            let uri = imp.uri()?;
            let first = imp.imported_names().first()?;
            (qualify(imp.namespace(), first) == key).then_some(uri)
        })
    }

    /// Deprecated name of [`ModelFile::import_uri`].
    #[deprecated(since = "0.1.0", note = "use `import_uri`")]
    pub fn get_import_uri(&self, key: &str) -> Option<&str> {
        self.import_uri(key)
    }

    /// TS: `ModelFile.getExternalImports` — every import-URI pair
    /// [`ModelFile::get_import_uri`] can answer, keyed the same way.
    ///
    /// In import order, as TS's `importUriMap` keeps insertion order with a
    /// later duplicate key overwriting in place (PORTING.md 3.7).
    pub fn external_imports(&self) -> IndexMap<String, String> {
        let mut out = IndexMap::new();
        for imp in self.imports.iter() {
            let Some(uri) = imp.uri() else { continue };
            let Some(first) = imp.imported_names().first() else {
                continue;
            };
            out.insert(qualify(imp.namespace(), first), uri.to_string());
        }
        out
    }

    /// Deprecated name of [`ModelFile::external_imports`].
    #[deprecated(since = "0.1.0", note = "use `external_imports`")]
    pub fn get_external_imports(&self) -> IndexMap<String, String> {
        self.external_imports()
    }

    /// TS: `ModelFile.getImports`: the fully-qualified names this file
    /// imports (declared names, never aliases), with the built-in system
    /// import for a non-system file.
    pub fn imported_type_names(&self) -> Vec<String> {
        self.imports
            .iter()
            .flat_map(|imp| {
                imp.imported_names()
                    .iter()
                    .map(|name| qualify(imp.namespace(), name))
            })
            .collect()
    }

    /// Deprecated name of [`ModelFile::imported_type_names`].
    #[deprecated(since = "0.1.0", note = "use `imported_type_names`")]
    pub fn get_imports(&self) -> Vec<String> {
        self.imported_type_names()
    }

    /// TS: `ModelFile.getLocalType` — accepts either a short name, or a name
    /// already qualified with this file's own namespace.
    pub fn local_type(&self, type_name: &str) -> Option<&Declaration> {
        self.local_type_index(type_name)
            .map(|i| &self.declarations[i])
    }

    /// The position in [`ModelFile::declarations`] of
    /// [`ModelFile::local_type`]'s declaration.
    pub(crate) fn local_type_index(&self, type_name: &str) -> Option<usize> {
        let short = type_name
            .strip_prefix(self.namespace.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
            .unwrap_or(type_name);
        self.local_index(short)
    }

    /// Deprecated name of [`ModelFile::local_type`].
    #[deprecated(since = "0.1.0", note = "use `local_type`")]
    pub fn get_local_type(&self, type_name: &str) -> Option<&Declaration> {
        self.local_type(type_name)
    }

    /// TS: `ModelFile.isLocalType`.
    pub fn is_local_type(&self, type_name: &str) -> bool {
        !type_name.is_empty() && self.local_type(type_name).is_some()
    }

    /// The namespace and declared name a locally visible import name (an
    /// alias only under its alias) resolves to, from `importShortNames`: a
    /// later import of a local name replaces an earlier one, so the built-in
    /// import `fromAst` appends last wins.
    pub(crate) fn import_target(&self, type_name: &str) -> Option<(&str, &str)> {
        let &(import, position) = self.import_short_names.get(type_name)?;
        let import = &self.imports[import as usize];
        Some((
            import.namespace(),
            import.imported_names()[position as usize].as_str(),
        ))
    }

    /// [`ModelFile::import_target`], fully qualified.
    pub(crate) fn find_import(&self, type_name: &str) -> Option<String> {
        self.import_target(type_name)
            .map(|(namespace, name)| qualify(namespace, name))
    }

    /// TS: `ModelFile.isImportedType`.
    pub fn is_imported_type(&self, type_name: &str) -> bool {
        self.find_import(type_name).is_some()
    }

    /// TS: `ModelFile.resolveImport`. The error names this file, and its
    /// `imports` parameter is TS's `JSON.stringify(this.imports)`.
    pub fn resolve_import(&self, type_name: &str) -> Result<String> {
        self.find_import(type_name).ok_or_else(|| {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "modelfile-resolveimport-failfindimp",
                vec![
                    ("type", type_name.to_string()),
                    ("imports", self.imports_json()),
                    ("namespace", self.namespace.clone()),
                ],
            );
            err.model_file = Some(self.file_name.clone());
            err.into()
        })
    }

    /// TS: `ModelFile.getImportedType` — the actual (possibly aliased) local
    /// name's target short name, from the namespace it is imported from.
    pub fn imported_type(&self, type_name: &str) -> Result<String> {
        self.resolve_import(type_name)
            .map(|fqn| short_name(&fqn).to_string())
    }

    /// Deprecated name of [`ModelFile::imported_type`].
    #[deprecated(since = "0.1.0", note = "use `imported_type`")]
    pub fn get_imported_type(&self, type_name: &str) -> Result<String> {
        self.imported_type(type_name)
    }

    /// TS: `ModelFile.isDefined` — a primitive, or a type this file declares
    /// itself (an imported-only name is not "defined" by this file).
    pub fn is_defined(&self, type_name: &str) -> bool {
        is_primitive_type(type_name) || self.local_type(type_name).is_some()
    }

    /// TS: `ModelFile.getFullyQualifiedTypeName`, entirely local: a
    /// primitive's own name, an imported name's target FQN, or a local
    /// type's FQN; `None` (TS `null`) otherwise.
    pub fn fully_qualified_type_name(&self, type_name: &str) -> Option<String> {
        if is_primitive_type(type_name) {
            return Some(type_name.to_string());
        }
        if let Some(fqn) = self.find_import(type_name) {
            return Some(fqn);
        }
        self.local_type(type_name)
            .map(|d| qualify(&self.namespace, d.name()))
    }

    /// Deprecated name of [`ModelFile::fully_qualified_type_name`].
    #[deprecated(since = "0.1.0", note = "use `fully_qualified_type_name`")]
    pub fn get_fully_qualified_type_name(&self, type_name: &str) -> Option<String> {
        self.fully_qualified_type_name(type_name)
    }

    /// TS: `ModelFile.getAssetDeclaration`.
    pub fn asset_declaration(&self, name: &str) -> Option<&Declaration> {
        self.local_type(name)
            .filter(|d| d.as_class().is_some_and(ClassDeclaration::is_asset))
    }

    /// Deprecated name of [`ModelFile::asset_declaration`].
    #[deprecated(since = "0.1.0", note = "use `asset_declaration`")]
    pub fn get_asset_declaration(&self, name: &str) -> Option<&Declaration> {
        self.asset_declaration(name)
    }

    /// TS: `ModelFile.getTransactionDeclaration`.
    pub fn transaction_declaration(&self, name: &str) -> Option<&Declaration> {
        self.local_type(name)
            .filter(|d| d.as_class().is_some_and(ClassDeclaration::is_transaction))
    }

    /// Deprecated name of [`ModelFile::transaction_declaration`].
    #[deprecated(since = "0.1.0", note = "use `transaction_declaration`")]
    pub fn get_transaction_declaration(&self, name: &str) -> Option<&Declaration> {
        self.transaction_declaration(name)
    }

    /// TS: `ModelFile.getEventDeclaration`.
    pub fn event_declaration(&self, name: &str) -> Option<&Declaration> {
        self.local_type(name)
            .filter(|d| d.as_class().is_some_and(ClassDeclaration::is_event))
    }

    /// Deprecated name of [`ModelFile::event_declaration`].
    #[deprecated(since = "0.1.0", note = "use `event_declaration`")]
    pub fn get_event_declaration(&self, name: &str) -> Option<&Declaration> {
        self.event_declaration(name)
    }

    /// TS: `ModelFile.getParticipantDeclaration`.
    pub fn participant_declaration(&self, name: &str) -> Option<&Declaration> {
        self.local_type(name)
            .filter(|d| d.as_class().is_some_and(ClassDeclaration::is_participant))
    }

    /// Deprecated name of [`ModelFile::participant_declaration`].
    #[deprecated(since = "0.1.0", note = "use `participant_declaration`")]
    pub fn get_participant_declaration(&self, name: &str) -> Option<&Declaration> {
        self.participant_declaration(name)
    }

    /// TS: `ModelFile.getAssetDeclarations`.
    pub fn asset_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.by_class_kind(ClassDeclaration::is_asset)
    }

    /// Deprecated name of [`ModelFile::asset_declarations`].
    #[deprecated(since = "0.1.0", note = "use `asset_declarations`")]
    pub fn get_asset_declarations(&self) -> Vec<&Declaration> {
        self.asset_declarations().collect()
    }

    /// TS: `ModelFile.getTransactionDeclarations`.
    pub fn transaction_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.by_class_kind(ClassDeclaration::is_transaction)
    }

    /// Deprecated name of [`ModelFile::transaction_declarations`].
    #[deprecated(since = "0.1.0", note = "use `transaction_declarations`")]
    pub fn get_transaction_declarations(&self) -> Vec<&Declaration> {
        self.transaction_declarations().collect()
    }

    /// TS: `ModelFile.getEventDeclarations`.
    pub fn event_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.by_class_kind(ClassDeclaration::is_event)
    }

    /// Deprecated name of [`ModelFile::event_declarations`].
    #[deprecated(since = "0.1.0", note = "use `event_declarations`")]
    pub fn get_event_declarations(&self) -> Vec<&Declaration> {
        self.event_declarations().collect()
    }

    /// TS: `ModelFile.getParticipantDeclarations`.
    pub fn participant_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.by_class_kind(ClassDeclaration::is_participant)
    }

    /// Deprecated name of [`ModelFile::participant_declarations`].
    #[deprecated(since = "0.1.0", note = "use `participant_declarations`")]
    pub fn get_participant_declarations(&self) -> Vec<&Declaration> {
        self.participant_declarations().collect()
    }

    /// TS: `ModelFile.getConceptDeclarations`.
    pub fn concept_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.by_class_kind(ClassDeclaration::is_concept)
    }

    /// Deprecated name of [`ModelFile::concept_declarations`].
    #[deprecated(since = "0.1.0", note = "use `concept_declarations`")]
    pub fn get_concept_declarations(&self) -> Vec<&Declaration> {
        self.concept_declarations().collect()
    }

    fn by_class_kind(
        &self,
        matches_kind: fn(&ClassDeclaration) -> bool,
    ) -> impl Iterator<Item = &Declaration> {
        self.declarations
            .iter()
            .filter(move |d| d.as_class().is_some_and(matches_kind))
    }

    /// TS: `ModelFile.getClassDeclarations`: `instanceof ClassDeclaration`,
    /// which an enum satisfies too; only a map or scalar is left out.
    pub fn class_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.declarations
            .iter()
            .filter(|d| !d.is_map_declaration() && !d.is_scalar_declaration())
    }

    /// Deprecated name of [`ModelFile::class_declarations`].
    #[deprecated(since = "0.1.0", note = "use `class_declarations`")]
    pub fn get_class_declarations(&self) -> Vec<&Declaration> {
        self.class_declarations().collect()
    }

    /// TS: `ModelFile.getEnumDeclarations`.
    pub fn enum_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.declarations.iter().filter(|d| d.is_enum_declaration())
    }

    /// Deprecated name of [`ModelFile::enum_declarations`].
    #[deprecated(since = "0.1.0", note = "use `enum_declarations`")]
    pub fn get_enum_declarations(&self) -> Vec<&Declaration> {
        self.enum_declarations().collect()
    }

    /// TS: `ModelFile.getMapDeclarations`.
    pub fn map_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.declarations.iter().filter(|d| d.is_map_declaration())
    }

    /// Deprecated name of [`ModelFile::map_declarations`].
    #[deprecated(since = "0.1.0", note = "use `map_declarations`")]
    pub fn get_map_declarations(&self) -> Vec<&Declaration> {
        self.map_declarations().collect()
    }

    /// TS: `ModelFile.getScalarDeclarations`.
    pub fn scalar_declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.declarations
            .iter()
            .filter(|d| d.is_scalar_declaration())
    }

    /// Deprecated name of [`ModelFile::scalar_declarations`].
    #[deprecated(since = "0.1.0", note = "use `scalar_declarations`")]
    pub fn get_scalar_declarations(&self) -> Vec<&Declaration> {
        self.scalar_declarations().collect()
    }

    /// TS: `ModelFile.filter`: a new model file with only the declarations
    /// `predicate` accepts, or `None` (TS `null`) if none is left. An import
    /// is dropped when every declaration it brings in is rejected, an
    /// `ImportTypes`' `types`/`aliasedTypes` pruned as TS prunes them; the
    /// built-in import always survives. `source_manager` is the manager the
    /// file is loaded into, through which each import's source file is read.
    pub fn filter(
        &self,
        predicate: impl Fn(&Declaration) -> bool,
        source_manager: &crate::model_manager::ModelManager,
    ) -> Result<Option<Self>> {
        match self.filter_outcome(predicate, source_manager)? {
            FilterOutcome::Empty => Ok(None),
            FilterOutcome::Unchanged => Self::from_json_with_definitions(
                self.ast(),
                self.definitions.clone(),
                self.file_name.clone(),
            )
            .map(Some),
            FilterOutcome::Filtered(filtered) => Ok(Some(*filtered)),
        }
    }

    js_compat_pub! {
        /// [`ModelFile::filter`], telling apart a filter that keeps the file
        /// exactly as it is ([`FilterOutcome::Unchanged`]), so a caller may
        /// keep this file, shared. The predicate is called as `filter` calls
        /// it, and the same errors are returned.
        pub fn filter_outcome(
            &self,
            predicate: impl Fn(&Declaration) -> bool,
            source_manager: &crate::model_manager::ModelManager,
        ) -> Result<FilterOutcome> {
            self.filter_outcome_at(|_, _, decl| predicate(decl), source_manager)
        }
    }

    /// [`ModelFile::filter_outcome`], with a predicate also handed the
    /// declaring file's namespace and the declaration's position there, by
    /// which the model manager keys its kept set.
    pub(crate) fn filter_outcome_at(
        &self,
        predicate: impl Fn(&str, usize, &Declaration) -> bool,
        source_manager: &crate::model_manager::ModelManager,
    ) -> Result<FilterOutcome> {
        let ast_declarations: &[serde_json::Value] = self
            .ast()
            .get("declarations")
            .and_then(|v| v.as_array())
            .map_or(&[], Vec::as_slice);
        let keep: Vec<bool> = ast_declarations
            .iter()
            .zip(self.declarations.iter().enumerate())
            .map(|(_, (index, decl))| predicate(&self.namespace, index, decl))
            .collect();
        let kept = keep.iter().filter(|k| **k).count();

        if kept == 0 {
            return Ok(FilterOutcome::Empty);
        }
        let all_declarations_kept =
            kept == ast_declarations.len() && kept == self.declarations.len();

        let original_imports = self.ast().get("imports").and_then(|v| v.as_array());
        let kept_imports: Option<Vec<serde_json::Value>> =
            original_imports.cloned().map(|imports| {
                imports
                    .into_iter()
                    .filter_map(|imp| filter_import(imp, &predicate, source_manager))
                    .collect()
            });
        let imports_unchanged = match (original_imports, &kept_imports) {
            (Some(original), Some(kept)) => original == kept,
            _ => true,
        };
        if all_declarations_kept && imports_unchanged {
            return Ok(FilterOutcome::Unchanged);
        }

        let declarations: Vec<serde_json::Value> = ast_declarations
            .iter()
            .zip(&keep)
            .filter(|(_, keep)| **keep)
            .map(|(ast, _)| ast.clone())
            .collect();
        let mut filtered = self.ast().clone();
        filtered["declarations"] = serde_json::Value::Array(declarations);
        if let Some(kept) = kept_imports {
            filtered["imports"] = serde_json::Value::Array(kept);
        }

        Self::from_json_with_definitions(
            &filtered,
            self.definitions.clone(),
            self.file_name.clone(),
        )
        .map(|filtered| FilterOutcome::Filtered(Box::new(filtered)))
    }
}

js_compat_pub! {
    /// What [`ModelFile::filter_outcome`] found.
    #[derive(Debug)]
    pub enum FilterOutcome {
        /// No declaration was kept: `filter` returns `None` (TS `null`).
        Empty,
        /// Every declaration was kept and every import is unchanged: the
        /// filtered file is the file itself.
        Unchanged,
        /// The filtered file.
        Filtered(Box<ModelFile>),
    }
}

/// One import of [`ModelFile::filter_outcome`]: kept as it is, kept with
/// its `types`/`aliasedTypes` pruned, or dropped (`None`), as TS
/// `ModelFile.filter` prunes it.
fn filter_import(
    mut imp: serde_json::Value,
    predicate: &impl Fn(&str, usize, &Declaration) -> bool,
    source_manager: &crate::model_manager::ModelManager,
) -> Option<serde_json::Value> {
    let namespace = imp.get("namespace").and_then(|v| v.as_str())?.to_string();
    if namespace.starts_with("concerto@") || namespace == "concerto" {
        return Some(imp);
    }
    let short_class = short_name(imp.get("$class").and_then(|v| v.as_str()).unwrap_or(""));
    let source_file = source_manager.model_file(&namespace);
    match short_class {
        "ImportType" => {
            let name = imp.get("name").and_then(|v| v.as_str())?;
            let keep = source_file.is_none_or(|sf| keeps(sf, name, predicate));
            keep.then_some(imp)
        }
        "ImportTypes" => {
            let Some(sf) = source_file else {
                return Some(imp);
            };
            let types = imp.get("types").and_then(|v| v.as_array())?.clone();
            let kept_types: Vec<String> = types
                .iter()
                .filter_map(|t| t.as_str())
                .filter(|name| keeps(sf, name, predicate))
                .map(str::to_string)
                .collect();
            if kept_types.is_empty() {
                return None;
            }
            if let Some(aliased) = imp.get("aliasedTypes").and_then(|v| v.as_array()).cloned()
                && !aliased.is_empty()
            {
                let kept_aliased: Vec<serde_json::Value> = aliased
                    .into_iter()
                    .filter(|a| {
                        a.get("name")
                            .and_then(|v| v.as_str())
                            .is_some_and(|n| kept_types.iter().any(|k| k == n))
                    })
                    .collect();
                imp["aliasedTypes"] = serde_json::Value::Array(kept_aliased);
            }
            imp["types"] = serde_json::Value::Array(
                kept_types
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            );
            Some(imp)
        }
        _ => Some(imp),
    }
}

/// Whether [`filter_import`] keeps the import of `name` from `source_file`:
/// unless `predicate` rejects the declaration it names there (a name the
/// file does not declare is kept).
fn keeps(
    source_file: &ModelFile,
    name: &str,
    predicate: &impl Fn(&str, usize, &Declaration) -> bool,
) -> bool {
    source_file.local_type_index(name).is_none_or(|index| {
        predicate(
            source_file.namespace(),
            index,
            &source_file.declarations[index],
        )
    })
}

/// The error for an AST the typed read cannot read
/// (`modelfile-load-unreadable`): an `IllegalModelException` naming the
/// file. With BC-19's shape check on such an AST is rejected first, so
/// only a caller that skips the check meets it.
pub(crate) fn unreadable_ast(err: &serde_json::Error, file_name: Option<&str>) -> Error {
    let mut contract = ContractError::new(
        ErrorKind::IllegalModel,
        "modelfile-load-unreadable",
        vec![("message", err.to_string())],
    );
    contract.model_file = Some(file_name.map(str::to_string));
    contract.into()
}

/// [`ModelFile::ast`]: the AST as a `serde_json::Value`, either given
/// directly or parsed on first use from the source the typed AST path
/// read ([`ModelFile::from_json_text`]).
#[derive(Clone)]
struct Ast {
    value: OnceLock<serde_json::Value>,
    /// What `value` is parsed from on first use, when it was not given
    /// (one sum type where three independent fields could disagree).
    source: AstSource,
}

/// Where an [`Ast`]'s value comes from when it was not given directly.
#[derive(Clone)]
enum AstSource {
    /// The value was given (or nothing else is kept).
    None,
    /// The JSON text the typed AST path read.
    Text(Arc<str>),
    /// The AST in the compact binary layout
    /// ([`ModelFile::from_compact_with_imports`]).
    #[cfg_attr(not(feature = "js-compat"), allow(dead_code))]
    Compact(Arc<[u8]>),
}

impl Ast {
    fn from_value(value: serde_json::Value) -> Self {
        Self {
            value: OnceLock::from(value),
            source: AstSource::None,
        }
    }

    fn from_text(text: Arc<str>) -> Self {
        Self {
            value: OnceLock::new(),
            source: AstSource::Text(text),
        }
    }

    #[cfg(feature = "js-compat")]
    fn from_compact(bytes: &[u8]) -> Self {
        Self {
            value: OnceLock::new(),
            source: AstSource::Compact(Arc::from(bytes)),
        }
    }

    /// The kept JSON text, if the AST was read from text.
    #[cfg(feature = "js-compat")]
    fn text(&self) -> Option<&Arc<str>> {
        match &self.source {
            AstSource::Text(text) => Some(text),
            AstSource::None | AstSource::Compact(_) => None,
        }
    }

    fn get(&self) -> &serde_json::Value {
        self.value.get_or_init(|| match &self.source {
            #[cfg(feature = "js-compat")]
            AstSource::Compact(bytes) => crate::introspect::compact::to_value(bytes)
                // The typed read checks every byte as `to_value` does,
                // a value it skips included (`Compact::skip`).
                .expect("the typed read accepted these bytes, so they are in the layout"),
            // The typed path only accepts text that also parses as a `Value`
            // (typed_ast's module doc, "JSON syntax").
            AstSource::Text(text) => serde_json::from_str(text)
                .expect("the typed AST path accepted this text, so it is JSON"),
            #[cfg(not(feature = "js-compat"))]
            AstSource::Compact(_) => serde_json::Value::Null,
            AstSource::None => serde_json::Value::Null,
        })
    }
}

/// The value when it has been parsed; otherwise only the source's kind and
/// length, so that `{:?}` on a model file (or a manager) never parses and
/// keeps a lazily kept AST.
impl std::fmt::Debug for Ast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.value.get(), &self.source) {
            (Some(value), _) => value.fmt(f),
            (None, AstSource::Text(text)) => write!(f, "<JSON text, {} bytes>", text.len()),
            (None, AstSource::Compact(bytes)) => {
                write!(f, "<compact AST, {} bytes>", bytes.len())
            }
            (None, AstSource::None) => f.write_str("null"),
        }
    }
}

/// The system import every non-system model file gets implicitly (TS:
/// `ModelFile.fromAst`).
fn built_in_import() -> serde_json::Value {
    // TS: `fromAst`'s object literal, in its own key order.
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "concerto@1.0.0",
        "types": ["Concept", "Asset", "Transaction", "Participant", "Event"]
    })
}

/// [`built_in_import`], read once (every non-system model load appends
/// it), as the whole import list of a file that imports nothing else.
static BUILT_IN_IMPORTS: LazyLock<Option<[Import; 1]>> = LazyLock::new(|| {
    Import::try_from(&built_in_import())
        .ok()
        .map(|import| [import])
});

/// [`built_in_import`] as an [`Import`]: the cached copy, or, if it could
/// not be read (it always can), the error reading it gives.
fn built_in_import_typed() -> Result<Import> {
    match &*BUILT_IN_IMPORTS {
        Some([import]) => Ok(import.clone()),
        None => Import::try_from(&built_in_import()),
    }
}

/// TS `ModelFile.importShortNames`' shape: a local name to the import (its
/// index in the file's imports) and the position in that import's
/// `imported_names` it stands for.
///
/// Keyed by names from user models, so hashed with the per-process seeded
/// foldhash ([`FastSeededState`]; see [`name_hash`]).
type ImportShortNames = FastSeededHashMap<Box<str>, (u32, u32)>;

/// TS `ModelFile.fromAst`'s `importShortNames.set` loop: one forward pass
/// over `imports`, so the last import of a local name wins.
fn import_short_names(imports: &[Import]) -> ImportShortNames {
    let mut map = ImportShortNames::default();
    for (index, import) in imports.iter().enumerate() {
        for (position, local) in import.local_names().into_iter().enumerate() {
            map.insert(local.into(), (index as u32, position as u32));
        }
    }
    map
}

/// [`import_short_names`] of [`built_in_imports`]' shared list, built once.
static BUILT_IN_SHORT_NAMES: LazyLock<ImportShortNames> = LazyLock::new(|| {
    BUILT_IN_IMPORTS
        .as_ref()
        .map(|imports| import_short_names(imports))
        .unwrap_or_default()
});

/// The imports of a non-system file that imports nothing else: the cached
/// built-in import alone, shared, or, if it could not be read (it always
/// can), the error reading it gives.
fn built_in_imports() -> Result<Cow<'static, [Import]>> {
    match &*BUILT_IN_IMPORTS {
        Some(imports) => Ok(Cow::Borrowed(imports)),
        None => Import::try_from(&built_in_import()).map(|import| Cow::Owned(vec![import])),
    }
}

impl ModelFile {
    /// TS `JSON.stringify(this.imports)`: the AST's own import nodes,
    /// verbatim, then the built-in import `fromAst` appends for a non-system
    /// file.
    fn imports_json(&self) -> String {
        let mut values = match self.ast().get("imports") {
            Some(serde_json::Value::Array(imports)) => imports.clone(),
            _ => Vec::new(),
        };
        if values.len() < self.imports.len() {
            values.push(built_in_import());
        }
        serde_json::Value::Array(values).to_string()
    }
}

/// TS `ModelFile.isCompatibleVersion`: a declared `concertoVersion` range
/// must admit this runtime's version (with prereleases) or a v3 model;
/// anything else is a plain `Error`. The range grammar is node-semver's
/// ([`crate::semver_range::satisfies`]), not Cargo's, which disagrees on
/// space-separated comparators, hyphen ranges and a bare version.
fn check_compatible_version(value: Option<&serde_json::Value>) -> Result<Option<String>> {
    let Some(range) = value.and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    compatible_concerto_version(range).map(Some)
}

js_compat_pub! {
    /// TS `ModelFile.isCompatibleVersion` for a non-empty `concertoVersion`
    /// range: the range, when this runtime's version satisfies it
    /// (prereleases included) or it admits a v3 model, otherwise the plain
    /// `Error` TS throws.
    pub fn compatible_concerto_version(range: &str) -> Result<String> {
        if crate::semver_range::satisfies(CONCERTO_CORE_VERSION, range, true)
            || crate::semver_range::satisfies("3.0.0", range, false)
        {
            return Ok(range.to_string());
        }
        Err(incompatible_concerto_version(range))
    }
}

js_compat_pub! {
    /// The plain `Error` `ModelFile.isCompatibleVersion` throws for a
    /// `concertoVersion` range this runtime does not support; `range` is the
    /// range as JS `String()` renders it.
    pub fn incompatible_concerto_version(range: &str) -> Error {
        plain_error(format!(
            "This version of Concerto supports a language version of v3.0.0 or greater, but this model is for {range}"
        ))
    }
}

/// TS `packageJson.version`: the frozen TS 5.0.0 reference's own
/// version, which `ModelFile.isCompatibleVersion` checks a model's
/// `concertoVersion` range against.
const CONCERTO_CORE_VERSION: &str = "5.0.0";

/// A plain JS `Error(message)` (`ErrorKind::InvalidArgument`), for the several
/// hardcoded, non-catalogue messages `ModelFile.fromAst`/`isCompatibleVersion`
/// throw this way rather than as an `IllegalModelException`.
fn plain_error(message: String) -> Error {
    ContractError::pre_port(ErrorKind::InvalidArgument, message, None).into()
}

/// `ModelFile.fromAst`'s namespace handling: parses `namespace` as
/// `ModelUtil.parseNamespace` does (an unversioned namespace gets this
/// function's errors, in TS 5.0.0's order), rejects a part that is not a
/// valid identifier (`IllegalModelException`, naming the file only: a
/// `ModelFile` keeps no AST `location`), then requires a version, for
/// every model file (BC-02). Returns the version.
fn parse_namespace_version<'a>(namespace: &'a str, file_name: &Option<String>) -> Result<&'a str> {
    let (name, version) = model_util::split_namespace(namespace)?;
    for part in name.split('.') {
        if !is_valid_identifier(part) {
            // A `ContractError`, so `final_message` decorates it as
            // `IllegalModelException`'s constructor does, with
            // `File '<name>': ` when the file has a name.
            let mut err = ContractError::pre_port(
                ErrorKind::IllegalModel,
                format!("Invalid namespace part '{part}'"),
                None,
            );
            err.model_file = Some(file_name.clone());
            return Err(err.into());
        }
    }
    if version.is_none() {
        return Err(plain_error(format!(
            "Cannot create a ModelFile with an unversioned namespace: {namespace}. All \
             models must specify a version (e.g., @1.0.0)."
        )));
    }
    Ok(version.unwrap_or_default())
}

/// Stamps this file's name onto an `IllegalModel` error that came up while
/// parsing one of its declarations, so the message points somewhere useful.
/// Only a pre-port `IllegalModel` error ([`Error::illegal_model`]) that
/// names no file yet is stamped.
fn annotate(mut err: Error, file_name: &Option<String>) -> Error {
    let contract = err.contract_mut();
    if contract.kind == ErrorKind::IllegalModel
        && contract.code == "pre-port"
        && contract.model_file.is_none()
    {
        contract.model_file = file_name.clone().map(Some);
    }
    err
}

#[cfg(test)]
mod tests;
