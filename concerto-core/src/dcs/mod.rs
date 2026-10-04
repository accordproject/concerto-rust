//! Decorator command sets (DCS): a port of `DecoratorManager`
//! (`src/decoratormanager.ts`) — applying, validating and migrating
//! [DecoratorCommandSet](https://models.accordproject.org/concerto/decorators.cto)
//! objects against a loaded [`ModelManager`] — plus [`extractor`], the
//! companion port of `DecoratorExtractor` (`src/decoratorextractor.ts`) that
//! runs the same command set model in reverse, pulling decorators back out
//! of a model into a command set and a vocabulary file, and
//! [`dcsconverter`], the port of `src/dcsconverter.ts`'s `jsonToYaml`/
//! `yamlToJson` (the short DCS YAML format).
//!
//! Both TS classes work on the metamodel AST as plain, dynamically shaped
//! objects (`ModelFile.getAst()`, mutated and fed back through
//! `ModelManager.fromAst`), not through the typed introspection views. This
//! port keeps that shape: every command, target, decorator and AST node is a
//! [`crate::json::Value`], and [`decorate_models`] round-trips a
//! [`ModelManager`] through [`ModelFile::ast`] as `fromAst` does. Where TS
//! reads a property of `undefined`/`null` (a command set with no `commands`,
//! a command with no `decorator`), this port raises the same JS `TypeError`.
//!
//! `DecoratorManager.validate` and `migrateAndValidate` check each command
//! set with `Serializer.fromJSON` against a validation model manager (the
//! metamodel, the user's model files, then `DCS_MODEL`), built here from the
//! metamodel and `DCS_MODEL` ASTs (CTO stays in JS); the `$class` check and
//! `getType` are hand-ported to keep TS's errors (`from_json_against`), and
//! the rest is [`crate::instance::from_json`].
//!
//! `decorateModels` and the `extract*` statics read `modelManager.getAst(true,
//! …)`, which resolves every model (`BaseModelManager.resolveMetaModel`), as
//! [`ModelManager::get_ast`] does here. Resolving an already resolved AST is
//! idempotent.
pub mod dcsconverter;
#[cfg(test)]
#[path = "tests/decoratormanager.rs"]
mod decoratormanager_tests;
pub mod extractor;
mod yaml_quote;

pub use dcsconverter::{json_to_yaml, yaml_to_json};
use yaml_quote::quote_string_value;

use crate::hash::{SeededHashMap, SeededHashSet};
use std::borrow::Cow;
use std::sync::Arc;

use crate::json::{Map, Value};

use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::instance::metamodel::metamodel_class;
use crate::introspect::DecoratorValidationOptions;
use crate::introspect::model_file::ModelFile;
use crate::model_manager::ModelManager;
use crate::model_util::{self, ParsedNamespace};

/// `DCS_VERSION` (`src/decoratormanager.ts`): the decorator command set
/// model version this port targets.
pub(crate) const DCS_VERSION: &str = "0.4.0";

/// The metamodel's `MapDeclaration` class.
const MAP_DECLARATION_CLASS: &str = metamodel_class!("MapDeclaration");
/// The metamodel's `ImportType` class.
const IMPORT_TYPE_CLASS: &str = metamodel_class!("ImportType");

/// `falsyOrEqual(test, values)` (`src/decoratormanager.ts`): `true` when
/// `test` is JS-falsy (`None` for `undefined` and `null`), an array with a
/// string element in `values`, or a string in `values`. Any other truthy
/// `test` is never in `values` (`includes` is strict equality).
pub fn falsy_or_equal(test: Option<&Value>, values: &[&str]) -> bool {
    match test {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .any(|s| values.contains(&s)),
        Some(Value::String(s)) if !s.is_empty() => values.contains(&s.as_str()),
        Some(v) if crate::ecma::is_truthy(v) => false,
        _ => true,
    }
}

/// `falsyOrEqual(test, values)` where TS passes a bare string as `values`
/// rather than an array: `applyDecoratorForMapElement` and
/// `executeCommand`'s `MapDeclaration` branch pass `decl.$class` itself.
/// `String.prototype.includes` then makes a string `test` a *substring*
/// test (and coerces any other truthy `test` to a string first), and
/// `new Set(values)` in `intersect` makes an array `test` a test against
/// `values`' individual characters.
fn falsy_or_equal_in_string(test: Option<&Value>, values: &str) -> bool {
    match test {
        Some(Value::Array(arr)) => arr.iter().any(|x| {
            matches!(x, Value::String(s) if s.chars().count() == 1 && values.contains(s.as_str()))
        }),
        Some(v) if crate::ecma::is_truthy(v) => values.contains(&crate::ecma::to_js_string(v)),
        _ => true,
    }
}

/// `DcsIndexWrapper` (`src/decoratormanager.ts`): a decorator command
/// alongside its position in the command set it was collected from, so
/// commands collected from several of [`get_decorator_maps`]'s maps can be
/// put back into command-set order before they run.
///
/// It borrows the command, which is never copied.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DcsIndexWrapper<'a> {
    command: &'a Value,
    index: usize,
}

impl<'a> DcsIndexWrapper<'a> {
    /// The decorator command.
    pub(crate) fn command(&self) -> &'a Value {
        self.command
    }

    /// The command's index in the (possibly flattened) command set it came
    /// from.
    pub(crate) fn index(&self) -> usize {
        self.index
    }
}

/// `DecoratorManager.getDecoratorMaps`'s five return maps, each keyed by the
/// target value its commands share, borrowed from the commands.
#[derive(Debug, Clone, Default)]
pub(crate) struct DecoratorMaps<'a> {
    /// Commands targeting a `target.namespace`.
    pub(crate) namespace_commands: SeededHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    /// Commands targeting a `target.declaration`.
    pub(crate) declaration_commands: SeededHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    /// Commands targeting a `target.property` (or one entry of
    /// `target.properties`).
    pub(crate) property_commands: SeededHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    /// Commands targeting a `target.mapElement`.
    pub(crate) map_element_commands: SeededHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    /// Commands targeting a `target.type`.
    pub(crate) type_commands: SeededHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
}

/// `DecoratorManager.addDcsWithIndexToMap` (`src/decoratormanager.ts`).
fn add_dcs_with_index_to_map<'a>(
    map: &mut SeededHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    key: &'a str,
    dcs_with_index: DcsIndexWrapper<'a>,
) {
    map.entry(key).or_default().push(dcs_with_index);
}

