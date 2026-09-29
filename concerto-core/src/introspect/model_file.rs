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

use std::sync::{Arc, OnceLock};

use indexmap::IndexMap;

use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::declaration::{ClassDeclaration, Declaration};
use crate::introspect::decorator::{Decorated, Decorator, null_decorator, parse_decorators};
use crate::introspect::import::Import;
use crate::model_util::{self, is_primitive_type, is_valid_identifier, qualify, short_name};

/// A parsed model file for one namespace.
#[derive(Debug, Clone)]
pub struct ModelFile {
    namespace: String,
    version: String,
    imports: Vec<Import>,
    declarations: Vec<Declaration>,
    /// Declaration names to their index, for `getLocalType` (FxHash,
    /// P5-13: only ever looked up, never iterated).
    local_types: rustc_hash::FxHashMap<String, usize>,
    file_name: Option<String>,
    ast: Ast,
    decorators: Vec<Decorator>,
    /// TS `ModelFile.concertoVersion`: the AST's own `concertoVersion` range
    /// (e.g. `"^3.0.0"`), once [`check_compatible_version`] has checked it
    /// against this runtime, or `None` when the AST carries none at all
    /// (`this.concertoVersion` stays its constructor default, `null`).
    concerto_version: Option<String>,
    /// TS `ModelFile.definitions`: the optional CTO source text a caller
    /// supplied alongside the AST — kept verbatim, never parsed or produced
    /// here (CTO parsing is `concerto-cto`, out of scope: PORTING.md 1.1).
    /// [`ModelFile::from_json`] always leaves this `None`; use
    /// [`ModelFile::from_json_with_definitions`] to set it.
    definitions: Option<String>,
    /// TS `ModelFile.external`: `true` when [`ModelFile::file_name`] starts
    /// with `@` — a model downloaded from an external URI rather than one
    /// given directly (`fileName.startsWith('@')`, the constructor).
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
    /// for `ModelFile::get_definitions` — never parsed or checked against
    /// `value` here (CTO parsing is `concerto-cto`, out of scope).
    pub fn from_json_with_definitions(
        value: &serde_json::Value,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<Self> {
        let mut model_file = Self::load(value, None, definitions, file_name)?;
        model_file.ast = Ast::from_value(value.clone());
        Ok(model_file)
    }

    /// [`ModelFile::from_json_with_definitions`], taking ownership of the
    /// AST so it is kept without being copied (P5-06: a caller that has just
    /// parsed the AST from JSON text, such as the WASM binding, has no other
    /// use for it). Same result, same errors, in the same order.
    pub fn from_owned_json_with_definitions(
        value: serde_json::Value,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<Self> {
        let mut model_file = Self::load(&value, None, definitions, file_name)?;
        model_file.ast = Ast::from_value(value);
        Ok(model_file)
    }

    /// [`ModelFile::from_json_with_definitions`] for an AST given as JSON
    /// text (P5-06c): the same result, and the same errors in the same
    /// order, as parsing `text` into a `serde_json::Value` and loading that.
    /// The outer `Err` is the parse error for text that is not JSON; the
    /// inner result is the load's.
    ///
    /// It first tries the typed AST path, which reads the text straight into
    /// the typed model without a `Value` for the whole document (module doc
    /// of `typed_ast`); [`ModelFile::ast`] is then parsed from the kept text
    /// on first use. Whenever the typed path does not succeed, for any
    /// reason, its result is discarded and the text goes through the `Value`
    /// path, which reports every error.
    pub fn from_json_text(
        text: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<Self>, serde_json::Error> {
        Ok(Self::load_text_with_imports(text, definitions, file_name)?
            .map(|(model_file, _)| model_file))
    }

    /// [`ModelFile::from_json_text`], also returning the AST's own
    /// `imports` node exactly as the text holds it (`None` when the AST has
    /// no `imports` key), so a caller that needs it has no second decode of
    /// the text (P5-28, accordproject/concerto-rust#333: the WASM binding's
    /// `stageModelFileWithHeader` reads the TS `ModelFile` header from it).
    /// Same result, same errors in the same order.
    ///
    /// Behind `js-compat`, like the rest of the seam concerto-wasm builds
    /// on, so it stays out of the default (D11) public surface
    /// (docs/public-api.md sections 2.1 and 4.6).
    #[cfg(feature = "js-compat")]
    pub fn from_json_text_with_imports(
        text: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        Self::load_text_with_imports(text, definitions, file_name)
    }

    /// The body of [`ModelFile::from_json_text`] and of the `js-compat`
    /// `from_json_text_with_imports`: the loaded file plus the AST's own
    /// `imports` node.
    fn load_text_with_imports(
        text: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<Result<(Self, Option<serde_json::Value>)>, serde_json::Error> {
        if let Some(model) = crate::introspect::typed_ast::parse(text)
            && let Ok(mut model_file) = Self::load(
                &model.header,
                Some(model.declarations),
                definitions.clone(),
                file_name.clone(),
            )
        {
            model_file.ast = Ast::from_text(text);
            let imports = match model.header {
                serde_json::Value::Object(mut header) => header.remove("imports"),
                _ => None,
            };
            return Ok(Ok((model_file, imports)));
        }
        let value: serde_json::Value = serde_json::from_str(text)?;
        let imports = value.get("imports").cloned();
        Ok(
            Self::from_owned_json_with_definitions(value, definitions, file_name)
                .map(|model_file| (model_file, imports)),
        )
    }

    /// The body of [`ModelFile::from_json_with_definitions`], leaving
    /// [`ModelFile::ast`] `Null` for the caller to fill in. `typed` is the
    /// declarations the typed AST path has already read
    /// ([`ModelFile::from_json_text`]), in place of `value`'s own
    /// `declarations` (which `value` then does not have).
    fn load(
        value: &serde_json::Value,
        typed: Option<Vec<crate::introspect::typed_ast::TypedDeclaration>>,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<Self> {
        // TS: the constructor's `this.process()` (`Decorated.process`, the
        // model file's own decorators) runs before `fromAst`. DV-018: a
        // `null` decorator node is an `IllegalModelException` naming this
        // file, where TS crashes (`null_decorator`).
        if let Some(mut err) = null_decorator(value) {
            err.model_file = Some(file_name.clone());
            return Err(err.into());
        }

        let namespace = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::illegal_model("model missing 'namespace'", file_name.clone(), None)
            })?
            .to_string();

        // TS: `ModelFile.fromAst`'s own namespace handling (modelfile.ts) —
        // `ModelUtil.parseNamespace` (which accepts an unversioned namespace,
        // DV-003), a check that every dot-separated part of the name is a
        // valid identifier, and then, for a non-system model file only, a
        // version requirement (P2-08). `is_system` is computed here rather
        // than reused from below, since TS's own version needs it first.
        let is_system_namespace = namespace.starts_with("concerto@") || namespace == "concerto";
        let version = parse_namespace_version(&namespace, is_system_namespace, &file_name)?;

        let mut imports = match value.get("imports") {
            None => Vec::new(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(Import::try_from)
                .collect::<Result<Vec<_>>>()?,
            Some(_) => {
                return Err(Error::illegal_model(
                    "model 'imports' must be an array",
                    file_name.clone(),
                    None,
                ));
            }
        };

        // Every non-system model file imports the system types implicitly.
        // TS: ModelFile.fromAst (src/introspect/modelfile.ts), the built-in
        // import; ported here because the trial's oracle fixtures load models
        // that use them (P0-04b).
        let is_system = is_system_namespace;
        if !is_system {
            imports.push(built_in_import_typed()?);
        }

        // TS: `ModelFile.fromAst`'s `imports.forEach` loop (modelfile.ts)
        // runs two checks over every import, including the built-in one just
        // pushed above (it is always versioned, so `enforceImportVersioning`
        // never rejects it): an aliased type's alias may not itself name a
        // primitive, and — `enforceImportVersioning` — the imported namespace
        // must carry a version. Both throw a plain `Error`, not an
        // `IllegalModelException`.
        for imp in &imports {
            for alias in imp.aliased_types() {
                if is_primitive_type(&alias.aliased_name) {
                    return Err(plain_error(
                        "Types cannot be aliased to primitive type".to_string(),
                    ));
                }
            }
            // P5-48: `parseNamespace`'s checks, without its owned result.
            let versioned = model_util::split_namespace(imp.namespace())?.1.is_some();
            if !versioned {
                return Err(plain_error(format!(
                    "Cannot use an unversioned import {}.",
                    imp.namespace()
                )));
            }
        }

        let mut declarations = Vec::new();
        let mut local_types = rustc_hash::FxHashMap::default();
        if let Some(typed) = typed {
            declarations.reserve(typed.len());
            for raw in typed {
                let decl = Declaration::from_typed(raw, &namespace, file_name.as_deref())?;
                local_types.insert(decl.name().to_string(), declarations.len());
                declarations.push(decl);
            }
        } else {
            match value.get("declarations") {
                None => {}
                Some(serde_json::Value::Array(arr)) => {
                    for raw in arr {
                        let decl =
                            Declaration::from_model_json(raw, &namespace, file_name.as_deref())
                                .map_err(|e| annotate(e, &file_name))?;
                        // TS: the constructor's `localTypes` loop is a plain
                        // `Map.set` per declaration, so a second declaration of
                        // the same name is accepted here and simply replaces the
                        // first in the lookup (the last one wins), while
                        // `getAllDeclarations()` still lists both. Only
                        // `ModelFile.validate()`'s duplicate-name scan rejects it
                        // (`ModelManager::validate_model_file`, P2-08).
                        local_types.insert(decl.name().to_string(), declarations.len());
                        declarations.push(decl);
                    }
                }
                Some(_) => {
                    return Err(Error::illegal_model(
                        "model 'declarations' must be an array",
                        file_name.clone(),
                        None,
                    ));
                }
            }
        }

        // TS: `ModelFile.isCompatibleVersion`, run from the constructor right
        // after `fromAst` has populated the imports and declarations, before
        // `localTypes` is built — so a bad declaration is still reported
        // ahead of an incompatible `concertoVersion` when a model has both.
        let concerto_version = check_compatible_version(value)?;

        let external = file_name.as_deref().is_some_and(|n| n.starts_with('@'));

        Ok(Self {
            namespace,
            version,
            imports,
            declarations,
            local_types,
            file_name,
            decorators: parse_decorators(value),
            ast: Ast::from_value(serde_json::Value::Null),
            concerto_version,
            definitions,
            external,
        })
    }

    js_compat_pub! {
        /// TS: the argument checks `new ModelFile(modelManager, ast, definitions,
        /// fileName)` runs before it reads the AST at all, for a caller that
        /// holds arbitrary JS values rather than this crate's typed arguments (a
        /// binding, or the oracle harness). Each argument is `None` for JS
        /// `undefined`. In TS's order, each a plain `Error`:
        /// `Decorated`'s constructor rejects a falsy `ast` (`ast not
        /// specified`); `ModelFile`'s rejects an `ast` that is not an object,
        /// then a truthy `definitions` that is not a string, then a truthy
        /// `fileName` that is not a string (P2-08).
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
        &self.version
    }

    /// The JSON AST this model file was built from, exactly as it was given.
    pub fn ast(&self) -> &serde_json::Value {
        self.ast.get()
    }

    /// Whether this file was built by the typed AST path.
    #[cfg(test)]
    pub(crate) fn built_by_typed_path(&self) -> bool {
        self.ast.text.is_some()
    }

    /// Whether this file and `other` were built from equal ASTs
    /// (`self.ast() == other.ast()`), without parsing either one's kept
    /// text when both were built from the same text
    /// ([`ModelFile::from_json_text`]).
    pub(crate) fn same_ast(&self, other: &ModelFile) -> bool {
        if let (Some(a), Some(b)) = (&self.ast.text, &other.ast.text)
            && a == b
        {
            return true;
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
        self.local_types.get(short).copied()
    }

    /// True if this is the built-in `concerto` system namespace.
    pub fn is_system_namespace(&self) -> bool {
        self.namespace.starts_with("concerto@")
    }

    /// Resolves a short name from what this file declares or imports: the
    /// primitives, its named imports, and its own declarations. Returns
    /// `None` if the name is none of those.
    ///
    /// Imports are checked before local declarations, matching TS
    /// `ModelFile.getType`/`resolveType`'s own `isImportedType(type) ? … :
    /// isLocalType(type) ? … : null` order (modelfile.ts). A name is
    /// normally never both — the "clashes with an imported type" check
    /// (`Declaration.validate`) rejects a local declaration that shares a
    /// name with an import — except when
    /// `dangerouslyAllowReservedSystemTypeNamesInUserModels` waives that
    /// check for a name that also matches a reserved system declaration
    /// (P2-08): a local `Asset` importing the system `Asset` implicitly
    /// (every non-system file's built-in import) must still resolve its own
    /// implicit `superType` of `Asset` to the *system* declaration, not to
    /// itself, or loading it would see circular inheritance.
    pub fn resolve_local_type(&self, short: &str) -> Option<String> {
        if is_primitive_type(short) {
            return Some(short.to_string());
        }
        if let Some(fqn) = self.imports.iter().find_map(|imp| imp.resolve(short)) {
            return Some(fqn);
        }
        if self.local_types.contains_key(short) {
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

    /// TS: `ModelFile.getImportURI` — the URI an import was given (`import
    /// ns.Name from 'uri'`), keyed the same odd way TS's own `importUriMap`
    /// is: by the *first* fully-qualified name the owning import brings in,
    /// not by its bare namespace (`ModelUtil.importFullyQualifiedNames(imp)[0]`,
    /// modelfile.ts). `None` if no import with that key carries a URI.
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
    /// Returned in import order, matching TS's `importUriMap`: a plain
    /// object built by assigning `importUriMap[key] = uri` for each import
    /// in file order, so JS keeps insertion order and a later duplicate key
    /// overwrites the value in place without moving it (PORTING.md 3.7 —
    /// no `HashMap` iteration on an observable path).
    pub fn external_imports(&self) -> IndexMap<String, String> {
        let mut out = IndexMap::new();
        for imp in &self.imports {
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

    /// TS: `ModelFile.getImports` — the fully-qualified names this file
    /// imports (the declared name of each, never an alias, matching
    /// `ModelUtil.importFullyQualifiedNames`), including the built-in system
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
        let short = type_name
            .strip_prefix(self.namespace.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
            .unwrap_or(type_name);
        self.local_declaration(short)
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

    /// The fully-qualified name a locally-visible import name resolves to
    /// (an alias counts under its alias only, not its declared name — P2-08
    /// review carry-over (a) from P2-04's review, #48). Later imports
    /// overwrite earlier ones for the same local name, as `Map.set` does
    /// (TS builds `importShortNames` with one forward pass over `this.imports`).
    fn find_import(&self, type_name: &str) -> Option<String> {
        self.imports.iter().rev().find_map(|imp| {
            imp.local_names()
                .into_iter()
                .zip(imp.imported_names())
                .rfind(|(local, _)| *local == type_name)
                .map(|(_, imported)| qualify(imp.namespace(), imported))
        })
    }

    /// TS: `ModelFile.isImportedType`.
    pub fn is_imported_type(&self, type_name: &str) -> bool {
        self.find_import(type_name).is_some()
    }

    /// TS: `ModelFile.resolveImport`. The error, when `type_name` is not
    /// visible under any import, carries this file's own name for the
    /// `IllegalModelException` message's `File '…':` decoration, the same as
    /// every check in [`crate::validation`] does; its `imports` parameter is
    /// TS's `JSON.stringify(this.imports)` (`ModelFile::imports_json`).
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

    /// TS: `ModelFile.getFullyQualifiedTypeName` — entirely local: a
    /// primitive's own name, an imported name's target FQN, or a locally
    /// declared type's FQN; `None` (TS `null`) when `type_name` is none of
    /// those. [`crate::model_manager::ModelManager`]'s `ResolutionContext`
    /// implementation already serves `ModelFile.getType` and
    /// `Property.getFullyQualifiedTypeName`, the two members that need the
    /// owning `ModelManager` to chase into another file; this one never does.
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

    /// TS: `ModelFile.getClassDeclarations` — `instanceof ClassDeclaration`,
    /// which `EnumDeclaration` also satisfies (it extends `ClassDeclaration`
    /// in TS, module doc on [`crate::introspect::declaration::EnumDeclaration`]);
    /// only a map or scalar declaration is left out. The same predicate
    /// `crate::model_manager::ModelManager::class_declarations`
    /// (`Introspector.getClassDeclarations`) uses.
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

    /// TS: `ModelFile.filter` — a new model file with only the declarations
    /// `predicate` accepts, or `None` (TS `null`) if that leaves none. The
    /// predicate also decides which of this file's imports survive: an
    /// import is dropped only when every declaration it would have brought
    /// in is rejected (an `ImportType`) or all of its named types are (an
    /// `ImportTypes`, whose surviving `types`/`aliasedTypes` are pruned the
    /// same way TS's own `imp.types.filter`/`imp.aliasedTypes.filter` are);
    /// the built-in `concerto` import always survives. `source_manager` is
    /// the manager this file is currently loaded into — TS reads each
    /// import's source file through `this.getModelManager()` — and need not
    /// be the same manager the filtered file is later added to.
    pub fn filter(
        &self,
        predicate: impl Fn(&Declaration) -> bool,
        source_manager: &crate::model_manager::ModelManager,
    ) -> Result<Option<Self>> {
        let declarations: Vec<serde_json::Value> = self
            .ast()
            .get("declarations")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .zip(&self.declarations)
            .filter(|(_, decl)| predicate(decl))
            .map(|(ast, _)| ast.clone())
            .collect();

        if declarations.is_empty() {
            return Ok(None);
        }

        let mut filtered = self.ast().clone();
        filtered["declarations"] = serde_json::Value::Array(declarations);

        if let Some(imports) = self
            .ast()
            .get("imports")
            .and_then(|v| v.as_array())
            .cloned()
        {
            let kept: Vec<serde_json::Value> = imports
                .into_iter()
                .filter_map(|mut imp| {
                    let namespace = imp.get("namespace").and_then(|v| v.as_str())?.to_string();
                    if namespace.starts_with("concerto@") || namespace == "concerto" {
                        return Some(imp);
                    }
                    let short_class =
                        short_name(imp.get("$class").and_then(|v| v.as_str()).unwrap_or(""));
                    let source_file = source_manager.model_file(&namespace);
                    match short_class {
                        "ImportType" => {
                            let name = imp.get("name").and_then(|v| v.as_str())?;
                            let keep = source_file
                                .is_none_or(|sf| sf.local_type(name).is_none_or(&predicate));
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
                                .filter(|name| sf.local_type(name).is_none_or(&predicate))
                                .map(str::to_string)
                                .collect();
                            if kept_types.is_empty() {
                                return None;
                            }
                            if let Some(aliased) =
                                imp.get("aliasedTypes").and_then(|v| v.as_array()).cloned()
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
                })
                .collect();
            filtered["imports"] = serde_json::Value::Array(kept);
        }

        Self::from_json_with_definitions(
            &filtered,
            self.definitions.clone(),
            self.file_name.clone(),
        )
        .map(Some)
    }
}

/// [`ModelFile::ast`]: the AST as a `serde_json::Value`, either given
/// directly or parsed on first use from the JSON text the typed AST path
/// read (P5-06c, [`ModelFile::from_json_text`]).
#[derive(Clone)]
struct Ast {
    value: OnceLock<serde_json::Value>,
    text: Option<Arc<str>>,
}

impl Ast {
    fn from_value(value: serde_json::Value) -> Self {
        Self {
            value: OnceLock::from(value),
            text: None,
        }
    }

    fn from_text(text: &str) -> Self {
        Self {
            value: OnceLock::new(),
            text: Some(Arc::from(text)),
        }
    }

    fn get(&self) -> &serde_json::Value {
        self.value.get_or_init(|| {
            // The typed path only accepts text that also parses as a `Value`
            // (typed_ast's module doc, "JSON syntax").
            serde_json::from_str(self.text.as_deref().unwrap_or("null"))
                .expect("the typed AST path accepted this text, so it is JSON")
        })
    }
}

impl std::fmt::Debug for Ast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
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

thread_local! {
    /// [`built_in_import`], read once per thread (P5-48: every non-system
    /// model load appends it).
    static BUILT_IN_IMPORT: Option<Import> = Import::try_from(&built_in_import()).ok();
}

/// [`built_in_import`] as an [`Import`]: the cached copy, or, if it could
/// not be read (it always can), the error reading it gives.
fn built_in_import_typed() -> Result<Import> {
    match BUILT_IN_IMPORT.with(Clone::clone) {
        Some(import) => Ok(import),
        None => Import::try_from(&built_in_import()),
    }
}

impl ModelFile {
    /// TS `JSON.stringify(this.imports)`: the AST's own import nodes,
    /// verbatim and in their own key order (the AST is kept unchanged,
    /// module doc), followed by the built-in system import `fromAst` appends
    /// for a non-system file (P2-08 review: this used to re-encode the typed
    /// imports, which carry no `$class`).
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

/// TS `ModelFile.isCompatibleVersion` (modelfile.ts): if the AST declares a
/// `concertoVersion` range, this runtime's own version (D10: the frozen TS
/// 5.0.0 reference) must satisfy it (`semver.satisfies(…, {includePrerelease:
/// true})`); failing that, a model still targeting v3.0.0 or later is
/// accepted for backward compatibility (`semver.minSatisfying(['3.0.0'],
/// range)`, no options); anything else is a plain `Error`, not an
/// `IllegalModelException`. `None` (not an error) when the AST carries no
/// `concertoVersion` at all.
///
/// The range check is [`crate::semver_range::satisfies`], a port of
/// node-semver's own range grammar (module doc there), not the Cargo
/// `semver` crate's requirement syntax: the two disagree on space-separated
/// AND comparators (`>=3.0.0 <6.0.0`), hyphen ranges (`1.2.3 - 2.3.4`) and
/// what a bare version means (exact in node-semver, caret in Cargo).
fn check_compatible_version(value: &serde_json::Value) -> Result<Option<String>> {
    let Some(range) = value
        .get("concertoVersion")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    compatible_concerto_version(range).map(Some)
}

js_compat_pub! {
    /// TS `ModelFile.isCompatibleVersion` for a non-empty `concertoVersion`
    /// range (P5-11, accordproject/concerto-rust#287): the range, when this
    /// runtime's version satisfies it (prereleases included) or it admits
    /// a v3 model, otherwise the plain `Error` TS throws.
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

/// TS `packageJson.version`: the frozen TS 5.0.0 reference's own version
/// (D10), which `ModelFile.isCompatibleVersion` checks a model's
/// `concertoVersion` range against.
const CONCERTO_CORE_VERSION: &str = "5.0.0";

/// A plain JS `Error(message)` (`ErrorKind::InvalidArgument`), for the several
/// hardcoded, non-catalogue messages `ModelFile.fromAst`/`isCompatibleVersion`
/// throw this way rather than as an `IllegalModelException`.
fn plain_error(message: String) -> Error {
    ContractError::pre_port(ErrorKind::InvalidArgument, message, None).into()
}

/// `ModelFile.fromAst`'s own namespace handling (modelfile.ts, P2-08): parses
/// `namespace` with `ModelUtil.parseNamespace` (`model_util::parse_namespace`,
/// which accepts an unversioned namespace and returns `version: None`,
/// DV-003), rejects a namespace whose name has a part that is not a valid
/// identifier (`IllegalModelException`, `this` and `this.ast.location` in TS —
/// no oracle fixture reaches this branch, and `ModelFile` keeps no AST
/// `location` in this port (validation.rs review comment), so only the file
/// name is attached here), then — for a non-system model file only —
/// requires a version, with the same plain `Error` TS's own hardcoded
/// message uses. Returns the version (`""` for none, as every unversioned
/// caller here is a system model file).
fn parse_namespace_version(
    namespace: &str,
    is_system: bool,
    file_name: &Option<String>,
) -> Result<String> {
    let (name, version) = match model_util::parse_namespace_with(Some(namespace), false)? {
        model_util::ParsedNamespace::Full { name, version, .. } => (name, version),
        model_util::ParsedNamespace::NameOnly { name } => (name, None),
    };
    for part in name.split('.') {
        if !is_valid_identifier(part) {
            // `ContractError` (not the pre-port `Error::illegal_model`
            // shape), so the oracle harness's `final_message`
            // decorates it exactly as `IllegalModelException`'s constructor
            // does (`to_oracle_error`'s doc comment): a trailing space always,
            // and `File '<name>': ` when TS's `this` (passed here, unlike
            // `plain_error`, below) has one.
            let mut err = ContractError::pre_port(
                ErrorKind::IllegalModel,
                format!("Invalid namespace part '{part}'"),
                None,
            );
            err.model_file = Some(file_name.clone());
            return Err(err.into());
        }
    }
    if version.is_none() && !is_system {
        return Err(plain_error(format!(
            "Cannot create a ModelFile with an unversioned namespace: {namespace}. All \
             models must specify a version (e.g., @1.0.0)."
        )));
    }
    Ok(version.unwrap_or_default())
}

/// Stamps this file's name onto an `IllegalModel` error that came up while
/// parsing one of its declarations, so the message points somewhere useful.
fn annotate(err: Error, file_name: &Option<String>) -> Error {
    match err.unported_illegal_model() {
        Some(message) => {
            Error::illegal_model(message, file_name.clone(), err.contract().location.clone())
        }
        None => err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ModelFile {
        ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.common@1.0.0", "name": "Address" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                      "name": "Person", "isAbstract": false, "properties": [] }
                ]
            }),
            Some("example.cto".into()),
        )
        .unwrap()
    }

    #[test]
    fn parses_namespace_imports_and_declarations() {
        let mf = sample();
        assert_eq!(mf.namespace(), "org.example@1.0.0");
        assert_eq!(mf.version(), "1.0.0");
        assert_eq!(mf.declarations().len(), 1);
        // The declared import, then the built-in import of the system types.
        assert_eq!(mf.imports().len(), 2);
        assert_eq!(mf.imports()[1].namespace(), "concerto@1.0.0");
        assert!(mf.local_declaration("Person").is_some());
        assert!(!mf.is_system_namespace());
    }

    #[test]
    fn resolves_local_primitive_and_import() {
        let mf = sample();
        assert_eq!(
            mf.resolve_local_type("Person").as_deref(),
            Some("org.example@1.0.0.Person")
        );
        assert_eq!(mf.resolve_local_type("String").as_deref(), Some("String"));
        assert_eq!(
            mf.resolve_local_type("Address").as_deref(),
            Some("org.common@1.0.0.Address")
        );
        assert_eq!(mf.resolve_local_type("Missing"), None);
    }

    /// TS: test/introspect/modelfile.js #constructor "should throw when null
    /// ast provided" / "non object ast" / "invalid definitions" / "invalid
    /// filename" — each a plain `Error`, checked in TS's order.
    #[test]
    fn constructor_arguments_are_checked_in_ts_order() {
        use serde_json::json;
        let message = |r: Result<()>| match r.unwrap_err().into_ported() {
            Some(c) => {
                assert_eq!(c.kind, ErrorKind::InvalidArgument);
                c.message()
            }
            other => panic!("expected a plain Error, got {other:?}"),
        };
        let ast = json!({ "namespace": "org.acme@1.0.0" });
        assert_eq!(
            message(ModelFile::check_constructor_arguments(
                Some(&json!(null)),
                None,
                None
            )),
            "ast not specified"
        );
        assert_eq!(
            message(ModelFile::check_constructor_arguments(
                None,
                Some(&json!({})),
                None
            )),
            "ast not specified"
        );
        assert_eq!(
            message(ModelFile::check_constructor_arguments(
                Some(&json!(true)),
                None,
                None
            )),
            "ModelFile expects a Concerto model AST as input."
        );
        assert_eq!(
            message(ModelFile::check_constructor_arguments(
                Some(&ast),
                Some(&json!({})),
                Some(&json!({}))
            )),
            "ModelFile expects an (optional) Concerto model definition as a string."
        );
        assert_eq!(
            message(ModelFile::check_constructor_arguments(
                Some(&ast),
                None,
                Some(&json!({}))
            )),
            "ModelFile expects an (optional) filename as a string."
        );
        // Falsy non-strings are ignored, as TS's `definitions && …` is.
        ModelFile::check_constructor_arguments(Some(&ast), Some(&json!(null)), Some(&json!("")))
            .unwrap();
        ModelFile::check_constructor_arguments(
            Some(&ast),
            Some(&json!("cto")),
            Some(&json!("a.cto")),
        )
        .unwrap();
    }

    /// TS: `ClassDeclaration.process` rejects a system property name with
    /// the declaration's own `ast.location` and the model file's name.
    #[test]
    fn a_system_property_name_is_rejected_with_the_declaration_location() {
        let location = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Range",
            "start": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 55, "line": 3, "column": 1 },
            "end": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 103, "line": 5, "column": 2 }
        });
        let err = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "C", "isAbstract": false, "location": location,
                    "properties": [
                        { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "$class",
                          "isArray": false, "isOptional": false }
                    ]
                }]
            }),
            Some("c.cto".into()),
        )
        .unwrap_err();
        let Some(err) = err.ported().cloned() else {
            panic!("expected a contract error, got {err:?}");
        };
        assert_eq!(err.location, Some(location));
        assert_eq!(
            err.final_message(),
            "Invalid field name '$class' File 'c.cto': line 3 column 1, to line 5 column 2. "
        );
    }

    /// TS's `ModelFile` constructor accepts two declarations of one name:
    /// both stay in `getAllDeclarations()`, and the `localTypes` lookup keeps
    /// the last (a `Map.set` per declaration). Rejecting the duplicate is
    /// `ModelFile.validate()`'s job (P2-08 review: this test used to assert
    /// that construction itself failed, which TS never does).
    #[test]
    #[allow(deprecated)]
    fn duplicate_declaration_is_accepted_at_construction_and_the_last_wins() {
        let mf = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.dup@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "A", "isAbstract": false, "properties": [] },
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "A", "isAbstract": false, "properties": [] }
                ]
            }),
            None,
        )
        .expect("TS's constructor accepts a duplicate declaration name");
        assert_eq!(mf.declarations().len(), 2);
        assert_eq!(mf.local_index("A"), Some(1));
        assert!(mf.get_asset_declaration("A").is_some());
    }

    #[test]
    fn missing_namespace_is_rejected() {
        let err = ModelFile::from_json(
            &serde_json::json!({ "$class": "concerto.metamodel@1.0.0.Model" }),
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn unversioned_namespace_is_rejected() {
        let err = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example",
                "declarations": []
            }),
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn non_array_declarations_or_imports_is_rejected() {
        let bad_decls = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.x@1.0.0",
                "declarations": { "not": "an array" }
            }),
            None,
        );
        assert!(bad_decls.is_err());

        let bad_imports = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.x@1.0.0",
                "imports": "nope"
            }),
            None,
        );
        assert!(bad_imports.is_err());
    }

    #[test]
    fn keeps_the_ast_it_was_given_in_its_original_key_order() {
        let text = r#"{"namespace":"org.order@1.0.0","$class":"concerto.metamodel@1.0.0.Model","declarations":[{"properties":[],"name":"A","$class":"concerto.metamodel@1.0.0.ConceptDeclaration","isAbstract":false,"extra":null}]}"#;
        let value: serde_json::Value = serde_json::from_str(text).unwrap();
        let mf = ModelFile::from_json(&value, None).unwrap();
        assert_eq!(mf.ast(), &value);
        assert_eq!(serde_json::to_string(mf.ast()).unwrap(), text);
    }

    // TS: test/introspect/modelfile.js `#isExternal`.
    #[test]
    fn is_external_reflects_an_at_prefixed_file_name() {
        let at_sign =
            ModelFile::from_json(&sample().ast().clone(), Some("@carlease".into())).unwrap();
        assert!(at_sign.is_external());
        let plain = ModelFile::from_json(&sample().ast().clone(), Some("carlease".into())).unwrap();
        assert!(!plain.is_external());
        assert!(!sample().is_external());
    }

    fn model_with_version(concerto_version: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.v@1.0.0",
            "concertoVersion": concerto_version,
            "declarations": []
        })
    }

    // TS: test/introspect/modelfile.js `#isCompatibleVersion`/`#getConcertoVersion`.
    #[test]
    fn a_concerto_version_satisfied_by_this_runtime_is_recorded_verbatim() {
        let mf = ModelFile::from_json(&model_with_version("^5.0.0"), None).unwrap();
        assert_eq!(mf.concerto_version(), Some("^5.0.0"));
    }

    #[test]
    fn a_v3_concerto_version_is_accepted_for_backward_compatibility() {
        let mf = ModelFile::from_json(&model_with_version("^3.0.0"), None).unwrap();
        assert_eq!(mf.concerto_version(), Some("^3.0.0"));
    }

    #[test]
    fn an_unsatisfiable_concerto_version_is_rejected() {
        let err = ModelFile::from_json(&model_with_version("^99.0.0"), None);
        let message = err.unwrap_err().to_string();
        assert!(message.contains("v3.0.0 or greater"));
        assert!(message.contains("^99.0.0"));
    }

    #[test]
    fn no_concerto_version_at_all_leaves_it_none() {
        assert_eq!(sample().concerto_version(), None);
    }

    /// TS: test/introspect/modelfile.js #resolveImport "should throw if it
    /// cannot resolve a type that is not imported": the message lists
    /// `JSON.stringify(this.imports)` — the AST's own import nodes verbatim,
    /// then the built-in system import.
    #[test]
    fn resolve_import_failure_lists_the_imports_as_ts_stringifies_them() {
        let mf = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "name": "Wow", "namespace": "org.doge@1.0.0" }
                ],
                "declarations": []
            }),
            None,
        )
        .unwrap();
        let Some(err) = mf.resolve_import("Coin").unwrap_err().into_ported() else {
            panic!("expected a contract error");
        };
        assert_eq!(
            err.final_message(),
            "Failed to find \"Coin\" in list of imports \"[[{\"$class\":\"concerto.metamodel@1.0.0.ImportType\",\"name\":\"Wow\",\"namespace\":\"org.doge@1.0.0\"},{\"$class\":\"concerto.metamodel@1.0.0.ImportTypes\",\"namespace\":\"concerto@1.0.0\",\"types\":[\"Concept\",\"Asset\",\"Transaction\",\"Participant\",\"Event\"]}]]\" for namespace \"org.acme@1.0.0\". "
        );
    }

    #[test]
    #[allow(deprecated)]
    fn resolves_and_reports_imported_types_by_their_visible_local_name() {
        let mf = sample();
        assert!(mf.is_imported_type("Address"));
        assert!(!mf.is_imported_type("Nonexistent"));
        assert_eq!(
            mf.resolve_import("Address").unwrap(),
            "org.common@1.0.0.Address"
        );
        assert_eq!(mf.get_imported_type("Address").unwrap(), "Address");
        assert!(mf.resolve_import("Nonexistent").is_err());
        assert!(mf.is_defined("Person"));
        assert!(mf.is_defined("String"));
        // TS `isDefined`: an imported-only name is not "defined" by this file.
        assert!(!mf.is_defined("Address"));
    }

    #[test]
    #[allow(deprecated)]
    fn get_imports_lists_declared_names_never_aliases() {
        let mf = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.alias@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportTypes",
                      "namespace": "org.common@1.0.0", "types": ["Address"],
                      "aliasedTypes": [
                        { "$class": "concerto.metamodel@1.0.0.AliasedType",
                          "name": "Address", "aliasedName": "Location" }
                      ] }
                ],
                "declarations": []
            }),
            None,
        )
        .unwrap();
        assert!(
            mf.get_imports()
                .contains(&"org.common@1.0.0.Address".to_string())
        );
        assert!(mf.is_imported_type("Location"));
        assert!(!mf.is_imported_type("Address"));
    }

    /// P5-28: `from_json_text_with_imports` loads the same file as
    /// `from_json_text` and returns the AST's own `imports` node verbatim,
    /// or `None` when the AST has none.
    #[test]
    fn from_json_text_with_imports_returns_the_imports_node() {
        let imports = serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.ImportType",
              "namespace": "org.common@1.0.0", "name": "Address", "uri": "u" }
        ]);
        let text = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.uri@1.0.0",
            "imports": imports,
            "declarations": []
        })
        .to_string();
        let (mf, node) = ModelFile::load_text_with_imports(&text, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(mf.namespace(), "org.uri@1.0.0");
        assert_eq!(node, Some(imports));
        let text = r#"{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.x@1.0.0"}"#;
        let (_, node) = ModelFile::load_text_with_imports(text, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(node, None);
        assert!(ModelFile::load_text_with_imports("{", None, None).is_err());
    }

    #[test]
    #[allow(deprecated)]
    fn get_import_uri_is_keyed_by_the_imports_first_fully_qualified_name() {
        let mf = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.uri@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.common@1.0.0", "name": "Address",
                      "uri": "https://example.org/common.cto" }
                ],
                "declarations": []
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            mf.get_import_uri("org.common@1.0.0.Address"),
            Some("https://example.org/common.cto")
        );
        assert_eq!(mf.get_import_uri("org.common@1.0.0"), None);
        assert_eq!(
            mf.get_external_imports().get("org.common@1.0.0.Address"),
            Some(&"https://example.org/common.cto".to_string())
        );
    }

    #[test]
    fn external_imports_preserves_import_order_for_several_uri_imports() {
        // Issue #263: `getExternalImports` must come back in import order
        // (TS builds `importUriMap` by assigning one key per import, in
        // file order), not the arbitrary order a `HashMap` would give.
        let n = 12;
        let imports: Vec<serde_json::Value> = (0..n)
            .map(|i| {
                serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.ImportType",
                    "namespace": format!("org.n{i}@1.0.0"),
                    "name": format!("T{i}"),
                    "uri": format!("https://example.com/m{i}.cto"),
                })
            })
            .collect();
        let mf = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.order@1.0.0",
                "imports": imports,
                "declarations": []
            }),
            None,
        )
        .unwrap();

        let expected: Vec<String> = (0..n).map(|i| format!("org.n{i}@1.0.0.T{i}")).collect();
        let actual: Vec<String> = mf.external_imports().keys().cloned().collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn external_imports_last_write_wins_in_place_for_a_duplicate_key() {
        // Matches TS's `importUriMap[key] = imp.uri`: assigning to an
        // existing plain-object key updates the value without moving it.
        let mf = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.dup@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.common@1.0.0", "name": "Address",
                      "uri": "https://example.org/first.cto" },
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.other@1.0.0", "name": "Thing",
                      "uri": "https://example.org/other.cto" },
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.common@1.0.0", "name": "Address",
                      "uri": "https://example.org/second.cto" }
                ],
                "declarations": []
            }),
            None,
        )
        .unwrap();

        let imports = mf.external_imports();
        assert_eq!(
            imports.keys().cloned().collect::<Vec<_>>(),
            vec![
                "org.common@1.0.0.Address".to_string(),
                "org.other@1.0.0.Thing".to_string(),
            ]
        );
        assert_eq!(
            imports.get("org.common@1.0.0.Address"),
            Some(&"https://example.org/second.cto".to_string())
        );
    }

    fn model_with_two_concepts(ns: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": ns,
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                  "name": "Keep", "isAbstract": false, "properties": [] },
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                  "name": "Drop", "isAbstract": false, "properties": [] }
            ]
        })
    }

    // TS: test/introspect/modelfile.js `#filter`.
    #[test]
    fn filter_keeps_only_matching_declarations() {
        let manager = crate::model_manager::ModelManager::new().unwrap();
        let mf = ModelFile::from_json(&model_with_two_concepts("org.f@1.0.0"), None).unwrap();
        let filtered = mf
            .filter(|d| d.name() == "Keep", &manager)
            .unwrap()
            .expect("Keep survives");
        assert_eq!(filtered.declarations().len(), 1);
        assert_eq!(filtered.declarations()[0].name(), "Keep");
    }

    #[test]
    fn filter_returns_none_when_every_declaration_is_rejected() {
        let manager = crate::model_manager::ModelManager::new().unwrap();
        let mf = ModelFile::from_json(&model_with_two_concepts("org.f2@1.0.0"), None).unwrap();
        assert!(mf.filter(|_| false, &manager).unwrap().is_none());
    }

    #[test]
    fn filter_drops_an_import_whose_only_type_is_filtered_out_of_its_source_file() {
        let mut manager = crate::model_manager::ModelManager::new().unwrap();
        manager
            .load_model(&model_with_two_concepts("org.src@1.0.0"), None)
            .unwrap();

        let importing = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.importing@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.src@1.0.0", "name": "Drop" }
            ],
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                  "name": "User", "isAbstract": false, "properties": [] }
            ]
        });
        let mf = ModelFile::from_json(&importing, None).unwrap();

        // The predicate rejects `Drop` wherever it is asked about, including
        // in the source file `org.src@1.0.0` that the import is checked
        // against — so the import of `Drop` alone is dropped entirely.
        let filtered = mf
            .filter(|d| d.name() != "Drop", &manager)
            .unwrap()
            .expect("User survives");
        assert!(
            filtered
                .ast()
                .get("imports")
                .and_then(|v| v.as_array())
                .is_none_or(Vec::is_empty)
        );
    }
}
