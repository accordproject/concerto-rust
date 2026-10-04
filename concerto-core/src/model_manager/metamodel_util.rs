use crate::hash::SeededHashMap;

use crate::json::Value;

use crate::error::{ContractError, Error, ErrorKind, Result};

/// The metamodel's own namespace, short for the five reserved
/// declarations `createNameTable` seeds the table with.
const CONCERTO_NS: &str = "concerto@1.0.0";

/// The metamodel's namespace prefix, stripped from a node's `$class` to
/// get the short name the `switch` in `resolveTypeNames` matches on.
const MM_NS: &str = "concerto.metamodel@1.0.0.";

/// One `createNameTable` entry: the namespace and (possibly aliased)
/// local name a bare name resolves to.
struct ResolvedName {
    namespace: String,
    name: String,
    resolved_name: Option<String>,
}

/// The registered models (`getAst(false, true).models`), borrowed and
/// keyed by namespace (first model wins, as TS `findNamespace`'s
/// `Array.find` does); see `ModelManager::prior_models`.
pub(super) type PriorModels<'a> = SeededHashMap<&'a str, &'a Value>;

/// TS `findNamespace`: the model in `prior_models` whose namespace is
/// `namespace`, if one is registered.
fn find_namespace<'a>(prior_models: &PriorModels<'a>, namespace: &str) -> Option<&'a Value> {
    prior_models.get(namespace).copied()
}

/// TS `findDeclaration`: `model`'s own declaration named `name`, if any.
fn find_declaration<'a>(model: &'a Value, name: &str) -> Option<&'a Value> {
    model
        .get("declarations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|decl| decl.get("name").and_then(Value::as_str) == Some(name))
}

/// TS: `Declaration ${imp.name} in namespace ${namespace} not found`.
fn declaration_not_found(name: &str, namespace: &str) -> Error {
    ContractError::new(
        ErrorKind::InvalidArgument,
        "metamodelutil-createnametable-declarationnotfound",
        vec![
            ("name", name.to_string()),
            ("namespace", namespace.to_string()),
        ],
    )
    .into()
}

/// TS: `Name ${name} not found`.
fn name_not_found(name: &str) -> Error {
    ContractError::new(
        ErrorKind::InvalidArgument,
        "metamodelutil-resolvename-notfound",
        vec![("name", name.to_string())],
    )
    .into()
}

/// TS: `Unrecognized $class ${String(metaModel.$class)}`.
fn unrecognized_class(rendered: String) -> Error {
    ContractError::new(
        ErrorKind::InvalidArgument,
        "metamodelutil-resolvetypenames-unrecognizedclass",
        vec![("class", rendered)],
    )
    .into()
}

/// A JS `TypeError` for reading `.declarations` of `undefined`: TS's
/// `findNamespace` returns `undefined` for an import whose namespace is
/// not (yet) registered, and every `createNameTable` branch reads
/// straight off that result without an existence check.
fn undefined_declarations() -> Error {
    ContractError::new(
        ErrorKind::MalformedInput,
        "engine-typeerror-readproperties",
        vec![
            ("value", "undefined".to_string()),
            ("property", "declarations".to_string()),
        ],
    )
    .into()
}

