//! The `ModelUtil` and `ResourceId` bindings.
//!
//! Split out of `lib.rs`; the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// ModelUtil (src/modelutil.ts)
// ---------------------------------------------------------------------------

/// TS: ModelUtil.getShortName
#[wasm_bindgen(js_name = modelUtilGetShortName)]
pub fn model_util_get_short_name(fqn: JsValue) -> JsResult<String> {
    run(|| Ok(mu::short_name(&receiver(&fqn, "fqn", "lastIndexOf")?).to_string()))
}

/// TS: ModelUtil.getNamespace. `!fqn` covers every falsy value.
#[wasm_bindgen(js_name = modelUtilGetNamespace)]
pub fn model_util_get_namespace(fqn: JsValue) -> JsResult<String> {
    run(|| {
        if !fqn.is_truthy() {
            return Ok(mu::get_namespace(None)?.to_string());
        }
        let fqn = receiver(&fqn, "fqn", "lastIndexOf")?;
        Ok(mu::get_namespace(Some(&fqn))?.to_string())
    })
}

/// TS `ModelUtil.parseNamespace(ns, {disableVersionParsing})` of a JS value:
/// `!ns` fails for every falsy value; a truthy non-string has no `split`.
pub(crate) fn parse_namespace_js(ns: &JsValue, disable: bool) -> Result<mu::ParsedNamespace> {
    if ns.is_truthy() {
        let ns = receiver(ns, "ns", "split")?;
        Ok(mu::parse_namespace_with(Some(&ns), disable)?)
    } else {
        Ok(mu::parse_namespace_with(None, disable)?)
    }
}

/// Whether `ns` is a non-empty string with no `@`: a namespace with no
/// version, which `parseNamespace` rejects since BC-02, and which the
/// model file header and `enforceImportVersioning` reject with their own
/// errors, as TS 5.0.0 did.
pub(crate) fn is_unversioned_namespace(ns: &JsValue) -> bool {
    ns.as_string()
        .is_some_and(|ns| !ns.is_empty() && !ns.contains('@'))
}

/// TS: ModelUtil.parseNamespace, with the version checked in Rust only: no
/// `semver.parse` callback, and the result comes back as one string rather
/// than an object built property by property across the boundary. It throws
/// what `ModelUtil.parseNamespace` throws. Otherwise the first character
/// says which result it is, and the rest holds its parts separated by `@`
/// (no part can contain one: the namespace has at most one, and `name` and
/// `version` are the text either side of it):
/// - `N<name>`: `{ name }` (`disableVersionParsing`);
/// - `U<name>@<escapedNamespace>`: no version, so `version` and
///   `versionParsed` are `null`;
/// - `V<name>@<escapedNamespace>@<version>`: the shim builds
///   `versionParsed` itself with `semver.parse`, in JS, where it costs far
///   less than a callback across the boundary. The Rust check is strict
///   SemVer 2.0.0 (BC-41), which `semver.parse` accepts too, except where
///   node-semver's own limits reject it (a component above
///   `Number.MAX_SAFE_INTEGER`, or more than 256 UTF-16 units): there
///   `semver.parse` returns `null`, as the engine's own `versionParsed` is
///   `None` (`model_util::semver_parse`).
#[wasm_bindgen(js_name = modelUtilParseNamespaceChecked)]
pub fn model_util_parse_namespace_checked(ns: JsValue, options: JsValue) -> JsResult<String> {
    run(|| {
        let disable = !nullish(&options) && get(&options, "disableVersionParsing")?.is_truthy();
        Ok(match parse_namespace_js(&ns, disable)? {
            mu::ParsedNamespace::NameOnly { name } => format!("N{name}"),
            mu::ParsedNamespace::Full {
                name,
                escaped_namespace,
                version: None,
                ..
            } => format!("U{name}@{escaped_namespace}"),
            mu::ParsedNamespace::Full {
                name,
                escaped_namespace,
                version: Some(version),
                ..
            } => format!("V{name}@{escaped_namespace}@{version}"),
        })
    })
}

/// TS: ModelUtil.importFullyQualifiedNames
#[wasm_bindgen(js_name = modelUtilImportFullyQualifiedNames)]
pub fn model_util_import_fully_qualified_names(imp: JsValue) -> JsResult<Array> {
    run(|| {
        let imp = to_json(&imp)?;
        let names = mu::import_fully_qualified_names(imp.as_ref())?;
        Ok(names.iter().map(|n| JsValue::from_str(n)).collect())
    })
}

/// TS: ModelUtil.isPrimitiveType. `indexOf` is strict: a non-string is not a
/// primitive type name.
#[wasm_bindgen(js_name = modelUtilIsPrimitiveType)]
pub fn model_util_is_primitive_type(type_name: JsValue) -> bool {
    type_name
        .as_string()
        .is_some_and(|t| mu::is_primitive_type(&t))
}

