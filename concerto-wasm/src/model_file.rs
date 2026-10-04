//! The `ModelFile` header bindings and the staged-header snapshot.
//!
//! Split out of `lib.rs`; the crate root glob-imports it.

use super::*;

/// The metamodel namespace, TS `MetaModelNamespace` (concerto-metamodel).
pub(crate) const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// TS: `ModelFile.enforceImportVersioning(imp)`:
/// `ModelUtil.parseNamespace(imp.namespace)` must give a version, or the plain
/// `Error` TS throws is raised; a namespace `parseNamespace` rejects raises
/// its own error first.
pub(crate) fn enforce_import_versioning(imp: &JsValue) -> Result<()> {
    let namespace = get(imp, "namespace")?;
    // BC-02: `parseNamespace` rejects an unversioned namespace itself;
    // an unversioned import keeps this function's own error.
    let versioned = !is_unversioned_namespace(&namespace)
        && matches!(
            parse_namespace_js(&namespace, false)?,
            mu::ParsedNamespace::Full { version: Some(ref v), .. } if !v.is_empty()
        );
    if versioned {
        return Ok(());
    }
    Err(ContractError::pre_port(
        ErrorKind::InvalidArgument,
        format!(
            "Cannot use an unversioned import {}.",
            js_string(&namespace)?
        ),
        None,
    )
    .into())
}

/// TS: `ModelFile.enforceImportVersioning(imp)`,
/// [`enforce_import_versioning`].
#[wasm_bindgen(js_name = modelFileEnforceImportVersioning)]
pub fn model_file_enforce_import_versioning(imp: JsValue) -> JsResult<()> {
    run(|| enforce_import_versioning(&imp))
}

/// TS: `ModelFile.isCompatibleVersion()`, on the JS `ModelFile` `view`: when
/// `view.ast.concertoVersion` is truthy it must be a range this runtime supports
/// ([`concerto_core::introspect::model_file::compatible_concerto_version`]), which is
/// then stored as `view.concertoVersion`; otherwise the plain `Error` TS throws is
/// raised. A truthy non-string is never a range node-semver can parse (`satisfies`
/// and `minSatisfying` both give up on it), so it is always that `Error`.
#[wasm_bindgen(js_name = modelFileIsCompatibleVersion)]
pub fn model_file_is_compatible_version(view: JsValue) -> JsResult<()> {
    use concerto_core::introspect::model_file::{
        compatible_concerto_version, incompatible_concerto_version,
    };
    run(|| {
        let range = get(&get(&view, "ast")?, "concertoVersion")?;
        if !range.is_truthy() {
            return Ok(());
        }
        let Some(text) = range.as_string() else {
            return Err(incompatible_concerto_version(&js_string(&range)?).into());
        };
        let accepted = compatible_concerto_version(&text)?;
        set_property(&view, "concertoVersion", &JsValue::from_str(&accepted))
    })
}