/// `DecoratorManager.getDecoratorMaps`: indexes `commands` by target. Each
/// command goes into one map, by the first target field that is truthy, in
/// the order of TS's `switch (true)` over `!!decoratorCommand?.target?.<f>`:
/// `type`, `property`, `properties` (one entry per property named),
/// `mapElement`, `declaration`, `namespace`. TS keys its `Map`s by the
/// field's value, and only ever looks names (strings) up, so a truthy
/// non-string value claims the command but is never matched: it is not
/// indexed. A truthy `properties` that is not an array is a `TypeError`, as
/// `decoratorCommand.target.properties.forEach(...)` is.
pub(crate) fn get_decorator_maps<'a>(
    commands: impl IntoIterator<Item = &'a Value>,
) -> Result<DecoratorMaps<'a>> {
    let mut maps = DecoratorMaps::default();
    for (index, command) in commands.into_iter().enumerate() {
        let target = command.get("target");
        let truthy = |key: &str| {
            target
                .and_then(|t| t.get(key))
                .filter(|v| crate::ecma::is_truthy(v))
        };
        let dcs = DcsIndexWrapper { command, index };
        let (map, key) = if let Some(t) = truthy("type") {
            (&mut maps.type_commands, t)
        } else if let Some(p) = truthy("property") {
            (&mut maps.property_commands, p)
        } else if let Some(ps) = truthy("properties") {
            let Value::Array(ps) = ps else {
                return Err(ContractError::new(
                    ErrorKind::MalformedInput,
                    "engine-typeerror-notafunction",
                    vec![(
                        "expression",
                        "decoratorCommand.target.properties.forEach".to_string(),
                    )],
                )
                .into());
            };
            for p in ps.iter().filter_map(Value::as_str) {
                add_dcs_with_index_to_map(&mut maps.property_commands, p, dcs);
            }
            continue;
        } else if let Some(m) = truthy("mapElement") {
            (&mut maps.map_element_commands, m)
        } else if let Some(d) = truthy("declaration") {
            (&mut maps.declaration_commands, d)
        } else if let Some(n) = truthy("namespace") {
            (&mut maps.namespace_commands, n)
        } else {
            continue;
        };
        if let Value::String(key) = key {
            add_dcs_with_index_to_map(map, key, dcs);
        }
    }
    Ok(maps)
}

/// `DecoratorManager.pushMapValues` (`src/decoratormanager.ts`).
fn push_map_values<'a>(
    out: &mut Vec<DcsIndexWrapper<'a>>,
    map: &SeededHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    key: &str,
) {
    if let Some(values) = map.get(key) {
        out.extend(values.iter().copied());
    }
}

/// Sorts commands collected from [`get_decorator_maps`]'s maps back into
/// command-set order, as every `.sort((a, b) => a.getIndex() - b.getIndex())`
/// call in `decorateModels` does.
fn sorted_by_index(mut commands: Vec<DcsIndexWrapper<'_>>) -> Vec<DcsIndexWrapper<'_>> {
    commands.sort_by_key(|c| c.index());
    commands
}

/// `DecoratorManager.migrateTo`: rewrites every `$class` naming the
/// `org.accordproject.decoratorcommands` namespace, in place, to
/// `DCS_VERSION`. TS's `version` parameter is never read (its body uses
/// the module constant), so it is dropped. As in TS, the version is read
/// through `ModelUtil.getNamespace` and `parseNamespace`, whose errors
/// propagate, an unversioned namespace included (BC-02).
pub fn migrate_to(value: &mut Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            // The `$class` is read borrowed, and copied only when it is
            // rewritten.
            let migrated = match map.get("$class") {
                Some(Value::String(class))
                    if class.contains("org.accordproject.decoratorcommands") =>
                {
                    let ns = model_util::get_namespace(Some(class))?;
                    let (_, version) = model_util::namespace_parts(ns)?;
                    // `String.prototype.replace` with a string pattern
                    // replaces only the first occurrence.
                    Some(class.replacen(version, DCS_VERSION, 1))
                }
                _ => None,
            };
            if let Some(migrated) = migrated {
                map.insert("$class".to_string(), Value::String(migrated));
            }
            for v in map.values_mut() {
                migrate_to(v)?;
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                migrate_to(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A bare version string (`"0.4.0"`) as strict SemVer 2.0.0, the grammar
/// `parseNamespace` uses (BC-41): `semver::Version` itself, so that
/// components above 2^53 compare exactly.
fn parse_version(version: &str) -> Option<semver::Version> {
    semver::Version::parse(version).ok()
}

/// A JS `TypeError` for reading `property` of `undefined` (`is_null` false)
/// or `null` (`is_null` true).
fn read_properties_error(is_null: bool, property: &str) -> Error {
    ContractError::new(
        ErrorKind::MalformedInput,
        "engine-typeerror-readproperties",
        vec![
            (
                "value",
                if is_null { "null" } else { "undefined" }.to_string(),
            ),
            ("property", property.to_string()),
        ],
    )
    .into()
}

/// JS `object.key`, where `object` may be `undefined` (`None`) or `null`:
/// reading a property of either is a `TypeError`; any other value yields its
/// own property, or `undefined` (`None`) when it has none (a string, number
/// or array has none of the keys this module reads).
fn js_read<'a>(object: Option<&'a Value>, key: &str) -> Result<Option<&'a Value>> {
    match object {
        None => Err(read_properties_error(false, key)),
        Some(Value::Null) => Err(read_properties_error(true, key)),
        Some(v) => Ok(v.get(key)),
    }
}

/// JS `value.forEach(...)`, where `value` is `object.key` read as
/// [`js_read`] reads it (`None` for `undefined`): an array yields its
/// elements; `undefined` or `null` is a `TypeError` for reading `forEach`;
/// any other value (a JSON value never holds a function) is a `TypeError`
/// naming `expression` as not a function.
fn js_for_each<'a>(value: Option<&'a Value>, expression: &str) -> Result<&'a [Value]> {
    js_read(value, "forEach")?;
    match value {
        Some(Value::Array(items)) => Ok(items),
        _ => Err(ContractError::new(
            ErrorKind::MalformedInput,
            "engine-typeerror-notafunction",
            vec![("expression", expression.to_string())],
        )
        .into()),
    }
}

/// `DecoratorManager.canMigrate`: whether `decorator_command_set`'s `$class`
/// version can be migrated to `target_version` (same major, strictly lower
/// minor). `getNamespace` rejects a missing `$class`, and `parseNamespace`
/// an invalid namespace, an unversioned one included (BC-02).
pub(crate) fn can_migrate(decorator_command_set: &Value, target_version: &str) -> Result<bool> {
    let class = js_read(Some(decorator_command_set), "$class")?;
    let class = match class {
        Some(Value::String(s)) => Some(s.as_str()),
        // `fqn.lastIndexOf('.')` on a truthy non-string.
        Some(v) if crate::ecma::is_truthy(v) => {
            return Err(ContractError::new(
                ErrorKind::MalformedInput,
                "engine-typeerror-notafunction",
                vec![("expression", "fqn.lastIndexOf".to_string())],
            )
            .into());
        }
        // `if (!fqn)`: every falsy value is rejected with "FQN is invalid.".
        _ => None,
    };
    let ns = model_util::get_namespace(class)?;
    let (_, input_version) = model_util::namespace_parts(ns)?;
    let (Some(input), Some(target)) = (parse_version(input_version), parse_version(target_version))
    else {
        // `parseNamespace` already validated `input_version` as a semver,
        // and `target_version` is always `DCS_VERSION`.
        return Ok(false);
    };
    Ok(input.major == target.major && input.minor < target.minor)
}

