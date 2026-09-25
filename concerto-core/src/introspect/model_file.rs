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

use std::collections::HashMap;

use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::Named;
use crate::introspect::declaration::{ClassDeclaration, Declaration};
use crate::introspect::decorator::{Decorated, Decorator, parse_decorators};
use crate::introspect::import::Import;
use crate::model_util::{
    self, get_fully_qualified_name, get_short_name, is_primitive_type, is_valid_identifier,
};

/// A parsed model file for one namespace.
#[derive(Debug, Clone)]
pub struct ModelFile {
    namespace: String,
    version: String,
    imports: Vec<Import>,
    declarations: Vec<Declaration>,
    local_types: HashMap<String, usize>,
    file_name: Option<String>,
    ast: serde_json::Value,
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
    fn get_decorators(&self) -> &[Decorator] {
        &self.decorators
    }
}

impl ModelFile {
    /// Builds a model file from the JSON AST of a `concerto.metamodel@….Model`,
    /// with no CTO source text ([`ModelFile::get_definitions`] will answer
    /// `None`). TS: `new ModelFile(modelManager, ast, definitions, fileName)`
    /// with `definitions` omitted.
    pub fn from_json(value: &serde_json::Value, file_name: Option<String>) -> Result<Self> {
        Self::from_json_with_definitions(value, None, file_name)
    }