/// TS: `ModelFile._fromAstHeader(ast)` on the JS `ModelFile` `view`: checks
/// `ast.namespace` (versioned for a system file too, BC-02), then sets
/// `namespace`, `version`, `imports` (with the implicit system import),
/// `importShortNames` and `importUriMap`, rejecting an unversioned or
/// wildcard import and an alias to a primitive. It reads the same JS values
/// in TS's order, so a malformed AST raises TS's own errors.
#[wasm_bindgen(js_name = modelFileFromAstHeader)]
pub fn model_file_from_ast_header(view: JsValue, ast: JsValue) -> JsResult<()> {
    let body = || -> Result<()> {
        let namespace = get(&ast, "namespace")?;
        // BC-02: an unversioned namespace keeps this header's own checks
        // and errors (the identifier check, then the plain `Error`
        // below), rather than `parseNamespace`'s.
        let (name, version) = if is_unversioned_namespace(&namespace) {
            (namespace.as_string().unwrap_or_default(), JsValue::NULL)
        } else {
            match parse_namespace_js(&namespace, false)? {
                mu::ParsedNamespace::Full { name, version, .. } => (
                    name,
                    version.map_or(JsValue::NULL, |v| JsValue::from_str(&v)),
                ),
                mu::ParsedNamespace::NameOnly { name } => (name, JsValue::UNDEFINED),
            }
        };
        for part in name.split('.') {
            if !mu::is_valid_identifier(part) {
                return Err(illegal_model_error(
                    format!("Invalid namespace part '{part}'"),
                    to_json(&get(&get(&view, "ast")?, "location")?)?,
                ));
            }
        }
        set_property(&view, "namespace", &namespace)?;
        set_property(&view, "version", &version)?;
        let is_system = || -> Result<bool> {
            Ok(call(&view, "isSystemModelFile", &[], "this.isSystemModelFile")?.is_truthy())
        };
        // BC-02: every model file needs a version; TS 5.0.0
        // exempted a system one (`isSystemModelFile()`, a bare `concerto`
        // namespace).
        if !version.is_truthy() {
            return Err(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                format!(
                    "Cannot create a ModelFile with an unversioned namespace: {}. All models \
                     must specify a version (e.g., @1.0.0).",
                    js_string(&namespace)?
                ),
                None,
            )
            .into());
        }

        // A copy, since the implicit import is added to it.
        let ast_imports = get(&ast, "imports")?;
        let imports = if ast_imports.is_truthy() {
            call(
                &ast_imports,
                "concat",
                &[Array::new().into()],
                "ast.imports.concat",
            )?
        } else {
            Array::new().into()
        };
        if !is_system()? {
            let implicit = to_js(&json!({
                "$class": format!("{METAMODEL_NAMESPACE}.ImportTypes"),
                "namespace": "concerto@1.0.0",
                "types": ["Concept", "Asset", "Transaction", "Participant", "Event"],
            }));
            call(&imports, "push", &[implicit], "imports.push")?;
        }
        set_property(&view, "imports", &imports)?;

        let short_names = get(&view, "importShortNames")?;
        let uri_map = get(&view, "importUriMap")?;
        let set_short_name = |key: &JsValue, value: &JsValue| -> Result<()> {
            call(
                &short_names,
                "set",
                &[key.clone(), value.clone()],
                "this.importShortNames.set",
            )
            .map(|_| ())
        };
        let import_types = format!("{METAMODEL_NAMESPACE}.ImportTypes");
        let import_all = format!("{METAMODEL_NAMESPACE}.ImportAll");
        for imp in each(&imports, "this.imports.forEach")? {
            // `this.enforceImportVersioning(imp)`, as TS calls it
            // ([`model_file_enforce_import_versioning`]).
            call(
                &view,
                "enforceImportVersioning",
                std::slice::from_ref(&imp),
                "this.enforceImportVersioning",
            )?;
            let class = get(&imp, "$class")?.as_string();
            if class.as_deref() == Some(import_all.as_str()) {
                return Err(ContractError::pre_port(
                    ErrorKind::InvalidArgument,
                    "Wildcard Imports are not permitted.".to_string(),
                    None,
                )
                .into());
            }
            if class.as_deref() == Some(import_types.as_str()) {
                let ns = js_string(&get(&imp, "namespace")?)?;
                let aliased = get(&imp, "aliasedTypes")?;
                let has_aliases = aliased.is_truthy() && js_length(&aliased)?.gt(&JsValue::from(0));
                let aliases = js_sys::Map::new();
                if has_aliases {
                    for entry in each(&aliased, "imp.aliasedTypes.forEach")? {
                        let alias_name = get(&entry, "name")?;
                        let aliased_name = get(&entry, "aliasedName")?;
                        if aliased_name
                            .as_string()
                            .is_some_and(|n| mu::is_primitive_type(&n))
                        {
                            return Err(ContractError::pre_port(
                                ErrorKind::InvalidArgument,
                                "Types cannot be aliased to primitive type".to_string(),
                                None,
                            )
                            .into());
                        }
                        aliases.set(&alias_name, &aliased_name);
                    }
                }
                for type_name in each(&get(&imp, "types")?, "imp.types.forEach")? {
                    let fqn = JsValue::from_str(&format!("{ns}.{}", js_string(&type_name)?));
                    let alias = aliases.get(&type_name);
                    let key = if has_aliases && !nullish(&alias) {
                        alias
                    } else {
                        type_name
                    };
                    set_short_name(&key, &fqn)?;
                }
            } else {
                let first = import_fully_qualified_name(&imp)?;
                set_short_name(&get(&imp, "name")?, &first)?;
            }
            let uri = get(&imp, "uri")?;
            if uri.is_truthy() {
                let first = import_fully_qualified_name(&imp)?;
                Reflect::set(&uri_map, &first, &uri).map_err(Error::Js)?;
            }
        }
        Ok(())
    };
    run_naming(|| view.clone(), body)
}