/// `DecoratorManager.checkForDuplicateDecorators` (`src/decoratormanager.ts`):
/// raises an `IllegalModelException` (its constructor's location suffix
/// included, [`ContractError::final_message`]) if `decorated_ast.decorators`
/// names the same decorator twice.
pub(crate) fn check_for_duplicate_decorators(decorated_ast: &Value) -> Result<()> {
    let mut seen = SeededHashSet::default();
    if let Some(decorators) = decorated_ast.get("decorators").and_then(Value::as_array) {
        for d in decorators {
            let name = d
                .get("name")
                .map(crate::ecma::to_js_string)
                .unwrap_or_else(|| "undefined".to_string());
            if !seen.insert(name.clone()) {
                return Err(ContractError::pre_port(
                    ErrorKind::IllegalModel,
                    format!("Duplicate decorator {name}"),
                    decorated_ast.get("location").cloned(),
                )
                .into());
            }
        }
    }
    Ok(())
}

/// `DecoratorManager.applyDecorator` (`src/decoratormanager.ts`): applies
/// `new_decorator` to `decorated.decorators` — replacing (or adding) the
/// entry of the same name for `"UPSERT"`, or always adding it (then checking
/// for the duplicate it may just have created) for `"APPEND"`. Any other
/// `command_type` (the command's `type` as JS would print it, `"undefined"`
/// when it has none) is an error.
pub(crate) fn apply_decorator(
    decorated: &mut Value,
    command_type: &str,
    new_decorator: &Value,
) -> Result<()> {
    match command_type {
        "UPSERT" => {
            let Value::Object(map) = decorated else {
                return Ok(());
            };
            let new_name = new_decorator.get("name");
            let mut updated = false;
            if let Some(Value::Array(decorators)) = map.get_mut("decorators") {
                for d in decorators.iter_mut() {
                    if d.get("name") == new_name {
                        *d = new_decorator.clone();
                        updated = true;
                    }
                }
            }
            if !updated {
                push_decorator(map, new_decorator);
            }
        }
        "APPEND" => {
            let Value::Object(map) = decorated else {
                return Ok(());
            };
            push_decorator(map, new_decorator);
            check_for_duplicate_decorators(decorated)?;
        }
        other => {
            return Err(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                format!("Unknown command type {other}"),
                None,
            )
            .into());
        }
    }
    Ok(())
}

fn push_decorator(map: &mut Map<String, Value>, decorator: &Value) {
    match map.get_mut("decorators") {
        Some(Value::Array(arr)) => arr.push(decorator.clone()),
        _ => {
            map.insert(
                "decorators".to_string(),
                Value::Array(vec![decorator.clone()]),
            );
        }
    }
}

/// `null`, for a command part that is absent.
static NULL: Value = Value::Null;

/// A command's `type`, `decorator` and `target`, as `const { target,
/// decorator, type } = command` reads them: `type` as JS would interpolate
/// it into `Unknown command type ${type}` (`"undefined"` when absent), and
/// `decorator` and `target` as `null` when absent. All three are borrowed
/// from `command`, but for a `type` that is not a string, which is spelled
/// as JS would print it.
fn command_parts(command: &Value) -> (Cow<'_, str>, &Value, &Value) {
    let command_type = match command.get("type") {
        Some(Value::String(s)) => Cow::Borrowed(s.as_str()),
        Some(other) => Cow::Owned(crate::ecma::to_js_string(other)),
        None => Cow::Borrowed("undefined"),
    };
    let decorator = command.get("decorator").unwrap_or(&NULL);
    let target = command.get("target").unwrap_or(&NULL);
    (command_type, decorator, target)
}

/// `DecoratorManager.applyDecoratorForMapElement` (`src/decoratormanager.ts`):
/// applies `new_decorator` to a `MapDeclaration`'s `key` or `value` element,
/// honouring `target.type` when the command names one.
fn apply_decorator_for_map_element(
    element: &str,
    target: &Value,
    declaration: &mut Value,
    command_type: &str,
    new_decorator: &Value,
) -> Result<()> {
    let field = if element == "KEY" { "key" } else { "value" };
    let Some(decl) = declaration.get_mut(field) else {
        return Ok(());
    };
    let target_type = target.get("type").filter(|t| crate::ecma::is_truthy(t));
    let matches = match target_type {
        Some(_) => {
            let class = decl
                .get("$class")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            falsy_or_equal_in_string(target_type, &class)
        }
        None => true,
    };
    if matches {
        apply_decorator(decl, command_type, new_decorator)?;
    }
    Ok(())
}

/// `DecoratorManager.checkForNamespaceTargetAndApplyDecorator`: applies the
/// decorator to `declaration` only when `target.declaration` is truthy (a
/// namespace-level command is [`execute_namespace_command`]'s).
fn check_for_namespace_target_and_apply_decorator(
    declaration: &mut Value,
    command_type: &str,
    decorator: &Value,
    target: &Value,
) -> Result<()> {
    if target
        .get("declaration")
        .is_some_and(crate::ecma::is_truthy)
    {
        apply_decorator(declaration, command_type, decorator)?;
    }
    Ok(())
}

/// `DecoratorManager.executeNamespaceCommand` (`src/decoratormanager.ts`):
/// applies a bare `{ $class, namespace }` command target — exactly two keys,
/// one of them a truthy `namespace` — directly to the model itself.
pub(crate) fn execute_namespace_command(model: &mut Value, command: &Value) -> Result<()> {
    let (command_type, decorator, target) = command_parts(command);
    let is_bare_namespace_target = target
        .as_object()
        .is_some_and(|m| m.len() == 2 && m.get("namespace").is_some_and(crate::ecma::is_truthy));
    if !is_bare_namespace_target {
        return Ok(());
    }
    let namespace = model
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::to_string);
    let name = model_util::namespace_parts(namespace.as_deref().unwrap_or_default())?
        .0
        .to_string();
    let namespace = namespace.unwrap_or_default();
    if falsy_or_equal(
        target.get("namespace"),
        &[namespace.as_str(), name.as_str()],
    ) {
        apply_decorator(model, &command_type, decorator)?;
    }
    Ok(())
}