/// TS: ModelUtil.capitalizeFirstLetter
#[wasm_bindgen(js_name = modelUtilCapitalizeFirstLetter)]
pub fn model_util_capitalize_first_letter(string: JsValue) -> JsResult<String> {
    run(|| {
        Ok(mu::capitalize_first_letter(&receiver(
            &string, "string", "charAt",
        )?))
    })
}

/// TS: ModelUtil.isValidIdentifier. A non-string (`undefined`, `null`, a
/// number, ...) is not a valid identifier (BC-01). TS 5.0.0 passed it to
/// `RegExp.prototype.test`, which converts it with `String()`, so
/// `undefined` and `null` answered `true` (DV-002).
#[wasm_bindgen(js_name = modelUtilIsValidIdentifier)]
pub fn model_util_is_valid_identifier(name: JsValue) -> JsResult<bool> {
    run(|| {
        Ok(name
            .as_string()
            .is_some_and(|name| mu::is_valid_identifier(&name)))
    })
}

/// TS: ModelUtil.getFullyQualifiedName. A falsy namespace returns the `type`
/// argument itself, whatever it is.
#[wasm_bindgen(js_name = modelUtilGetFullyQualifiedName)]
pub fn model_util_get_fully_qualified_name(
    namespace: JsValue,
    type_name: JsValue,
) -> JsResult<JsValue> {
    run(|| {
        if !namespace.is_truthy() {
            return Ok(type_name);
        }
        let joined = mu::qualify(&js_string(&namespace)?, &js_string(&type_name)?);
        Ok(JsValue::from_str(&joined))
    })
}

/// TS: ModelUtil.removeNamespaceVersionFromFullyQualifiedName
#[wasm_bindgen(js_name = modelUtilRemoveNamespaceVersionFromFullyQualifiedName)]
pub fn model_util_remove_namespace_version_from_fully_qualified_name(
    fqn: JsValue,
) -> JsResult<String> {
    run(|| {
        if !fqn.is_truthy() {
            return Ok(mu::remove_namespace_version_from_fully_qualified_name(
                None,
            )?);
        }
        let fqn = receiver(&fqn, "fqn", "lastIndexOf")?;
        Ok(mu::remove_namespace_version_from_fully_qualified_name(
            Some(&fqn),
        )?)
    })
}

/// TS: ModelUtil.isSystemProperty. `includes` is strict: a non-string is
/// never a system property.
#[wasm_bindgen(js_name = modelUtilIsSystemProperty)]
pub fn model_util_is_system_property(name: JsValue) -> bool {
    name.as_string().is_some_and(|n| mu::is_system_property(&n))
}

/// TS: ModelUtil.isPrivateSystemProperty (also used as an `Array.filter`
/// callback, so extra arguments are ignored).
#[wasm_bindgen(js_name = modelUtilIsPrivateSystemProperty)]
pub fn model_util_is_private_system_property(name: JsValue) -> bool {
    name.as_string()
        .is_some_and(|n| mu::is_private_system_property(&n))
}

/// The one key `isValidMapKey`/`isValidMapValue` read, `$class`, as JSON.
pub(crate) fn class_node(node: &JsValue) -> Result<Option<Value>> {
    if node.is_undefined() {
        return Ok(None);
    }
    if node.is_null() {
        return Ok(Some(Value::Null));
    }
    let class = get(node, "$class")?;
    Ok(Some(match class.as_string() {
        Some(class) => json!({ "$class": class }),
        None => json!({}),
    }))
}

/// TS: ModelUtil.isValidMapKey
#[wasm_bindgen(js_name = modelUtilIsValidMapKey)]
pub fn model_util_is_valid_map_key(key: JsValue) -> JsResult<bool> {
    run(|| Ok(mu::is_valid_map_key(class_node(&key)?.as_ref())?))
}

/// TS: ModelUtil.isValidMapValue
#[wasm_bindgen(js_name = modelUtilIsValidMapValue)]
pub fn model_util_is_valid_map_value(value: JsValue) -> JsResult<bool> {
    run(|| Ok(mu::is_valid_map_value(class_node(&value)?.as_ref())?))
}

// ---------------------------------------------------------------------------
// ResourceId (src/model/resourceid.ts)
// ---------------------------------------------------------------------------