/// The strict AST shape check ([`ModelManagerHandle::check_ast_shape`]) as a
/// free function: it reads no handle, so the TS views call it without one.
/// Throws an `IllegalModelException` for an AST that does not have the
/// metamodel's shape; malformed JSON throws a JS `SyntaxError`.
#[wasm_bindgen(js_name = checkAstShape)]
pub fn check_ast_shape(ast: &str) -> JsResult<()> {
    run(|| {
        let value = parse_json(ast)?;
        Ok(concerto_core::instance::check_ast_shape(&value)?)
    })
}

/// The precomputed system model header as a free function: it reads no
/// handle, so the TS views call it without one.
#[wasm_bindgen(js_name = systemModelFileHeader)]
pub fn system_model_file_header(ast: &str) -> Option<String> {
    system_model_header(ast)
}

/// [`system_model_file_header`]: the header text of the
/// fixed system model whose AST is exactly `ast`, from one checked load of
/// that text ([`ModelFile::from_json_text_checked_with_imports`], what
/// `stageModelFileBytes` runs with [`STAGE_CHECKED`]) on first use. `None` for any other text.
pub(crate) fn system_model_header(ast: &str) -> Option<String> {
    caches::SYSTEM_MODEL_HEADERS.with(|cell| {
        cell.get_or_init(|| {
            concerto_core::rootmodel::system_model_json_texts()
                .into_iter()
                .map(|(file_name, text)| {
                    let header = match ModelFile::from_json_text_checked_with_imports(
                        text,
                        None,
                        Some(file_name.to_string()),
                    ) {
                        // In the flat layout, its stage id 0 (nothing is
                        // staged).
                        Ok(Ok((file, imports))) => flat_staged_text(
                            0,
                            staged_header_from_parts(file.namespace(), imports.as_ref()).as_ref(),
                        )
                        .ok(),
                        _ => None,
                    };
                    (text, header)
                })
                .collect()
        })
        .iter()
        .find(|(text, _)| *text == ast)
        .and_then(|(_, header)| header.clone())
    })
}