/// `DecoratorManager.executePropertyCommand` (`src/decoratormanager.ts`):
/// applies `command` to `property` when its target names it, by name (or by
/// membership of `target.properties`) and, if given, by `target.type`.
pub fn execute_property_command(property: &mut Value, command: &Value) -> Result<()> {
    let (command_type, decorator, target) = command_parts(command);
    let truthy = |key: &str| target.get(key).filter(|v| crate::ecma::is_truthy(v));
    if truthy("properties").is_none() && truthy("property").is_none() && truthy("type").is_none() {
        return Ok(());
    }
    let property_name = property
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let property_class = property
        .get("$class")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // `target.property ? target.property : target.properties`.
    let by_name = truthy("property").or_else(|| target.get("properties"));
    if falsy_or_equal(by_name, &[property_name.as_str()])
        && falsy_or_equal(target.get("type"), &[property_class.as_str()])
    {
        apply_decorator(property, &command_type, decorator)?;
    }
    Ok(())
}

/// `DecoratorManager.executeCommand` (`src/decoratormanager.ts`): applies
/// `command` to `declaration` (a `Model`'s AST declaration node), or to
/// `property` when the command's target reaches a property and one is
/// given, honouring `MapDeclaration`'s `key`/`value`/`mapElement` targeting.
pub(crate) fn execute_command(
    namespace: &str,
    declaration: &mut Value,
    command: &Value,
    property: Option<&mut Value>,
) -> Result<()> {
    let (command_type, decorator, target) = command_parts(command);
    // The namespace version is already validated by `decorate_models`.
    let name = match model_util::parse_namespace_with(Some(namespace), true)? {
        ParsedNamespace::NameOnly { name } | ParsedNamespace::Full { name, .. } => name,
    };
    let declaration_name = declaration
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !falsy_or_equal(target.get("namespace"), &[namespace, name.as_str()])
        || !falsy_or_equal(target.get("declaration"), &[declaration_name.as_str()])
    {
        return Ok(());
    }
    let truthy = |key: &str| target.get(key).filter(|v| crate::ecma::is_truthy(v));
    let is_map = declaration.get("$class").and_then(Value::as_str) == Some(MAP_DECLARATION_CLASS);
    if is_map {
        if let Some(map_element) = truthy("mapElement") {
            match map_element.as_str() {
                Some(element @ ("KEY" | "VALUE")) => {
                    apply_decorator_for_map_element(
                        element,
                        target,
                        declaration,
                        &command_type,
                        decorator,
                    )?;
                }
                Some("KEY_VALUE") => {
                    apply_decorator_for_map_element(
                        "KEY",
                        target,
                        declaration,
                        &command_type,
                        decorator,
                    )?;
                    apply_decorator_for_map_element(
                        "VALUE",
                        target,
                        declaration,
                        &command_type,
                        decorator,
                    )?;
                }
                _ => {}
            }
        } else if let Some(target_type) = truthy("type") {
            for element in ["key", "value"] {
                if let Some(decl) = declaration.get_mut(element) {
                    let class = decl
                        .get("$class")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if falsy_or_equal_in_string(Some(target_type), &class) {
                        apply_decorator(decl, &command_type, decorator)?;
                    }
                }
            }
        } else {
            check_for_namespace_target_and_apply_decorator(
                declaration,
                &command_type,
                decorator,
                target,
            )?;
        }
    } else if truthy("property").is_none()
        && truthy("properties").is_none()
        && truthy("type").is_none()
    {
        check_for_namespace_target_and_apply_decorator(
            declaration,
            &command_type,
            decorator,
            target,
        )?;
    } else if let Some(property) = property {
        execute_property_command(property, command)?;
    }
    Ok(())
}

/// `DecoratorManager.validateCommand`: checks a single command's target
/// resolves against `model_manager`: its `target.type` names a real type,
/// its `target.namespace` (versioned, BC-02) a loaded model, and, with a
/// `target.namespace` and `target.declaration`, its
/// `target.property`/`target.properties` a real property of that
/// declaration. The messages are the reference's own text.
pub(crate) fn validate_command(model_manager: &ModelManager, command: &Value) -> Result<()> {
    // `command.target.type`: reading through an absent or `null` target is
    // a JS `TypeError`.
    let target = js_read(Some(command), "target")?.cloned();
    js_read(target.as_ref(), "type")?;
    let target = target.unwrap_or(Value::Null);

    if let Some(t) = target.get("type").and_then(Value::as_str)
        && !t.is_empty()
    {
        resolve_type(model_manager, "DecoratorCommand.type", t)?;
    }

    let mut resolved_model_file: Option<&ModelFile> = None;
    if let Some(namespace) = target
        .get("namespace")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    {
        resolved_model_file = model_manager.model_file(namespace);
        // `ModelUtil.parseNamespace(target.namespace)`: BC-02 rejects an
        // unversioned target namespace with the invalid-namespace error,
        // where TS 5.0.0 matched any version.
        if resolved_model_file.is_none() {
            model_util::parse_namespace(namespace)?;
        }
        if resolved_model_file.is_none() {
            return Err(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                format!(
                    "Decorator Command references namespace \"{namespace}\" which does not exist: {}",
                    serde_json::to_string_pretty(command).unwrap_or_default()
                ),
                None,
            )
            .into());
        }
    }

    // TS: whenever both `namespace` and `declaration` are given, the
    // declaration is checked, with or without a property.
    if let (Some(_), Some(declaration)) = (
        target
            .get("namespace")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty()),
        target
            .get("declaration")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty()),
    ) {
        let model_file = resolved_model_file.expect("guarded above: namespace resolved or errored");
        let fqn = format!("{}.{declaration}", model_file.namespace());
        resolve_type(model_manager, "DecoratorCommand.target.declaration", &fqn)?;
    }

    if target.get("properties").is_some_and(crate::ecma::is_truthy)
        && target.get("property").is_some_and(crate::ecma::is_truthy)
    {
        return Err(ContractError::pre_port(
            ErrorKind::InvalidArgument,
            "Decorator Command references both property and properties. You must either reference a single property or a list of properites.".to_string(),
            None,
        )
        .into());
    }

    if let (Some(namespace), Some(declaration)) = (
        target
            .get("namespace")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty()),
        target
            .get("declaration")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty()),
    ) {
        let model_file = resolved_model_file.expect("guarded above: namespace resolved or errored");
        let fqn = format!("{}.{declaration}", model_file.namespace());

        // `target.property`, then each of `target.properties` (one
        // check, on the borrowed lookup).
        let property = target
            .get("property")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty());
        let properties = target
            .get("properties")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        for property in property.into_iter().chain(properties) {
            if model_manager.property(&fqn, property)?.is_none() {
                return Err(ContractError::pre_port(
                    ErrorKind::InvalidArgument,
                    format!(
                        "Decorator Command references property \"{namespace}.{declaration}.{property}\" which does not exist."
                    ),
                    None,
                )
                .into());
            }
        }
    }

    Ok(())
}