/// TS `createNameTable`: a bare-name -> (namespace, name[, resolvedName])
/// table for `meta_model`, seeded with the five reserved
/// `concerto@1.0.0` declarations, then every name `meta_model` imports —
/// in import order, a later import overriding an earlier one — and
/// finally every name `meta_model` declares itself (overriding its own
/// imports), the same override order as TS's two `forEach` loops.
fn create_name_table(
    prior_models: &PriorModels<'_>,
    meta_model: &Value,
) -> Result<SeededHashMap<String, ResolvedName>> {
    let mut table: SeededHashMap<String, ResolvedName> =
        ["Concept", "Asset", "Participant", "Transaction", "Event"]
            .into_iter()
            .map(|name| {
                (
                    name.to_string(),
                    ResolvedName {
                        namespace: CONCERTO_NS.to_string(),
                        name: name.to_string(),
                        resolved_name: None,
                    },
                )
            })
            .collect();

    for imp in meta_model
        .get("imports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let namespace = imp
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let model_file = find_namespace(prior_models, namespace);
        let class = imp
            .get("$class")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match class.strip_prefix(MM_NS) {
            Some("ImportType") => {
                let model_file = model_file.ok_or_else(undefined_declarations)?;
                let name = imp.get("name").and_then(Value::as_str).unwrap_or_default();
                if find_declaration(model_file, name).is_none() {
                    return Err(declaration_not_found(name, namespace));
                }
                table.insert(
                    name.to_string(),
                    ResolvedName {
                        namespace: namespace.to_string(),
                        name: name.to_string(),
                        resolved_name: None,
                    },
                );
            }
            Some("ImportTypes") => {
                // TS only reads `modelFile.declarations` inside
                // `imp.types.forEach`, so an import of no types from an
                // unregistered namespace does not throw.
                let aliases: SeededHashMap<&str, &str> = imp
                    .get("aliasedTypes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|a| {
                        Some((
                            a.get("name").and_then(Value::as_str)?,
                            a.get("aliasedName").and_then(Value::as_str)?,
                        ))
                    })
                    .collect();
                for ty in imp
                    .get("types")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(ty) = ty.as_str() else { continue };
                    let model_file = model_file.ok_or_else(undefined_declarations)?;
                    if find_declaration(model_file, ty).is_none() {
                        return Err(declaration_not_found(ty, namespace));
                    }
                    let local_name = aliases.get(ty).copied().unwrap_or(ty);
                    let entry = if local_name != ty {
                        ResolvedName {
                            namespace: namespace.to_string(),
                            name: local_name.to_string(),
                            resolved_name: Some(ty.to_string()),
                        }
                    } else {
                        ResolvedName {
                            namespace: namespace.to_string(),
                            name: ty.to_string(),
                            resolved_name: None,
                        }
                    };
                    table.insert(local_name.to_string(), entry);
                }
            }
            _ => {
                // TS's `else` branch: `ImportAll` (and anything else),
                // every one of the target model's own declarations.
                let model_file = model_file.ok_or_else(undefined_declarations)?;
                for decl in model_file
                    .get("declarations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(name) = decl.get("name").and_then(Value::as_str) {
                        table.insert(
                            name.to_string(),
                            ResolvedName {
                                namespace: namespace.to_string(),
                                name: name.to_string(),
                                resolved_name: None,
                            },
                        );
                    }
                }
            }
        }
    }

    let own_namespace = meta_model
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for decl in meta_model
        .get("declarations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(name) = decl.get("name").and_then(Value::as_str) {
            table.insert(
                name.to_string(),
                ResolvedName {
                    namespace: own_namespace.to_string(),
                    name: name.to_string(),
                    resolved_name: None,
                },
            );
        }
    }

    Ok(table)
}

/// Sets a `TypeIdentifier`-shaped `node`'s `namespace` (and `name`, and
/// `resolvedName` when present) from `table[name]`: the shared tail of TS's
/// `superType` case and `.type` group. `table[name].name` always equals
/// `name`, so TS's re-read after reassigning it is one read here.
fn set_resolved_type_identifier(
    node: &mut Value,
    name: &str,
    table: &SeededHashMap<String, ResolvedName>,
) -> Result<()> {
    let entry = table.get(name).ok_or_else(|| name_not_found(name))?;
    let Some(map) = node.as_object_mut() else {
        return Ok(());
    };
    map.insert("namespace".into(), Value::String(entry.namespace.clone()));
    map.insert("name".into(), Value::String(entry.name.clone()));
    if let Some(resolved) = &entry.resolved_name {
        map.insert("resolvedName".into(), Value::String(resolved.clone()));
    }
    Ok(())
}