/// What [`model_file_from_ast_header`] would set, read from a staged file
/// for [`ModelManagerHandle::stage_model_file_bytes`] to return with the
/// stage: `{namespace, version, system, shortNames, uriMap}`, the last two
/// as their assignments in order. `None` for any error or non-canonical
/// AST shape; the caller then runs that binding over the JS values, so
/// every error keeps its path.
pub(crate) fn staged_header_from_parts<'a>(
    namespace: &'a str,
    imports: Option<&'a Value>,
) -> Option<StagedHeader<'a>> {
    let version = match mu::parse_namespace_with(Some(namespace), false).ok()? {
        mu::ParsedNamespace::Full { name, version, .. } => {
            if !name.split('.').all(mu::is_valid_identifier) {
                return None;
            }
            version
        }
        mu::ParsedNamespace::NameOnly { .. } => return None,
    };
    let system = namespace.starts_with("concerto@") || namespace == "concerto";
    if version.as_deref().is_none_or(str::is_empty) && !system {
        return None;
    }
    let ast_imports: &[Value] = match imports {
        None | Some(Value::Null) => &[],
        Some(Value::Array(items)) => items,
        Some(_) => return None,
    };
    let import_types = format!("{METAMODEL_NAMESPACE}.ImportTypes");
    let import_type = format!("{METAMODEL_NAMESPACE}.ImportType");
    let mut short_names: Vec<(Cow<'a, str>, Cow<'a, str>)> = Vec::new();
    let mut uri_map: Vec<(Cow<'a, str>, Cow<'a, str>)> = Vec::new();
    for imp in ast_imports {
        let imp = imp.as_object()?;
        let class = imp.get("$class")?.as_str()?;
        let ns = imp.get("namespace")?.as_str()?;
        // `this.enforceImportVersioning(imp)`.
        match mu::parse_namespace_with(Some(ns), false).ok()? {
            mu::ParsedNamespace::Full {
                version: Some(ref v),
                ..
            } if !v.is_empty() => {}
            _ => return None,
        }
        let first = if class == import_types {
            let mut aliases: Vec<(&str, &str)> = Vec::new();
            match imp.get("aliasedTypes") {
                None | Some(Value::Null) => {}
                Some(Value::Array(entries)) => {
                    for entry in entries {
                        let entry = entry.as_object()?;
                        let name = entry.get("name")?.as_str()?;
                        let aliased_name = entry.get("aliasedName")?.as_str()?;
                        if mu::is_primitive_type(aliased_name) {
                            return None;
                        }
                        // `Map.set`: a later entry for the same name wins.
                        match aliases.iter_mut().find(|(n, _)| *n == name) {
                            Some(slot) => slot.1 = aliased_name,
                            None => aliases.push((name, aliased_name)),
                        }
                    }
                }
                Some(_) => return None,
            }
            let types = imp.get("types")?.as_array()?;
            let mut first: Option<Cow<'a, str>> = None;
            for type_name in types {
                let type_name = type_name.as_str()?;
                let fqn = format!("{ns}.{type_name}");
                let key = aliases
                    .iter()
                    .find(|(n, _)| *n == type_name)
                    .map_or(type_name, |(_, alias)| alias);
                if first.is_none() {
                    first = Some(Cow::Owned(fqn.clone()));
                }
                short_names.push((Cow::Borrowed(key), Cow::Owned(fqn)));
            }
            first
        } else if class == import_type {
            let name = imp.get("name")?.as_str()?;
            let fqn = format!("{ns}.{name}");
            short_names.push((Cow::Borrowed(name), Cow::Owned(fqn.clone())));
            Some(Cow::Owned(fqn))
        } else {
            return None;
        };
        match imp.get("uri") {
            None | Some(Value::Null) => {}
            Some(Value::String(uri)) if uri.is_empty() => {}
            Some(Value::String(uri)) => uri_map.push((first?, Cow::Borrowed(uri.as_str()))),
            Some(Value::Bool(false)) => {}
            Some(_) => return None,
        }
    }
    // The implicit import of the system types every non-system file gets
    // (`ModelFile.fromAst`), last: always versioned, with no aliases and no
    // URI, so it adds exactly these short names (without building its node
    // and running the loop above over it).
    if !system {
        short_names.extend(
            IMPLICIT_IMPORT_SHORT_NAMES
                .iter()
                .map(|(key, fqn)| (Cow::Borrowed(*key), Cow::Borrowed(*fqn))),
        );
    }
    Some(StagedHeader {
        namespace: Cow::Borrowed(namespace),
        version,
        system,
        short_names,
        uri_map,
    })
}

/// The short names the implicit import of the system types
/// (`concerto@1.0.0`'s `Concept`, `Asset`, `Transaction`, `Participant` and
/// `Event`, in that order) adds to a non-system file's header.
pub(crate) const IMPLICIT_IMPORT_SHORT_NAMES: [(&str, &str); 5] = [
    ("Concept", "concerto@1.0.0.Concept"),
    ("Asset", "concerto@1.0.0.Asset"),
    ("Transaction", "concerto@1.0.0.Transaction"),
    ("Participant", "concerto@1.0.0.Participant"),
    ("Event", "concerto@1.0.0.Event"),
];

/// [`staged_header_from_parts`]'s header: what `modelFileFromAstHeader` would
/// set on a JS `ModelFile`. Every staging path returns it in the flat layout
/// ([`FlatStaged`]): a model file staged from its AST
/// ([`ModelManagerHandle::stage_model_file_bytes`]), a fixed system model's
/// verdict ([`system_model_file_header`]) and a DecoratorManager result
/// ([`stage_shared`]).
#[derive(Debug, Serialize)]
pub(crate) struct StagedHeader<'a> {
    namespace: Cow<'a, str>,
    version: Option<String>,
    system: bool,
    #[serde(rename = "shortNames")]
    short_names: Vec<(Cow<'a, str>, Cow<'a, str>)>,
    #[serde(rename = "uriMap")]
    uri_map: Vec<(Cow<'a, str>, Cow<'a, str>)>,
}