/// `BaseModelManager.resolveType`, as `validateCommand` calls it: a
/// primitive, or a type its namespace's loaded model file recognises under
/// that exact name, with the reference's catalogue templates
/// (`modelmanager-resolvetype-nonsfortype`/`-notypeinnsforcontext`).
fn resolve_type(model_manager: &ModelManager, context: &str, type_name: &str) -> Result<()> {
    if model_util::is_primitive_type(type_name) {
        return Ok(());
    }
    let ns = model_util::get_namespace(Some(type_name))?;
    let Some(mf) = model_manager.model_file(ns) else {
        return Err(ContractError::new(
            ErrorKind::IllegalModel,
            "modelmanager-resolvetype-nonsfortype",
            vec![
                ("type", type_name.to_string()),
                ("context", context.to_string()),
            ],
        )
        .into());
    };
    let short = model_util::short_name(type_name);
    if mf.resolve_local_type(short).as_deref() == Some(type_name) {
        Ok(())
    } else {
        Err(ContractError::new(
            ErrorKind::IllegalModel,
            "modelmanager-resolvetype-notypeinnsforcontext",
            vec![
                ("context", context.to_string()),
                ("type", type_name.to_string()),
                ("namespace", mf.namespace().to_string()),
            ],
        )
        .into())
    }
}

/// The AST of `DCS_MODEL`, the CTO text `DecoratorManager.validate` and
/// `migrateAndValidate` compile: concerto-metamodel 3.17.0's
/// `lib/dcsmodel.json` with `concertoVersion` set to `DCS_MODEL`'s
/// `">3.0.0"`.
const DCS_MODEL_AST_JSON: &str = include_str!("dcsmodel.json");

/// The file name `DecoratorManager.validate` gives `DCS_MODEL`.
const VALIDATE_DCS_FILE_NAME: &str = "decoratorcommands@0.3.0.cto";

/// The file name `DecoratorManager.migrateAndValidate` gives `DCS_MODEL`.
const MIGRATE_DCS_FILE_NAME: &str = "decoratorcommands@0.4.0.cto";

/// The `DCS_MODEL` file under `file_name`, read once per thread and file
/// name and shared. A load error is returned, not cached.
fn dcs_model_file(file_name: &'static str) -> Result<Arc<ModelFile>> {
    thread_local! {
        static DCS_MODEL_FILES: std::cell::RefCell<Vec<(&'static str, Arc<ModelFile>)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    let cached = DCS_MODEL_FILES.with(|cache| {
        cache
            .borrow()
            .iter()
            .find(|(name, _)| *name == file_name)
            .map(|(_, mf)| Arc::clone(mf))
    });
    if let Some(mf) = cached {
        return Ok(mf);
    }
    let dcs_model: Value =
        serde_json::from_str(DCS_MODEL_AST_JSON).expect("the DCS model AST is JSON");
    let mf = Arc::new(ModelFile::from_owned_json_with_definitions(
        dcs_model,
        None,
        Some(file_name.to_string()),
    )?);
    DCS_MODEL_FILES.with(|cache| cache.borrow_mut().push((file_name, Arc::clone(&mf))));
    Ok(mf)
}

/// The validation manager `DecoratorManager.validate` and
/// `migrateAndValidate` build: a [`ModelManager::fork`] of the resident
/// metamodel manager, then the caller's files, then the DCS model, sharing
/// files as TS shares `ModelFile`s. Every file not known valid is validated
/// in TS's order, so the errors are the same; the per-model metamodel check
/// is not run.
fn validation_model_manager(
    model_files: Vec<(
        Arc<ModelFile>,
        Option<Arc<crate::model_manager::ValidityProof>>,
    )>,
    dcs_file_name: &'static str,
) -> Result<ModelManager> {
    let mut model_manager =
        crate::instance::metamodel::with_resident_metamodel_manager(|mm| Ok(mm.fork()))?;
    if !model_files.is_empty() {
        model_manager.insert_models(model_files, true)?;
    }
    model_manager.insert_models(vec![(dcs_model_file(dcs_file_name)?, None)], true)?;
    Ok(model_manager)
}

/// `serializer.fromJSON(decoratorCommandSet)` over the validation model
/// manager. Its first steps are hand-ported, as `Serializer::from_json`
/// does not reproduce them: no `$class` is rejected, a truthy non-string
/// `$class` fails as `getNamespace`'s `fqn.lastIndexOf('.')` does, and an
/// unknown type fails as `getType` fails ([`get_type`]). The rest is
/// [`crate::instance::from_json`].
fn from_json_against(model_manager: &ModelManager, instance: &Value) -> Result<()> {
    let class = js_read(Some(instance), "$class")?;
    let class = match class {
        Some(Value::String(s)) if !s.is_empty() => s.as_str(),
        Some(v) if crate::ecma::is_truthy(v) => {
            // `ModelUtil.getNamespace` calls `fqn.lastIndexOf('.')`.
            return Err(ContractError::new(
                ErrorKind::MalformedInput,
                "engine-typeerror-notafunction",
                vec![("expression", "fqn.lastIndexOf".to_string())],
            )
            .into());
        }
        _ => {
            return Err(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                "Invalid JSON data. Does not contain a $class type identifier.".to_string(),
                None,
            )
            .into());
        }
    };
    get_type(model_manager, class)?;
    // No `DCS_MODEL` declaration is system-identified or timestamped, so
    // the deterministic environment stands in for the id and the clock.
    let options = crate::instance::from_json::FromJsonOptions::default();
    crate::instance::from_json::from_json(
        model_manager,
        instance,
        &options,
        &mut crate::instance::from_json::FixedEnv,
    )
    .map(|_| ())
}

/// `BaseModelManager.getType(qualifiedName)` (`src/basemodelmanager.ts`),
/// as `Serializer.fromJSON` calls it: its two `TypeNotFoundException`s, for
/// a namespace with no model file and for a name its model file does not
/// declare.
fn get_type(model_manager: &ModelManager, qualified_name: &str) -> Result<()> {
    let namespace = model_util::get_namespace(Some(qualified_name))?;
    if model_manager.model_file(namespace).is_none() {
        return Err(ContractError::type_not_found(
            "modelmanager-gettype-noregisteredns",
            vec![("type", qualified_name.to_string())],
            qualified_name.to_string(),
            None,
        )
        .into());
    }
    if model_manager.declaration_id(qualified_name).is_none() {
        return Err(ContractError::type_not_found(
            "modelmanager-gettype-notypeinns",
            vec![
                ("type", model_util::short_name(qualified_name).to_string()),
                ("namespace", namespace.to_string()),
            ],
            qualified_name.to_string(),
            None,
        )
        .into());
    }
    Ok(())
}