/// TS `resolveTypeNames`: mutates `node` (and everything it holds) in
/// place, adding the fully-qualified `namespace` (and `resolvedName`,
/// where the name table has one) next to every type name `node` or one
/// of its descendants carries — a super type, an object/relationship
/// property's or map key/value's `type`, a decorator type reference
/// argument, and a scalar declaration's own name.
fn resolve_type_names(node: &mut Value, table: &SeededHashMap<String, ResolvedName>) -> Result<()> {
    // Any element can carry a decorator (including a primitive field),
    // so resolve those first, exactly as TS does before its `switch`.
    if let Some(decorators) = node.get_mut("decorators").and_then(Value::as_array_mut) {
        for decorator in decorators.iter_mut() {
            resolve_type_names(decorator, table)?;
        }
    }

    let class_value = node.get("$class");
    let class_str = class_value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    // TS: `if (!metaModel.$class) throw ...` — only a missing, `null`,
    // non-string or empty `$class` is falsy; anything else truthy that
    // matches no `case` below falls through `default` as a no-op.
    let Some(class_str) = class_str else {
        let rendered = match class_value {
            None => "undefined".to_string(),
            Some(Value::Null) => "null".to_string(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        };
        return Err(unrecognized_class(rendered));
    };
    let Some(short) = class_str.strip_prefix(MM_NS) else {
        return Ok(());
    };

    match short {
        "Model" => {
            if let Some(decls) = node.get_mut("declarations").and_then(Value::as_array_mut) {
                for decl in decls.iter_mut() {
                    resolve_type_names(decl, table)?;
                }
            }
        }
        "EnumDeclaration"
        | "AssetDeclaration"
        | "ConceptDeclaration"
        | "EventDeclaration"
        | "TransactionDeclaration"
        | "ParticipantDeclaration" => {
            if let Some(super_type) = node.get_mut("superType") {
                let name = super_type
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(name) = name {
                    set_resolved_type_identifier(super_type, &name, table)?;
                }
            }
            if let Some(props) = node.get_mut("properties").and_then(Value::as_array_mut) {
                for property in props.iter_mut() {
                    resolve_type_names(property, table)?;
                }
            }
        }
        "MapDeclaration" => {
            if let Some(key) = node.get_mut("key") {
                resolve_type_names(key, table)?;
            }
            if let Some(value) = node.get_mut("value") {
                resolve_type_names(value, table)?;
            }
        }
        "Decorator" => {
            if let Some(args) = node.get_mut("arguments").and_then(Value::as_array_mut) {
                for argument in args.iter_mut() {
                    resolve_type_names(argument, table)?;
                }
            }
        }
        "ObjectProperty"
        | "RelationshipProperty"
        | "DecoratorTypeReference"
        | "ObjectMapKeyType"
        | "ObjectMapValueType"
        | "RelationshipMapValueType" => {
            if let Some(type_node) = node.get_mut("type") {
                let name = type_node
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(name) = name {
                    set_resolved_type_identifier(type_node, &name, table)?;
                }
            }
        }
        "StringScalar" | "BooleanScalar" | "DateTimeScalar" | "DoubleScalar" | "LongScalar"
        | "IntegerScalar" => {
            let name = node.get("name").and_then(Value::as_str).map(str::to_string);
            if let Some(name) = name {
                let namespace = table
                    .get(&name)
                    .map(|entry| entry.namespace.clone())
                    .ok_or_else(|| name_not_found(&name))?;
                if let Some(map) = node.as_object_mut() {
                    map.insert("namespace".into(), Value::String(namespace));
                    map.insert("name".into(), Value::String(name));
                }
            }
        }
        // Every other `$class` (primitive properties and map key/value
        // types, decorator literals, …) needs no name resolution: TS's
        // `default` case is a no-op once `metaModel.$class` is truthy,
        // which every well-formed node here already established.
        _ => {}
    }
    Ok(())
}

/// TS `resolveLocalNames`: `meta_model` with every type name it holds
/// resolved to its declaring namespace, against `prior_models`
/// (`ModelManager.getAst(false, true)`'s models, see [`PriorModels`]).
pub(super) fn resolve_local_names(
    prior_models: &PriorModels<'_>,
    meta_model: &Value,
) -> Result<Value> {
    let table = create_name_table(prior_models, meta_model)?;
    let mut result = meta_model.clone();
    resolve_type_names(&mut result, &table)?;
    Ok(result)
}