impl StagedHeader<'_> {
    /// The header, owning what it borrowed from the AST it was read from
    /// (for a header kept past that AST, [`DcsExtractKept`]).
    pub(crate) fn into_owned(self) -> StagedHeader<'static> {
        let own = |(a, b): (Cow<'_, str>, Cow<'_, str>)| {
            (Cow::Owned(a.into_owned()), Cow::Owned(b.into_owned()))
        };
        StagedHeader {
            namespace: Cow::Owned(self.namespace.into_owned()),
            version: self.version,
            system: self.system,
            short_names: self.short_names.into_iter().map(own).collect(),
            uri_map: self.uri_map.into_iter().map(own).collect(),
        }
    }
}

/// A staging result in the flat layout: `[id]` when there
/// is no header, otherwise
///
/// ```text
/// [id, namespace, version, system, n, key_1, name_1, ..., key_n, name_n,
///  uriKey_1, uri_1, ...]
/// ```
///
/// where the `n` key/name pairs are the header's `shortNames` without the
/// implicit system import's five, which every non-system header ends with
/// ([`IMPLICIT_IMPORT_SHORT_NAMES`]) and the TS side appends itself, and
/// the pairs after them are its `uriMap`, in order. The one writer of the
/// staged-header wire format; the TS side has the one reader (engine/views.ts
/// `applyStagedFileHeader`).
pub(crate) struct FlatStaged<'h, 'a> {
    pub(crate) id: u32,
    pub(crate) header: Option<&'h StagedHeader<'a>>,
}

impl Serialize for FlatStaged<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(None)?;
        seq.serialize_element(&self.id)?;
        if let Some(header) = self.header {
            let implicit = if header.system {
                0
            } else {
                IMPLICIT_IMPORT_SHORT_NAMES.len()
            };
            let explicit = header
                .short_names
                .get(..header.short_names.len().saturating_sub(implicit))
                .unwrap_or_default();
            seq.serialize_element(&header.namespace)?;
            seq.serialize_element(&header.version)?;
            seq.serialize_element(&header.system)?;
            seq.serialize_element(&explicit.len())?;
            for (key, name) in explicit {
                seq.serialize_element(key)?;
                seq.serialize_element(name)?;
            }
            for (key, uri) in &header.uri_map {
                seq.serialize_element(key)?;
                seq.serialize_element(uri)?;
            }
        }
        seq.end()
    }
}

/// [`FlatStaged`]'s JSON text.
pub(crate) fn flat_staged_text(
    id: u32,
    header: Option<&StagedHeader<'_>>,
) -> serde_json::Result<String> {
    serde_json::to_string(&FlatStaged { id, header })
}

/// `value.length`: a string primitive's own length (UTF-16 code units),
/// which [`get`] does not read.
pub(crate) fn js_length(value: &JsValue) -> Result<JsValue> {
    match value.as_string() {
        Some(text) => Ok(JsValue::from(text.encode_utf16().count() as f64)),
        None => get(value, "length"),
    }
}

/// `ModelUtil.importFullyQualifiedNames(imp)[0]`: `undefined` when there is
/// none.
pub(crate) fn import_fully_qualified_name(imp: &JsValue) -> Result<JsValue> {
    let names = mu::import_fully_qualified_names(to_json(imp)?.as_ref())?;
    Ok(names
        .first()
        .map_or(JsValue::UNDEFINED, |n| JsValue::from_str(n)))
}

/// The elements `value.forEach` visits, for a JS array; `expression` names
/// the callee in the `TypeError` any other value raises (a nullish one
/// fails reading `forEach` itself, as in TS).
pub(crate) fn each(value: &JsValue, expression: &str) -> Result<Vec<JsValue>> {
    if Array::is_array(value) {
        return Ok(Array::from(value).iter().collect());
    }
    get(value, "forEach")?;
    Err(type_error(
        "engine-typeerror-notafunction",
        vec![("expression", expression.to_string())],
    ))
}