/// `DecoratorManager.validate(decoratorCommandSet, modelFiles?)`
/// (`src/decoratormanager.ts`): builds the validation model manager (the
/// decorator, root and metamodel models, then `model_files` if given, then
/// the DCS model), checks `decorator_command_set` against it
/// (`from_json_against`), and returns it. The model files are shared with
/// the returned manager, not copied, as TS shares them.
pub fn validate(
    decorator_command_set: &Value,
    model_files: Option<&[Arc<ModelFile>]>,
) -> Result<ModelManager> {
    let files = model_files
        .unwrap_or_default()
        .iter()
        .map(|mf| (Arc::clone(mf), None))
        .collect();
    let validation_model_manager = validation_model_manager(files, VALIDATE_DCS_FILE_NAME)?;
    from_json_against(&validation_model_manager, decorator_command_set)?;
    Ok(validation_model_manager)
}

/// The structural check of [`validate`] (`serializer.fromJSON(
/// decoratorCommandSet)`), against a validation model manager the caller
/// built as [`validate`] builds its own (the view builds it in
/// `DecoratorManager.validate`).
pub fn validate_against(
    validation_model_manager: &ModelManager,
    decorator_command_set: &Value,
) -> Result<()> {
    from_json_against(validation_model_manager, decorator_command_set)
}

/// `DecoratorManager.jsonToYaml(jsonInput)` (`src/decoratormanager.ts`):
/// [`validate`], then [`dcsconverter::json_to_yaml`].
pub fn validated_json_to_yaml(json_input: &Value) -> Result<String> {
    validate(json_input, None)?;
    json_to_yaml(json_input)
}

/// `DecoratorManager.yamlToJson(yamlInput)` (`src/decoratormanager.ts`):
/// [`dcsconverter::yaml_to_json`], then [`validate`] of its result.
pub fn validated_yaml_to_json(yaml_input: &str) -> Result<Value> {
    let json_output = yaml_to_json(yaml_input)?;
    validate(&json_output, None)?;
    Ok(json_output)
}

/// `DecoratorManager.migrateAndValidate`: migrates each command set's
/// `$class` to [`DCS_VERSION`] in place when `should_migrate` (and
/// [`can_migrate`] allows it); then, only when `should_validate`, builds the
/// validation model manager, checks each command set against it and, with
/// `should_validate_commands`, runs [`validate_command`] over every command.
pub(crate) fn migrate_and_validate(
    model_manager: &ModelManager,
    decorator_command_sets: &mut [Value],
    should_migrate: bool,
    should_validate: bool,
    should_validate_commands: bool,
) -> Result<()> {
    if should_migrate {
        for command_set in decorator_command_sets.iter_mut() {
            if can_migrate(command_set, DCS_VERSION)? {
                migrate_to(command_set)?;
            }
        }
    }
    if should_validate {
        // `validationModelManager.addModelFiles(modelManager.getModelFiles())`:
        // the caller's own files, shared.
        let validation_model_manager = validation_model_manager(
            model_manager.user_files_with_proofs(),
            MIGRATE_DCS_FILE_NAME,
        )?;
        for command_set in decorator_command_sets.iter() {
            from_json_against(&validation_model_manager, command_set)?;
            if should_validate_commands {
                // `commandSet.commands.forEach(...)`. `from_json_against`
                // accepts a valid instance of any type the validation
                // manager holds (a `CommandTarget`, say), so `commands` may
                // be missing or not an array.
                for command in
                    js_for_each(command_set.get("commands"), "commandSet.commands.forEach")?
                {
                    validate_command(&validation_model_manager, command)?;
                }
            }
        }
    }
    Ok(())
}

/// Options for [`decorate_models`]. `DecoratorManager.decorateModels`'s
/// `options` object (`src/decoratormanager.ts`).
#[derive(Debug, Clone, Default)]
pub struct DecorateOptions {
    /// Migrate every command set's `$class` to `DCS_VERSION` first.
    pub migrate: bool,
    /// Check every command set against `DCS_MODEL` first
    /// (`migrate_and_validate`). Gates [`validate_commands`](Self::validate_commands),
    /// matching the reference.
    pub validate: bool,
    /// Run `validate_command` over every command, when [`validate`](Self::validate) is
    /// also set.
    pub validate_commands: bool,
    /// The namespace to use for a decorator (or a type-reference argument)
    /// that names none of its own.
    pub default_namespace: Option<Value>,
    /// `skipValidationAndResolution`: sets both
    /// [`disable_metamodel_resolution`](Self::disable_metamodel_resolution)
    /// and [`disable_metamodel_validation`](Self::disable_metamodel_validation),
    /// and is an error when either was explicitly `Some(false)`.
    pub skip_validation_and_resolution: bool,
    /// `disableMetamodelResolution`: `Some(false)` is JS `false` itself
    /// (which [`skip_validation_and_resolution`](Self::skip_validation_and_resolution)
    /// rejects), `None` any other falsy value or the option's absence.
    pub disable_metamodel_resolution: Option<bool>,
    /// `disableMetamodelValidation`, as
    /// [`disable_metamodel_resolution`](Self::disable_metamodel_resolution).
    pub disable_metamodel_validation: Option<bool>,
    /// The result manager's `decoratorValidation`, which its models are
    /// validated with; `None` takes the source manager's, as TS's `new
    /// ModelManager({ decoratorValidation: modelManager.getDecoratorValidation() })`
    /// does. A binding whose result manager already has its own options
    /// passes them here rather than setting them on the source, which would
    /// forget the source's validated marks.
    pub decorator_validation: Option<DecoratorValidationOptions>,
}

impl DecorateOptions {
    /// The result manager's `decoratorValidation`
    /// ([`decorator_validation`](Self::decorator_validation)).
    fn result_decorator_validation<'a>(
        &'a self,
        model_manager: &'a ModelManager,
    ) -> &'a DecoratorValidationOptions {
        self.decorator_validation
            .as_ref()
            .unwrap_or_else(|| model_manager.decorator_validation())
    }
}

/// `DecoratorManager.decorateModels`: applies every command of every set,
/// in order, and returns a new [`ModelManager`] built as `fromAst` builds
/// one. Like TS, it mutates its arguments (migration rewrites the command
/// sets; `skip_validation_and_resolution` sets `options`' `disable_*`
/// flags). An empty `decorator_command_sets` shares the model files without
/// validating them again.
pub fn decorate_models(
    model_manager: &ModelManager,
    decorator_command_sets: &mut [Value],
    options: &mut DecorateOptions,
) -> Result<ModelManager> {
    if !prepare_command_sets(model_manager, decorator_command_sets, options)? {
        // The same model files, shared, under the same options, and
        // not validated again.
        let mut result =
            model_manager.new_like_with(model_manager.user_files_with_proofs(), false)?;
        let decorator_validation = options.result_decorator_validation(model_manager);
        if result.decorator_validation() != decorator_validation {
            result.set_decorator_validation(decorator_validation.clone());
        }
        return Ok(result);
    }
    let prepared = index_commands(decorator_command_sets, options)?;
    apply_decoration(model_manager, &prepared, options)
}