    /// [`ModelFile::from_json`], keeping the given CTO source text verbatim
    /// for [`ModelFile::get_definitions`] — never parsed or checked against
    /// `value` here (CTO parsing is `concerto-cto`, out of scope).
    pub fn from_json_with_definitions(
        value: &serde_json::Value,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<Self> {
        let namespace = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConcertoError::IllegalModel {
                message: "model missing 'namespace'".into(),
                file_name: file_name.clone(),
                location: None,
            })?
            .to_string();

        let version = split_versioned_namespace(&namespace)?.1;

        let mut imports = match value.get("imports") {
            None => Vec::new(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(Import::try_from)
                .collect::<Result<Vec<_>>>()?,
            Some(_) => {
                return Err(ConcertoError::IllegalModel {
                    message: "model 'imports' must be an array".into(),
                    file_name: file_name.clone(),
                    location: None,
                });
            }
        };

        // Every non-system model file imports the system types implicitly.
        // TS: ModelFile.fromAst (src/introspect/modelfile.ts), the built-in
        // import; ported here because the trial's oracle fixtures load models
        // that use them (P0-04b).
        let is_system = namespace.starts_with("concerto@") || namespace == "concerto";
        if !is_system {
            imports.push(Import::try_from(&built_in_import())?);
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
            let versioned = matches!(
                model_util::parse_namespace(Some(imp.namespace()), false)?,
                model_util::ParsedNamespace::Full {
                    version: Some(_),
                    ..
                }
            );
            if !versioned {
                return Err(plain_error(format!(
                    "Cannot use an unversioned import {}.",
                    imp.namespace()
                )));
            }
        }

        let mut declarations = Vec::new();
        let mut local_types = HashMap::new();
        match value.get("declarations") {
            None => {}
            Some(serde_json::Value::Array(arr)) => {
                for raw in arr {
                    let decl = Declaration::from_model_json(raw, &namespace, file_name.as_deref())
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
                return Err(ConcertoError::IllegalModel {
                    message: "model 'declarations' must be an array".into(),
                    file_name: file_name.clone(),
                    location: None,
                });
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
            ast: value.clone(),
            concerto_version,
            definitions,
            external,
        })
    }

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
        &self.ast
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
            return Some(get_fully_qualified_name(&self.namespace, short));
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
    pub fn get_import_uri(&self, key: &str) -> Option<&str> {
        self.imports.iter().find_map(|imp| {
            let uri = imp.uri()?;
            let first = imp.imported_names().first()?;
            (get_fully_qualified_name(imp.namespace(), first) == key).then_some(uri)
        })
    }

    /// TS: `ModelFile.getExternalImports` — every import-URI pair
    /// [`ModelFile::get_import_uri`] can answer, keyed the same way.
    pub fn get_external_imports(&self) -> HashMap<String, String> {
        self.imports
            .iter()
            .filter_map(|imp| {
                let uri = imp.uri()?;
                let first = imp.imported_names().first()?;
                Some((
                    get_fully_qualified_name(imp.namespace(), first),
                    uri.to_string(),
                ))
            })
            .collect()
    }

    /// TS: `ModelFile.getImports` — the fully-qualified names this file
    /// imports (the declared name of each, never an alias, matching
    /// `ModelUtil.importFullyQualifiedNames`), including the built-in system
    /// import for a non-system file.
    pub fn get_imports(&self) -> Vec<String> {
        self.imports
            .iter()
            .flat_map(|imp| {
                imp.imported_names()
                    .iter()
                    .map(|name| get_fully_qualified_name(imp.namespace(), name))
            })
            .collect()
    }

    /// TS: `ModelFile.getLocalType` — accepts either a short name, or a name
    /// already qualified with this file's own namespace.
    pub fn get_local_type(&self, type_name: &str) -> Option<&Declaration> {
        let short = type_name
            .strip_prefix(self.namespace.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
            .unwrap_or(type_name);
        self.local_declaration(short)
    }

    /// TS: `ModelFile.isLocalType`.
    pub fn is_local_type(&self, type_name: &str) -> bool {
        !type_name.is_empty() && self.get_local_type(type_name).is_some()
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
                .map(|(_, imported)| get_fully_qualified_name(imp.namespace(), imported))
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
    /// TS's `JSON.stringify(this.imports)` ([`ModelFile::imports_json`]).
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
    pub fn get_imported_type(&self, type_name: &str) -> Result<String> {
        self.resolve_import(type_name)
            .map(|fqn| get_short_name(&fqn).to_string())
    }

    /// TS: `ModelFile.isDefined` — a primitive, or a type this file declares
    /// itself (an imported-only name is not "defined" by this file).
    pub fn is_defined(&self, type_name: &str) -> bool {
        is_primitive_type(type_name) || self.get_local_type(type_name).is_some()
    }

    /// TS: `ModelFile.getFullyQualifiedTypeName` — entirely local: a
    /// primitive's own name, an imported name's target FQN, or a locally
    /// declared type's FQN; `None` (TS `null`) when `type_name` is none of
    /// those. [`crate::model_manager::ModelManager`]'s `ResolutionContext`
    /// implementation already serves `ModelFile.getType` and
    /// `Property.getFullyQualifiedTypeName`, the two members that need the
    /// owning `ModelManager` to chase into another file; this one never does.
    pub fn get_fully_qualified_type_name(&self, type_name: &str) -> Option<String> {
        if is_primitive_type(type_name) {
            return Some(type_name.to_string());
        }
        if let Some(fqn) = self.find_import(type_name) {
            return Some(fqn);
        }
        self.get_local_type(type_name)
            .map(|d| get_fully_qualified_name(&self.namespace, d.name()))
    }

    /// TS: `ModelFile.getAssetDeclaration`.
    pub fn get_asset_declaration(&self, name: &str) -> Option<&Declaration> {
        self.get_local_type(name)
            .filter(|d| d.as_class().is_some_and(ClassDeclaration::is_asset))
    }

    /// TS: `ModelFile.getTransactionDeclaration`.
    pub fn get_transaction_declaration(&self, name: &str) -> Option<&Declaration> {
        self.get_local_type(name)
            .filter(|d| d.as_class().is_some_and(ClassDeclaration::is_transaction))
    }

    /// TS: `ModelFile.getEventDeclaration`.
    pub fn get_event_declaration(&self, name: &str) -> Option<&Declaration> {
        self.get_local_type(name)
            .filter(|d| d.as_class().is_some_and(ClassDeclaration::is_event))
    }

    /// TS: `ModelFile.getParticipantDeclaration`.
    pub fn get_participant_declaration(&self, name: &str) -> Option<&Declaration> {
        self.get_local_type(name)
            .filter(|d| d.as_class().is_some_and(ClassDeclaration::is_participant))
    }

    /// TS: `ModelFile.getAssetDeclarations`.
    pub fn get_asset_declarations(&self) -> Vec<&Declaration> {
        self.by_class_kind(ClassDeclaration::is_asset)
    }

    /// TS: `ModelFile.getTransactionDeclarations`.
    pub fn get_transaction_declarations(&self) -> Vec<&Declaration> {
        self.by_class_kind(ClassDeclaration::is_transaction)
    }

    /// TS: `ModelFile.getEventDeclarations`.
    pub fn get_event_declarations(&self) -> Vec<&Declaration> {
        self.by_class_kind(ClassDeclaration::is_event)
    }

    /// TS: `ModelFile.getParticipantDeclarations`.
    pub fn get_participant_declarations(&self) -> Vec<&Declaration> {
        self.by_class_kind(ClassDeclaration::is_participant)
    }

    /// TS: `ModelFile.getConceptDeclarations`.
    pub fn get_concept_declarations(&self) -> Vec<&Declaration> {
        self.by_class_kind(ClassDeclaration::is_concept)
    }

    fn by_class_kind(&self, matches_kind: fn(&ClassDeclaration) -> bool) -> Vec<&Declaration> {
        self.declarations
            .iter()
            .filter(|d| d.as_class().is_some_and(matches_kind))
            .collect()
    }

    /// TS: `ModelFile.getClassDeclarations` — `instanceof ClassDeclaration`,
    /// which `EnumDeclaration` also satisfies (it extends `ClassDeclaration`
    /// in TS, module doc on [`crate::introspect::declaration::EnumDeclaration`]);
    /// only a map or scalar declaration is left out. The same predicate
    /// [`crate::model_manager::ModelManager::class_declarations`]
    /// (`Introspector.getClassDeclarations`) uses.
    pub fn get_class_declarations(&self) -> Vec<&Declaration> {
        self.declarations
            .iter()
            .filter(|d| !d.is_map_declaration() && !d.is_scalar_declaration())
            .collect()
    }

    /// TS: `ModelFile.getEnumDeclarations`.
    pub fn get_enum_declarations(&self) -> Vec<&Declaration> {
        self.declarations
            .iter()
            .filter(|d| d.is_enum_declaration())
            .collect()
    }

    /// TS: `ModelFile.getMapDeclarations`.
    pub fn get_map_declarations(&self) -> Vec<&Declaration> {
        self.declarations
            .iter()
            .filter(|d| d.is_map_declaration())
            .collect()
    }

    /// TS: `ModelFile.getScalarDeclarations`.
    pub fn get_scalar_declarations(&self) -> Vec<&Declaration> {
        self.declarations
            .iter()
            .filter(|d| d.is_scalar_declaration())
            .collect()
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
            .ast
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

        let mut filtered = self.ast.clone();
        filtered["declarations"] = serde_json::Value::Array(declarations);

        if let Some(imports) = self.ast.get("imports").and_then(|v| v.as_array()).cloned() {
            let kept: Vec<serde_json::Value> = imports
                .into_iter()
                .filter_map(|mut imp| {
                    let namespace = imp.get("namespace").and_then(|v| v.as_str())?.to_string();
                    if namespace.starts_with("concerto@") || namespace == "concerto" {
                        return Some(imp);
                    }
                    let short_class =
                        get_short_name(imp.get("$class").and_then(|v| v.as_str()).unwrap_or(""));
                    let source_file = source_manager.model_file(&namespace);
                    match short_class {
                        "ImportType" => {
                            let name = imp.get("name").and_then(|v| v.as_str())?;
                            let keep = source_file
                                .is_none_or(|sf| sf.get_local_type(name).is_none_or(&predicate));
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
                                .filter(|name| sf.get_local_type(name).is_none_or(&predicate))
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

impl ModelFile {
    /// TS `JSON.stringify(this.imports)`: the AST's own import nodes,
    /// verbatim and in their own key order (the AST is kept unchanged,
    /// module doc), followed by the built-in system import `fromAst` appends
    /// for a non-system file (P2-08 review: this used to re-encode the typed
    /// imports, which carry no `$class`).
    fn imports_json(&self) -> String {
        let mut values = match self.ast.get("imports") {
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
    if crate::semver_range::satisfies(CONCERTO_CORE_VERSION, range, true)
        || crate::semver_range::satisfies("3.0.0", range, false)
    {
        return Ok(Some(range.to_string()));
    }
    Err(plain_error(format!(
        "This version of Concerto supports a language version of v3.0.0 or greater, but this model is for {range}"
    )))
}

/// TS `packageJson.version`: the frozen TS 5.0.0 reference's own version
/// (D10), which `ModelFile.isCompatibleVersion` checks a model's
/// `concertoVersion` range against.
const CONCERTO_CORE_VERSION: &str = "5.0.0";

/// A plain JS `Error(message)` (`ErrorKind::Error`), for the several
/// hardcoded, non-catalogue messages `ModelFile.fromAst`/`isCompatibleVersion`
/// throw this way rather than as an `IllegalModelException`.
fn plain_error(message: String) -> ConcertoError {
    ContractError::pre_port(ErrorKind::Error, message, None).into()
}

/// Splits a namespace like `org.example@1.0.0` into its name and version,
/// rejecting a namespace without a `@version`, with a second `@`, with an empty
/// name or version, or with a name segment that is not an identifier.
///
/// This is the pre-port loader's own check, not a port of
/// `ModelUtil.parseNamespace` (which accepts unversioned namespaces, DV-003).
/// It goes when `ModelFile` is ported (P2-08).
pub(crate) fn split_versioned_namespace(namespace: &str) -> Result<(String, String)> {
    let illegal = || ConcertoError::IllegalModel {
        message: format!("invalid namespace: {namespace}"),
        file_name: None,
        location: None,
    };
    let mut parts = namespace.splitn(3, '@');
    let name = parts.next().unwrap_or("").to_string();
    match (parts.next(), parts.next()) {
        (Some(version), None) => {
            if name.is_empty() || version.is_empty() || !name.split('.').all(is_valid_identifier) {
                return Err(illegal());
            }
            Ok((name, version.to_string()))
        }
        _ => Err(illegal()),
    }
}

/// Stamps this file's name onto an `IllegalModel` error that came up while
/// parsing one of its declarations, so the message points somewhere useful.
fn annotate(err: ConcertoError, file_name: &Option<String>) -> ConcertoError {
    match err {
        ConcertoError::IllegalModel {
            message, location, ..
        } => ConcertoError::IllegalModel {
            message,
            file_name: file_name.clone(),
            location,
        },
        other => other,
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
        let message = |r: Result<()>| match r.unwrap_err() {
            ConcertoError::Contract(c) => {
                assert_eq!(c.kind, ErrorKind::Error);
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
        let ConcertoError::Contract(err) = err else {
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
        let ConcertoError::Contract(err) = mf.resolve_import("Coin").unwrap_err() else {
            panic!("expected a contract error");
        };
        assert_eq!(
            err.final_message(),
            "Failed to find \"Coin\" in list of imports \"[[{\"$class\":\"concerto.metamodel@1.0.0.ImportType\",\"name\":\"Wow\",\"namespace\":\"org.doge@1.0.0\"},{\"$class\":\"concerto.metamodel@1.0.0.ImportTypes\",\"namespace\":\"concerto@1.0.0\",\"types\":[\"Concept\",\"Asset\",\"Transaction\",\"Participant\",\"Event\"]}]]\" for namespace \"org.acme@1.0.0\". "
        );
    }

    #[test]
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

    #[test]
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
            .add_model(&model_with_two_concepts("org.src@1.0.0"), None)
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