/// TS: ResourceId.fromURI. `legacyNamespace`/`legacyType` are the optional,
/// nullable legacy-format arguments; a nullish value is `None`, matching how
/// TS reads an omitted parameter.
#[wasm_bindgen(js_name = resourceIdFromURI)]
pub fn resource_id_from_uri(
    uri: JsValue,
    legacy_namespace: JsValue,
    legacy_type: JsValue,
) -> JsResult<JsValue> {
    run(|| {
        // TS's private `parseUri` calls `uri.match(...)`, which throws a
        // `TypeError` for a non-string `uri`; `fromURI` catches that and
        // reports it as the same "Invalid URI" error a malformed string
        // produces, keyed on `String(uri)`. A non-string `uri` must not be
        // silently coerced into a valid id (PORTING.md 1.4).
        let uri = uri.as_string().ok_or_else(|| {
            js_string(&uri)
                .map(|rendered| {
                    plain_error("resourceid-fromuri-invaliduri", vec![("uri", rendered)])
                })
                .unwrap_or_else(|e| e)
        })?;
        let legacy_namespace = if nullish(&legacy_namespace) {
            None
        } else {
            Some(js_string(&legacy_namespace)?)
        };
        let legacy_type = if nullish(&legacy_type) {
            None
        } else {
            Some(js_string(&legacy_type)?)
        };
        let id = ResourceId::from_uri(&uri, legacy_namespace.as_deref(), legacy_type.as_deref())?;
        let out = Object::new();
        set(&out, "namespace", &JsValue::from_str(&id.namespace));
        set(&out, "type", &JsValue::from_str(&id.type_name));
        set(&out, "id", &JsValue::from_str(&id.id));
        Ok(out.into())
    })
}

/// TS: ResourceId.prototype.toURI. Takes the view's `namespace`/`type`/`id`
/// fields rather than a handle: `ResourceId` is a plain value object (the
/// ledger's HYBRID constructor row), so the view still holds its own state
/// and only the URI encoding runs in Rust.
#[wasm_bindgen(js_name = resourceIdToURI)]
pub fn resource_id_to_uri(namespace: JsValue, type_name: JsValue, id: JsValue) -> JsResult<String> {
    run(|| {
        let namespace = js_string(&namespace)?;
        let type_name = js_string(&type_name)?;
        let id = js_string(&id)?;
        let resource = ResourceId::new(namespace, type_name, id)?;
        Ok(resource.to_uri())
    })
}

/// TS: `ResourceId.fromURI` over many URIs at once, for the visitor path of
/// a relationship-typed map: one crossing per map rather than one per value.
/// `uris` is an array of URI strings, all read with the same legacy
/// namespace and type. The result is flat, three slots per URI: its
/// `namespace`, `type` and `id`, or `undefined` in all three when that URI
/// is not a string or does not parse. The caller then reads that one URI
/// with `resourceIdFromURI`, which throws its error at the same point of the
/// walk as before.
#[wasm_bindgen(js_name = resourceIdsFromURIs)]
pub fn resource_ids_from_uris(
    uris: Array,
    legacy_namespace: JsValue,
    legacy_type: JsValue,
) -> JsResult<Array> {
    run(|| {
        let legacy_namespace = if nullish(&legacy_namespace) {
            None
        } else {
            Some(js_string(&legacy_namespace)?)
        };
        let legacy_type = if nullish(&legacy_type) {
            None
        } else {
            Some(js_string(&legacy_type)?)
        };
        let out = Array::new_with_length(uris.length().saturating_mul(3));
        for i in 0..uris.length() {
            let Some(uri) = uris.get(i).as_string() else {
                continue;
            };
            let Ok(id) =
                ResourceId::from_uri(&uri, legacy_namespace.as_deref(), legacy_type.as_deref())
            else {
                continue;
            };
            let at = i * 3;
            out.set(at, JsValue::from_str(&id.namespace));
            out.set(at + 1, JsValue::from_str(&id.type_name));
            out.set(at + 2, JsValue::from_str(&id.id));
        }
        Ok(out)
    })
}

/// TS: `ResourceId.prototype.toURI` over many identifiers at once, for the
/// visitor path of a relationship-typed map: one crossing per map rather
/// than one per value. `fields` is flat, three slots per identifier
/// (`namespace`, `type`, `id`). The result holds one URI per identifier, or
/// `undefined` where a slot is not a string or the identifier is invalid;
/// the caller then writes that one with `resourceIdToURI`.
#[wasm_bindgen(js_name = resourceIdsToURIs)]
pub fn resource_ids_to_uris(fields: Array) -> JsResult<Array> {
    run(|| {
        let count = fields.length() / 3;
        let out = Array::new_with_length(count);
        for i in 0..count {
            let (Some(namespace), Some(type_name), Some(id)) = (
                fields.get(i * 3).as_string(),
                fields.get(i * 3 + 1).as_string(),
                fields.get(i * 3 + 2).as_string(),
            ) else {
                continue;
            };
            let Ok(resource) = ResourceId::new(namespace, type_name, id) else {
                continue;
            };
            out.set(i, JsValue::from_str(&resource.to_uri()));
        }
        Ok(out)
    })
}