/// What [`index_commands`] computes from the command sets before
/// `decorateModels` reads the AST: the synthetic imports and the commands
/// indexed by target, borrowed.
#[derive(Debug, Clone)]
pub struct PreparedDecoration<'a> {
    decorator_imports: Vec<Value>,
    maps: DecoratorMaps<'a>,
}

/// The first step of [`decorate_models`]: the empty-input early return
/// (`false`), the `skipValidationAndResolution` option check, then
/// migration and validation (`migrate_and_validate`), which may rewrite
/// the command sets in place. `true` when there are command sets to apply.
pub fn prepare_command_sets(
    model_manager: &ModelManager,
    decorator_command_sets: &mut [Value],
    options: &mut DecorateOptions,
) -> Result<bool> {
    if decorator_command_sets.is_empty() {
        return Ok(false);
    }

    if options.skip_validation_and_resolution {
        if options.disable_metamodel_resolution == Some(false)
            || options.disable_metamodel_validation == Some(false)
        {
            return Err(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                "skipValidationAndResolution cannot be used with disableMetamodelResolution or disableMetamodelValidation options as false".to_string(),
                None,
            )
            .into());
        }
        options.disable_metamodel_resolution = Some(true);
        options.disable_metamodel_validation = Some(true);
    }

    migrate_and_validate(
        model_manager,
        decorator_command_sets,
        options.migrate,
        options.validate,
        options.validate_commands,
    )?;
    Ok(true)
}

/// The second step of [`decorate_models`], before `decorateModels` reads
/// the AST: the synthetic imports and the target maps.
pub fn index_commands<'a>(
    decorator_command_sets: &'a [Value],
    options: &DecorateOptions,
) -> Result<PreparedDecoration<'a>> {
    // `decoratorCommandSets.flatMap(commandSet => commandSet.commands)`: an
    // array of commands is spread, anything else (`undefined` included) is
    // kept as one element.
    let mut combined_commands: Vec<Option<&'a Value>> = Vec::new();
    for command_set in decorator_command_sets {
        match js_read(Some(command_set), "commands")? {
            Some(Value::Array(commands)) => combined_commands.extend(commands.iter().map(Some)),
            other => combined_commands.push(other),
        }
    }

    let decorator_imports =
        synthetic_decorator_imports(&combined_commands, options.default_namespace.as_ref())?;
    // Every element is a command object: `synthetic_decorator_imports` has
    // already read `command.decorator` from each.
    let combined_commands: Vec<&'a Value> = combined_commands.into_iter().flatten().collect();
    let maps = get_decorator_maps(combined_commands.iter().copied())?;
    // BC-02: a command's `target.namespace` goes through `parseNamespace`,
    // so an unversioned one is rejected when the commands are applied,
    // where TS 5.0.0 matched any version.
    for command in &combined_commands {
        if let Some(namespace) = command
            .get("target")
            .and_then(|t| t.get("namespace"))
            .and_then(Value::as_str)
            .filter(|ns| !ns.is_empty())
        {
            model_util::parse_namespace(namespace)?;
        }
    }
    Ok(PreparedDecoration {
        decorator_imports,
        maps,
    })
}

/// The last step of [`decorate_models`]: applies `prepared` to every model
/// (the system ones included), then builds the result as `new
/// ModelManager({ decoratorValidation })` and `fromAst(decoratedAst, {
/// disableValidation })` do. Each decorated AST is moved into the result.
pub fn apply_decoration(
    model_manager: &ModelManager,
    prepared: &PreparedDecoration<'_>,
    options: &DecorateOptions,
) -> Result<ModelManager> {
    // `options?.disableMetamodelResolution ? getAst(false, true) : getAst(true, true)`.
    let resolve = options.disable_metamodel_resolution != Some(true);
    let mut models = model_manager.model_asts(resolve, true)?;
    for model in models.iter_mut() {
        decorate_model(model, &prepared.decorator_imports, &prepared.maps)?;
    }

    let mut decorated = ModelManager::new()?;
    decorated.set_decorator_validation(options.result_decorator_validation(model_manager).clone());
    for model in models.into_iter().filter(|m| {
        !m.get("namespace")
            .and_then(Value::as_str)
            .is_some_and(|ns| crate::model_manager::EXCLUDE_NS.contains(&ns))
    }) {
        decorated.add_owned_model_with_definitions(model, None, None)?;
    }
    if options.disable_metamodel_validation != Some(true) {
        decorated.validate_models()?;
    }
    Ok(decorated)
}

/// The synthetic `ImportType` nodes `decorateModels` declares for every
/// command's decorator and type-reference arguments, so an applied decorator
/// resolves. Only entries with a truthy namespace (their own, or
/// `default_namespace`) are kept, as TS's `.filter(i => i.namespace)` does.
/// `None` in `commands` is a JS `undefined`; reading through one is TS's
/// `TypeError`.
fn synthetic_decorator_imports(
    commands: &[Option<&Value>],
    default_namespace: Option<&Value>,
) -> Result<Vec<Value>> {
    let or_default = |namespace: Option<&Value>| -> Option<Value> {
        match namespace {
            Some(ns) if crate::ecma::is_truthy(ns) => Some(ns.clone()),
            _ => default_namespace.cloned(),
        }
    };
    let mut imports = Vec::new();
    for command in commands {
        let decorator = js_read(*command, "decorator")?;
        let name = js_read(decorator, "name")?.cloned();
        let namespace = decorator.and_then(|d| d.get("namespace"));
        imports.push(import_type(name, or_default(namespace)));

        match decorator.and_then(|d| d.get("arguments")) {
            Some(Value::Array(args)) => {
                for arg in args {
                    let Some(t) = js_read(Some(arg), "type")?.filter(|t| crate::ecma::is_truthy(t))
                    else {
                        continue;
                    };
                    let t_name = t.get("name").cloned();
                    imports.push(import_type(t_name, or_default(t.get("namespace"))));
                }
            }
            Some(v) if crate::ecma::is_truthy(v) => {
                return Err(ContractError::new(
                    ErrorKind::MalformedInput,
                    "engine-typeerror-notafunction",
                    vec![(
                        "expression",
                        "command.decorator.arguments?.filter".to_string(),
                    )],
                )
                .into());
            }
            _ => {}
        }
    }
    Ok(imports
        .into_iter()
        .filter(|i| i.get("namespace").is_some_and(crate::ecma::is_truthy))
        .collect())
}

fn import_type(name: Option<Value>, namespace: Option<Value>) -> Value {
    let mut m = Map::new();
    m.insert(
        "$class".to_string(),
        Value::String(IMPORT_TYPE_CLASS.to_string()),
    );
    if let Some(name) = name {
        m.insert("name".to_string(), name);
    }
    if let Some(ns) = namespace {
        m.insert("namespace".to_string(), ns);
    }
    Value::Object(m)
}

/// One model's worth of `DecoratorManager.decorateModels`'s per-model
/// `forEach` body (`src/decoratormanager.ts`): adds the synthetic imports it
/// needs, then applies every command that reaches each of its declarations
/// (and their properties, and a `MapDeclaration`'s key/value).
fn decorate_model(
    model: &mut Value,
    decorator_imports: &[Value],
    maps: &DecoratorMaps<'_>,
) -> Result<()> {
    let namespace = model
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let model_namespace = model.get("namespace").cloned();
    let needed_imports: Vec<Value> = decorator_imports
        .iter()
        .filter(|i| i.get("namespace") != model_namespace.as_ref())
        .cloned()
        .collect();
    match model.get_mut("imports") {
        Some(Value::Array(existing)) => existing.extend(needed_imports),
        _ => {
            if let Some(map) = model.as_object_mut() {
                map.insert("imports".to_string(), Value::Array(needed_imports));
            }
        }
    }

    let namespace_name = model_util::namespace_parts(&namespace)?.0.to_string();

    // `model.declarations.forEach(...)`: a model with no `declarations` is
    // a JS `TypeError`.
    js_read(js_read(Some(model), "declarations")?, "forEach")?;
    // The declarations are taken out of `model`, so a namespace command
    // can mutate `model` while a declaration is mutated.
    let mut declarations = match model.get_mut("declarations") {
        Some(slot) => std::mem::take(slot),
        None => Value::Array(Vec::new()),
    };
    if let Value::Array(decls) = &mut declarations {
        for decl in decls.iter_mut() {
            let declaration_name = decl
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let class_for_declaration = decl
                .get("$class")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            let mut declaration_commands = Vec::new();
            push_map_values(
                &mut declaration_commands,
                &maps.declaration_commands,
                &declaration_name,
            );
            push_map_values(
                &mut declaration_commands,
                &maps.namespace_commands,
                &namespace,
            );
            push_map_values(
                &mut declaration_commands,
                &maps.namespace_commands,
                &namespace_name,
            );
            push_map_values(
                &mut declaration_commands,
                &maps.type_commands,
                &class_for_declaration,
            );
            for wrapped in sorted_by_index(declaration_commands) {
                execute_command(&namespace, decl, wrapped.command(), None)?;
                execute_namespace_command(model, wrapped.command())?;
            }

            if class_for_declaration == MAP_DECLARATION_CLASS {
                let mut map_commands = Vec::new();
                let key_class = decl
                    .get("key")
                    .and_then(|k| k.get("$class"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let value_class = decl
                    .get("value")
                    .and_then(|v| v.get("$class"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                push_map_values(&mut map_commands, &maps.type_commands, &key_class);
                push_map_values(&mut map_commands, &maps.type_commands, &value_class);
                push_map_values(&mut map_commands, &maps.map_element_commands, "KEY");
                push_map_values(&mut map_commands, &maps.map_element_commands, "VALUE");
                push_map_values(&mut map_commands, &maps.map_element_commands, "KEY_VALUE");
                for wrapped in sorted_by_index(map_commands) {
                    execute_command(&namespace, decl, wrapped.command(), None)?;
                }
            }

            if let Some(Value::Array(_)) = decl.get("properties") {
                let mut properties =
                    match decl.as_object_mut().and_then(|m| m.get_mut("properties")) {
                        Some(slot) => std::mem::take(slot),
                        None => Value::Array(Vec::new()),
                    };
                if let Value::Array(props) = &mut properties {
                    for property in props.iter_mut() {
                        let property_name = property
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let class_for_property = property
                            .get("$class")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let mut property_commands = Vec::new();
                        push_map_values(
                            &mut property_commands,
                            &maps.property_commands,
                            &property_name,
                        );
                        push_map_values(
                            &mut property_commands,
                            &maps.type_commands,
                            &class_for_property,
                        );
                        for wrapped in sorted_by_index(property_commands) {
                            execute_command(&namespace, decl, wrapped.command(), Some(property))?;
                        }
                    }
                }
                if let Some(m) = decl.as_object_mut() {
                    m.insert("properties".to_string(), properties);
                }
            }
        }
    }
    if let Some(map) = model.as_object_mut() {
        map.insert("declarations".to_string(), declarations);
    }
    Ok(())
}

/// The `options` of `DecoratorManager.extractDecorators`,
/// `extractVocabularies` and `extractNonVocabDecorators`
/// (`src/decoratormanager.ts`), with the defaults each spreads the caller's
/// options over: `removeDecoratorsFromModel: false`, `locale: 'en'`.
#[derive(Debug, Clone)]
pub struct ExtractOptions {
    /// `removeDecoratorsFromModel`: strip the extracted decorators from the
    /// returned model manager's models.
    pub remove_decorators_from_model: bool,
    /// `locale`: the extracted vocabularies' locale.
    pub locale: String,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            remove_decorators_from_model: false,
            locale: "en".to_string(),
        }
    }
}

/// `DecoratorManager.extractDecorators` (`ExtractAll`: every model),
/// `extractVocabularies` (`ExtractVocab`: vocabulary decorators, no command
/// sets) or `extractNonVocabDecorators` (`ExtractNonVocab`: the user's
/// models, no vocabularies), by `action`, with the command sets as JSON
/// text. With `keep_source`, the result keeps the source models for
/// [`encode_extract_source`].
pub fn extract(
    model_manager: &ModelManager,
    options: &ExtractOptions,
    action: extractor::Action,
    keep_source: bool,
) -> Result<extractor::ExtractResult> {
    let include_system = action != extractor::Action::ExtractNonVocab;
    extractor::DecoratorExtractor::new(
        options.remove_decorators_from_model,
        &options.locale,
        DCS_VERSION,
        action,
    )
    .extract(model_manager.model_asts(true, include_system)?, keep_source)
}

/// The command sets (JSON text) and vocabularies [`extract`] gives for
/// `action` and `options`, rebuilt from `models`, the source models an
/// earlier [`extract`] keeping its source with the same `action`'s system
/// flag returned. The source models are neither resolved nor loaded again,
/// and no result manager is built.
pub fn encode_extract_source(
    models: &[Value],
    options: &ExtractOptions,
    action: extractor::Action,
) -> Result<(String, Vec<String>)> {
    extractor::DecoratorExtractor::new(
        options.remove_decorators_from_model,
        &options.locale,
        DCS_VERSION,
        action,
    )
    .encode_source(models)
}

#[cfg(test)]
mod tests;
