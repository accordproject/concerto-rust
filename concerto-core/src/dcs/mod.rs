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
//! objects (`ModelFile.getAst()`, mutated with `rfdc` and fed back through
//! `ModelManager.fromAst`), not through the typed `ClassDeclaration`/
//! `Property` views `crate::introspect` builds. This port keeps that shape:
//! every command, target, decorator and AST node here is a [`serde_json::Value`],
//! and [`decorate_models`] round-trips a [`ModelManager`] through
//! [`ModelFile::ast`] the way `fromAst` does, rather than adding a typed
//! `Command`/`DecoratorCommandSet` struct the reference has no counterpart
//! for. Where TS reads a property of `undefined`/`null` along the way (a
//! command set with no `commands`, a command with no `decorator`), this
//! port raises the same JS `TypeError`.
//!
//! ## What stays out of this port (PORTING.md 7.3-style divergence)
//!
//! **The DCS instance check.** `DecoratorManager.validate` and
//! `migrateAndValidate` build a validation model manager (the metamodel,
//! the user's model files, then `DCS_MODEL` compiled with `addCTOModel`) and
//! check each command set against it with `Serializer.fromJSON`. The model
//! manager is built here the same way, from the metamodel and `DCS_MODEL`
//! ASTs (`metamodel.json`, `dcsmodel.json`; CTO stays in JS), and
//! [`validate_command`] runs against it as in TS. Of `Serializer.fromJSON`,
//! the `$class` check and `getType` are hand-ported here to keep TS's own
//! errors for a missing or non-string `$class`
//! (`from_json_against`); the rest — the `JSONPopulator` walk and the
//! `ResourceValidator` pass — now runs as the real, ported
//! `Serializer.fromJSON` over plain JSON (P3-01b; `crate::instance::from_json` since P6-01), raising
//! the same `ValidationException`-style errors TS does.
//!
//! **Metamodel resolution.** `decorateModels` and the `extract*` statics read
//! `modelManager.getAst(true, …)`, which runs `BaseModelManager.resolveMetaModel`
//! over every model; so does this port, through [`ModelManager::get_ast`]
//! (P2-08b). In rust mode the concerto-wasm bindings are handed ASTs the TS
//! ModelManager has already resolved (concerto `src/engine/views.ts`), and
//! Rust then resolves them again here. That second pass is idempotent: every
//! type reference already carries the namespace it resolves to, so the
//! result is the same as resolving once, as in ts mode.
pub mod dcsconverter;
#[cfg(test)]
mod decoratormanager_tests;
pub mod extractor;
mod yaml_quote;

pub use dcsconverter::{json_to_yaml, yaml_to_json};
pub use yaml_quote::{DECORATOR_STRING_TYPE, quote_string_value};

use std::borrow::Cow;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::instance::metamodel::metamodel_class;
use crate::introspect::model_file::ModelFile;
use crate::model_manager::ModelManager;
use crate::model_util::{self, ParsedNamespace};

/// `DCS_VERSION` (`src/decoratormanager.ts`): the decorator command set
/// model version this port targets.
pub const DCS_VERSION: &str = "0.4.0";

/// The metamodel's `MapDeclaration` class.
const MAP_DECLARATION_CLASS: &str = metamodel_class!("MapDeclaration");
/// The metamodel's `ImportType` class.
const IMPORT_TYPE_CLASS: &str = metamodel_class!("ImportType");

/// `falsyOrEqual(test, values)` (`src/decoratormanager.ts`): `true` when
/// `test` is JS-falsy (`None` for `undefined`, `null`, `false`, `0`, `""`),
/// an array intersecting `values`, or a string `values` contains. Any other
/// truthy `test` (a number, `true`, an object) is never in the string array
/// `values` (`Array.prototype.includes` is strict equality).
///
/// The array case is TS's `intersect(test, values).length > 0` (the
/// elements both have in common, deduplicated), tested without building
/// either side's string array (P5-102, C-3): some string element of `test`
/// is in `values`.
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
/// It borrows the command from the command sets it was collected from
/// (P5-102, C-3): a command reached through several maps, or through
/// several entries of `target.properties`, is never copied.
#[derive(Debug, Clone, Copy)]
pub struct DcsIndexWrapper<'a> {
    command: &'a Value,
    index: usize,
}

impl<'a> DcsIndexWrapper<'a> {
    /// The decorator command.
    pub fn command(&self) -> &'a Value {
        self.command
    }

    /// The command's index in the (possibly flattened) command set it came
    /// from.
    pub fn index(&self) -> usize {
        self.index
    }
}

/// `DecoratorManager.getDecoratorMaps`'s five return maps
/// (`src/decoratormanager.ts`), each keyed by the target value commands in
/// it share.
/// Keyed by the target value the commands share, borrowed from the commands.
#[derive(Debug, Clone, Default)]
pub struct DecoratorMaps<'a> {
    /// Commands targeting a `target.namespace`.
    pub namespace_commands: FxHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    /// Commands targeting a `target.declaration`.
    pub declaration_commands: FxHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    /// Commands targeting a `target.property` (or one entry of
    /// `target.properties`).
    pub property_commands: FxHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    /// Commands targeting a `target.mapElement`.
    pub map_element_commands: FxHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    /// Commands targeting a `target.type`.
    pub type_commands: FxHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
}

/// `DecoratorManager.addDcsWithIndexToMap` (`src/decoratormanager.ts`).
fn add_dcs_with_index_to_map<'a>(
    map: &mut FxHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
    key: &'a str,
    dcs_with_index: DcsIndexWrapper<'a>,
) {
    map.entry(key).or_default().push(dcs_with_index);
}

/// `DecoratorManager.getDecoratorMaps` (`src/decoratormanager.ts`): indexes
/// `commands` by target type. Each command is added to exactly one map,
/// chosen by the first target field present, in this order: `type`,
/// `property`, `properties` (one entry per property named), `mapElement`,
/// `declaration`, `namespace` — matching the reference's `switch (true)`,
/// whose `case`s fall through to nothing (each `break`s) and so also try in
/// that order.
pub fn get_decorator_maps<'a>(commands: impl IntoIterator<Item = &'a Value>) -> DecoratorMaps<'a> {
    let mut maps = DecoratorMaps::default();
    for (index, command) in commands.into_iter().enumerate() {
        let target = command.get("target");
        let dcs = || DcsIndexWrapper { command, index };
        if let Some(t) = target.and_then(|t| t.get("type")).and_then(Value::as_str) {
            add_dcs_with_index_to_map(&mut maps.type_commands, t, dcs());
        } else if let Some(p) = target
            .and_then(|t| t.get("property"))
            .and_then(Value::as_str)
        {
            add_dcs_with_index_to_map(&mut maps.property_commands, p, dcs());
        } else if let Some(ps) = target
            .and_then(|t| t.get("properties"))
            .and_then(Value::as_array)
        {
            for p in ps.iter().filter_map(Value::as_str) {
                add_dcs_with_index_to_map(&mut maps.property_commands, p, dcs());
            }
        } else if let Some(m) = target
            .and_then(|t| t.get("mapElement"))
            .and_then(Value::as_str)
        {
            add_dcs_with_index_to_map(&mut maps.map_element_commands, m, dcs());
        } else if let Some(d) = target
            .and_then(|t| t.get("declaration"))
            .and_then(Value::as_str)
        {
            add_dcs_with_index_to_map(&mut maps.declaration_commands, d, dcs());
        } else if let Some(n) = target
            .and_then(|t| t.get("namespace"))
            .and_then(Value::as_str)
        {
            add_dcs_with_index_to_map(&mut maps.namespace_commands, n, dcs());
        }
    }
    maps
}

/// `DecoratorManager.pushMapValues` (`src/decoratormanager.ts`).
fn push_map_values<'a>(
    out: &mut Vec<DcsIndexWrapper<'a>>,
    map: &FxHashMap<&'a str, Vec<DcsIndexWrapper<'a>>>,
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

/// `DecoratorManager.migrateTo` (`src/decoratormanager.ts`): rewrites every
/// `$class` naming the `org.accordproject.decoratorcommands` namespace,
/// in place, to [`DCS_VERSION`]. TS accepts a `version` parameter but its
/// body only ever substitutes the module-level `DCS_VERSION` constant, never
/// the parameter — this port keeps that (every call site passes
/// [`DCS_VERSION`] as the argument regardless, so the difference is not
/// observable), dropping the unused parameter rather than porting the bug
/// literally (AGENTS.md: idiomatic Rust over dead parameters).
///
/// As in TS, the rewrite reads the version through `ModelUtil.getNamespace`
/// and `ModelUtil.parseNamespace`, whose errors (an unparseable namespace
/// in a nested `$class`) propagate; a namespace with no version leaves the
/// `$class` unchanged (TS `replace(undefined, …)` finds nothing to replace).
pub fn migrate_to(value: &mut Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(class)) = map.get("$class").cloned().as_ref()
                && class.contains("org.accordproject.decoratorcommands")
            {
                let ns = model_util::get_namespace(Some(class))?;
                if let ParsedNamespace::Full {
                    version: Some(version),
                    ..
                } = model_util::parse_namespace_with(Some(ns), false)?
                {
                    // `String.prototype.replace` with a string pattern
                    // replaces only the first occurrence.
                    let migrated = class.replacen(version.as_str(), DCS_VERSION, 1);
                    map.insert("$class".to_string(), Value::String(migrated));
                }
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
/// `parseNamespace` uses (BC-41, P5-38): `semver::Version` itself, so that
/// components above 2^53 compare exactly.
fn parse_version(version: &str) -> Option<semver::Version> {
    semver::Version::parse(version).ok()
}

/// node-semver's `new SemVer(undefined)` (`classes/semver.js`), which
/// `semver.major`/`semver.minor` raise for a `$class` namespace that has no
/// version.
fn semver_not_a_string() -> Error {
    ContractError::pre_port(
        ErrorKind::MalformedInput,
        "Invalid version. Must be a string. Got type \"undefined\".".to_string(),
        None,
    )
    .into()
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

/// `DecoratorManager.canMigrate` (`src/decoratormanager.ts`): whether
/// `decorator_command_set`'s `$class` version can be migrated to
/// `target_version` — same major version, and strictly lower minor version.
/// Its failures are TS's: `ModelUtil.getNamespace` rejects a missing
/// `$class` ("FQN is invalid."), and `ModelUtil.parseNamespace` an invalid
/// namespace, including, since BC-02 (R1, P5-50), one with no version (in
/// TS 5.0.0 node-semver rejected that one, with a `TypeError`).
pub fn can_migrate(decorator_command_set: &Value, target_version: &str) -> Result<bool> {
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
    let input_version = match model_util::parse_namespace_with(Some(ns), false)? {
        ParsedNamespace::Full {
            version: Some(v), ..
        } => v,
        _ => return Err(semver_not_a_string()),
    };
    let (Some(input), Some(target)) =
        (parse_version(&input_version), parse_version(target_version))
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
pub fn check_for_duplicate_decorators(decorated_ast: &Value) -> Result<()> {
    let mut seen = FxHashSet::default();
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
pub fn apply_decorator(
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
/// from `command` (P5-102, C-3), but for a `type` that is not a string,
/// which is spelled as JS would print it.
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

/// `DecoratorManager.checkForNamespaceTargetAndApplyDecorator`
/// (`src/decoratormanager.ts`): applies the decorator to `declaration` only
/// when the command actually targets a declaration (`target.declaration`
/// truthy) — a bare namespace-level command is handled instead by
/// [`execute_namespace_command`], not here.
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
pub fn execute_namespace_command(model: &mut Value, command: &Value) -> Result<()> {
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
    let name = match model_util::parse_namespace_with(namespace.as_deref(), false)? {
        ParsedNamespace::Full { name, .. } | ParsedNamespace::NameOnly { name } => name,
    };
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
pub fn execute_command(
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

/// `DecoratorManager.validateCommand` (`src/decoratormanager.ts`): checks a
/// single command's target resolves against `model_manager` — its
/// `target.type` names a real type, its `target.namespace` (which must be
/// versioned, BC-02) a loaded model, and, together with a `target.namespace` and
/// `target.declaration`, its `target.property`/`target.properties` a real
/// property of that declaration.
///
/// Errors here use [`ContractError::pre_port`] with the reference's own
/// message text: the exact `{kind, code}` catalogue entry (PORTING.md
/// section 2.2) is left for the task that ports `resolveType`'s error
/// messages generally (the module doc comment's divergence note).
pub fn validate_command(model_manager: &ModelManager, command: &Value) -> Result<()> {
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
        // `ModelUtil.parseNamespace(target.namespace)`: since BC-02 (R1,
        // P5-50; maintainer decision on accordproject/concerto-rust#371,
        // option 1) an unversioned target namespace is rejected with the
        // error an invalid namespace throws, instead of matching any
        // version of that namespace as in TS 5.0.0.
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

    // TS: "the guard above throws unless modelFile was resolved for the
    // namespace" — this runs whenever both `namespace` and `declaration` are
    // given, regardless of `property`/`properties`, so a command that names
    // no property still has its declaration checked.
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

        if let Some(property) = target
            .get("property")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            let found = model_manager
                .property(&fqn, property)
                .map(|found| found.map(|(owner, property)| (owner, property.clone())))?;
            if found.is_none() {
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

        if let Some(properties) = target.get("properties").and_then(Value::as_array) {
            for property in properties.iter().filter_map(Value::as_str) {
                let found = model_manager
                    .property(&fqn, property)
                    .map(|found| found.map(|(owner, property)| (owner, property.clone())))?;
                if found.is_none() {
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
    }

    Ok(())
}

/// `BaseModelManager.resolveType` (`src/basemodelmanager.ts`), as
/// `validateCommand` calls it: `type_name` resolves if it is a primitive, or
/// if its namespace has a loaded model file that recognises it (as a local
/// declaration, primitive, or import) under that exact fully-qualified name.
/// Errors use the reference's own catalogue templates
/// (`modelmanager-resolvetype-nonsfortype`/`-notypeinnsforcontext`,
/// `messages/en.json`), so message text matches `BaseModelManager.resolveType`
/// byte for byte.
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

/// The AST of `DCS_MODEL` (`src/decoratormanager.ts`), the CTO text that
/// `DecoratorManager.validate`/`migrateAndValidate` compile with
/// `addCTOModel`. It is concerto-metamodel 3.17.0's `lib/dcsmodel.json`
/// with the one field where the two differ, `concertoVersion`, set to
/// `DCS_MODEL`'s own `">3.0.0"` (checked against the oracle's recorded
/// `DecoratorManager.validate` outcomes).
const DCS_MODEL_AST_JSON: &str = include_str!("dcsmodel.json");

/// The file name `DecoratorManager.validate` gives `DCS_MODEL`.
const VALIDATE_DCS_FILE_NAME: &str = "decoratorcommands@0.3.0.cto";

/// The file name `DecoratorManager.migrateAndValidate` gives `DCS_MODEL`.
const MIGRATE_DCS_FILE_NAME: &str = "decoratorcommands@0.4.0.cto";

/// The `DCS_MODEL` file under `file_name`, read once per thread and file
/// name and then shared (P5-102, C-2), as the system model files and the
/// metamodel file are: a model file is a pure function of its AST and
/// file name, and a manager never changes a registered file. A load error
/// is returned, and not cached.
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

/// The validation model manager `DecoratorManager.validate` and
/// `migrateAndValidate` build: `new ModelManager({ metamodelValidation:
/// true, addMetamodel: true })` (the system models and the metamodel), then
/// `addModelFiles(model_files)` when there are any, then
/// `addCTOModel(DCS_MODEL, dcs_file_name)`, each step validated as TS's
/// `addModelFiles`/`addCTOModel` validate it.
///
/// P5-102 (accordproject/concerto-rust#456, C-2): built from shared files,
/// as TS shares the `ModelFile` objects
/// (`validationModelManager.addModelFiles(modelManager.getModelFiles())`):
/// the start is a [`ModelManager::fork`] of the resident metamodel manager
/// (`instance::metamodel::with_resident_metamodel_manager`, already
/// validated), the caller's files are registered as they are (each with
/// its [`crate::model_manager::ValidityProof`] from the caller's manager,
/// if any) and the DCS model file is the per-thread shared one
/// ([`dcs_model_file`]). Nothing is parsed again. Validation still runs on
/// every file not already known to be valid, in the same order as before,
/// so the errors are unchanged.
///
/// `metamodelValidation` (each added model's AST checked with
/// `Serializer.fromJSON` against the metamodel) is not run, as before.
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
/// manager, as `DecoratorManager.validate`/`migrateAndValidate` call it.
///
/// Its first two steps are hand-ported here rather than left to
/// `Serializer.fromJSON` (`src/serializer.ts`) itself — an instance with no
/// `$class` is rejected, then a truthy non-string `$class` fails the way
/// `ModelUtil.getNamespace`'s `fqn.lastIndexOf('.')` does, which
/// `Serializer::from_json` does not itself reproduce — so an instance of an
/// unknown type fails as TS fails ([`get_type`], TS's own `getType` call).
/// The rest, populating and validating a resource from the JSON, now runs
/// as the rest of `Serializer.fromJSON` does: its `JSONPopulator` walk and
/// `ResourceValidator` pass, ported in full by P3-01b, over plain JSON
/// (`crate::instance::from_json`, P6-01).
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
    // `Factory.newId` and the `dayjs.utc()` clock are never read here: no
    // declaration in `DCS_MODEL` is system-identified or timestamped. So the
    // crate's deterministic environment stands in for them (P5-102, C-12).
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
/// the returned manager, not copied (P5-102, C-2), as TS shares them.
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
/// has already built the way [`validate`] builds its own: the metamodel,
/// the caller's model files and the DCS model, loaded and validated
/// (P5-27, F6: `DecoratorManager.validate` builds that manager in the view,
/// so the engine checks the command set against it instead of building
/// another). Additive; [`validate`] is unchanged.
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

/// `DecoratorManager.migrateAndValidate` (`src/decoratormanager.ts`):
/// migrates each command set's `$class` to [`DCS_VERSION`] in place when
/// `should_migrate` (and [`can_migrate`] allows it), then, when
/// `should_validate` — matching the reference's nesting, *only* then —
/// builds the validation model manager (the metamodel, `model_manager`'s own
/// model files and the DCS model), checks each command set against it
/// (`from_json_against`) and, when also `should_validate_commands`, runs
/// [`validate_command`] over every command against it.
/// `should_validate_commands` alone (`should_validate` false) validates
/// nothing at all, exactly as the reference's `if (shouldValidate) { ...
/// if (shouldValidateCommands) {...} }` does.
pub fn migrate_and_validate(
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
        // the caller's own files, shared (P5-102, C-2).
        let validation_model_manager = validation_model_manager(
            model_manager.user_files_with_proofs(),
            MIGRATE_DCS_FILE_NAME,
        )?;
        for command_set in decorator_command_sets.iter() {
            from_json_against(&validation_model_manager, command_set)?;
            if should_validate_commands {
                // `from_json_against` already established `commands` is an
                // array.
                for command in command_set["commands"].as_array().expect("checked above") {
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
    /// Migrate every command set's `$class` to [`DCS_VERSION`] first.
    pub migrate: bool,
    /// Check every command set against `DCS_MODEL` first
    /// ([`migrate_and_validate`]). Gates [`validate_commands`](Self::validate_commands),
    /// matching the reference.
    pub validate: bool,
    /// Run [`validate_command`] over every command, when [`validate`](Self::validate) is
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
}

/// `DecoratorManager.decorateModels` (`src/decoratormanager.ts`): applies
/// every command of every set in `decorator_command_sets`, in order, across
/// `model_manager`'s loaded models, and returns a new [`ModelManager`] built
/// from the result, as `fromAst` builds one (every model but the system
/// ones, validated unless `disable_metamodel_validation`).
///
/// Like TS, this mutates its arguments: migration rewrites the caller's
/// command sets in place, and `skip_validation_and_resolution` sets
/// `options`' two `disable_*` flags.
///
/// An empty `decorator_command_sets` (TS: a falsy or empty
/// `decoratorCommandSet`) returns a manager over the same model files
/// (shared, with the same options), not validated again, rather than
/// `model_manager` itself (TS returns the same instance; a caller that only
/// reads it back cannot tell).
///
/// Unless `disableMetamodelResolution` is truthy, the models decorated are
/// `getAst(true, true)`'s, every one run through
/// `BaseModelManager.resolveMetaModel` ([`ModelManager::get_ast`]), which
/// adds the resolved `namespace` to each type reference, super type and
/// scalar, and fails as TS does for an import that does not resolve.
pub fn decorate_models(
    model_manager: &ModelManager,
    decorator_command_sets: &mut [Value],
    options: &mut DecorateOptions,
) -> Result<ModelManager> {
    if !prepare_command_sets(model_manager, decorator_command_sets, options)? {
        // The same model files, shared (P5-102, C-2), under the same
        // options, and not validated again.
        return model_manager.new_like_with(model_manager.user_files_with_proofs(), false);
    }
    let prepared = index_commands(decorator_command_sets, options)?;
    apply_decoration(model_manager, &prepared, options)
}

/// What [`index_commands`] computes from the command sets before
/// `decorateModels` reads the model manager's AST: the synthetic imports and
/// the commands indexed by target, borrowed from the command sets (P5-102,
/// C-3: no command is copied).
#[derive(Debug, Clone)]
pub struct PreparedDecoration<'a> {
    decorator_imports: Vec<Value>,
    maps: DecoratorMaps<'a>,
}

/// The first step of [`decorate_models`]: the empty-input early return
/// (`false`), the `skipValidationAndResolution` option check, then
/// migration and validation ([`migrate_and_validate`]), which may rewrite
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

/// The second step of [`decorate_models`], the rest of what
/// `decorateModels` does before it calls `modelManager.getAst(…)`, over the
/// command sets as [`prepare_command_sets`] left them: the synthetic
/// imports and the target maps. Neither step depends on metamodel
/// resolution (see [`decorate_models`]).
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
    // BC-02 (R1, P5-50; maintainer decision on
    // accordproject/concerto-rust#371, option 1): a command's
    // `target.namespace` goes through `ModelUtil.parseNamespace`, so an
    // unversioned one is rejected when the commands are applied, with or
    // without `validateCommands`, instead of matching any version of that
    // namespace as in TS 5.0.0.
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
    let maps = get_decorator_maps(combined_commands);
    Ok(PreparedDecoration {
        decorator_imports,
        maps,
    })
}

/// The last step of [`decorate_models`]: applies `prepared` to every
/// model of `model_manager` (the system ones included, as `getAst(…,
/// true)` returns them), then builds the result as `new ModelManager({
/// decoratorValidation })` and `fromAst(decoratedAst, { disableValidation })`
/// do — every model but the system ones, validated unless
/// `disable_metamodel_validation`. Each decorated AST is moved into the
/// result, not copied (P5-102, C-3), as the extractor's result is.
pub fn apply_decoration(
    model_manager: &ModelManager,
    prepared: &PreparedDecoration<'_>,
    options: &DecorateOptions,
) -> Result<ModelManager> {
    // `options?.disableMetamodelResolution ? getAst(false, true) : getAst(true, true)`.
    let resolve = options.disable_metamodel_resolution != Some(true);
    let mut models = models_of(model_manager.models_ast(resolve, true)?);
    for model in models.iter_mut() {
        decorate_model(model, &prepared.decorator_imports, &prepared.maps)?;
    }

    let mut decorated = ModelManager::new()?;
    decorated.set_decorator_validation(model_manager.decorator_validation().clone());
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

/// The synthetic `ImportType` AST nodes `decorateModels` declares for every
/// command's decorator, and for each of its type-reference arguments, so a
/// decorator applied to a model that does not already import it still
/// resolves. Only entries that end up with a (truthy) namespace — their own,
/// or `default_namespace` — are kept, as TS's trailing `.filter(i =>
/// i.namespace)` does. `commands` holds `None` for a JS `undefined`
/// element; reading `decorator` through one, or `name` through a missing
/// decorator, is the `TypeError` TS raises.
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

    let namespace_name = match model_util::parse_namespace_with(Some(&namespace), false)? {
        ParsedNamespace::Full { name, .. } | ParsedNamespace::NameOnly { name } => name,
    };

    // Detach `declarations` into an owned local: once it is out of `model`,
    // `execute_namespace_command` below (which mutates `model` itself, for a
    // bare namespace-level command) and the declaration it is iterating over
    // are no longer borrowed from the same JSON tree, so both can be passed
    // as `&mut` in the same loop body.
    // `model.declarations.forEach(...)`: a model with no `declarations` is
    // a JS `TypeError`.
    js_read(js_read(Some(model), "declarations")?, "forEach")?;
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

/// The `models` of a [`ModelManager::get_ast`] envelope.
fn models_of(ast: Value) -> Vec<Value> {
    match ast {
        Value::Object(mut m) => match m.remove("models") {
            Some(Value::Array(models)) => models,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// `DecoratorManager.extractDecorators`, `extractVocabularies` or
/// `extractNonVocabDecorators(modelManager, options)`
/// (`src/decoratormanager.ts`), by `action`, with the command sets encoded
/// as JSON text ([`extractor::DecoratorExtractor::extract`]).
///
/// - [`extractor::Action::ExtractAll`] (`extractDecorators`): every
///   decorator of every model, the system models included, extracted into
///   command sets and vocabularies.
/// - [`extractor::Action::ExtractVocab`] (`extractVocabularies`): the
///   vocabulary (`Term`/`Term_*`) decorators only; the command sets are
///   always `[]` (TS returns no `decoratorCommandSet` at all).
/// - [`extractor::Action::ExtractNonVocab`] (`extractNonVocabDecorators`):
///   the non-vocabulary decorators of the user's models only (TS reads
///   `getAst(true)`, without the system namespaces); the vocabularies are
///   always empty (TS returns none).
///
/// With `keep_source`, the result also holds the source models its walk
/// read ([`extractor::ExtractResult::source_models`]; P5-56, T2, F-A2,
/// accordproject/concerto-rust#377), so a caller that keeps them can
/// rebuild the same command sets and vocabularies with
/// [`encode_extract_source`] while `model_manager` is unchanged.
///
/// P5-103 (C-5) collapsed the per-action wrappers and the `Value` route
/// into this one function.
pub fn extract(
    model_manager: &ModelManager,
    options: &ExtractOptions,
    action: extractor::Action,
    keep_source: bool,
) -> Result<extractor::ExtractResult> {
    let include_system = action != extractor::Action::ExtractNonVocab;
    extractor::DecoratorExtractor::new(
        options.remove_decorators_from_model,
        options.locale.clone(),
        DCS_VERSION,
        model_manager.models_ast(true, include_system)?,
        action,
    )
    .extract(keep_source)
}

/// The command sets (JSON text) and vocabularies [`extract`] gives
/// for `action` and `options`, rebuilt from `models`, the source models an
/// earlier [`extract`] keeping its source with the same `action`'s
/// system flag returned (P5-56). The source models are neither resolved nor
/// loaded again, and no result manager is built.
pub fn encode_extract_source(
    models: &[Value],
    options: &ExtractOptions,
    action: extractor::Action,
) -> Result<(String, Vec<String>)> {
    extractor::DecoratorExtractor::new(
        options.remove_decorators_from_model,
        options.locale.clone(),
        DCS_VERSION,
        Value::Null,
        action,
    )
    .encode_source(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `org.acme@1.0.0` with a single `Person { name: String }`.
    fn sample_manager() -> ModelManager {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
        mgr
    }

    #[test]
    fn falsy_or_equal_treats_absent_null_and_empty_string_as_true() {
        assert!(falsy_or_equal(None, &["x"]));
        assert!(falsy_or_equal(Some(&Value::Null), &["x"]));
        assert!(falsy_or_equal(Some(&json!("")), &["x"]));
    }

    #[test]
    fn falsy_or_equal_matches_a_string_by_membership() {
        assert!(falsy_or_equal(Some(&json!("x")), &["x", "y"]));
        assert!(!falsy_or_equal(Some(&json!("z")), &["x", "y"]));
    }

    #[test]
    fn falsy_or_equal_matches_an_array_by_intersection() {
        assert!(falsy_or_equal(Some(&json!(["z", "y"])), &["x", "y"]));
        assert!(!falsy_or_equal(Some(&json!(["z", "w"])), &["x", "y"]));
        // P5-102 (C-3): the intersection's own rules, without building it:
        // an empty array (truthy) intersects nothing, a non-string element
        // is never in the string array, and a repeated one counts once.
        assert!(!falsy_or_equal(Some(&json!([])), &["x"]));
        assert!(!falsy_or_equal(
            Some(&json!([1, null, true])),
            &["1", "null", "true"]
        ));
        assert!(falsy_or_equal(Some(&json!([1, "y", "y"])), &["y"]));
    }

    /// P5-102 (C-10): the compile-time class names are the namespace's.
    #[test]
    fn metamodel_class_names_are_qualified_by_the_metamodel_namespace() {
        use crate::instance::metamodel::METAMODEL_NAMESPACE;
        assert_eq!(
            MAP_DECLARATION_CLASS,
            model_util::qualify(METAMODEL_NAMESPACE, "MapDeclaration")
        );
        assert_eq!(
            IMPORT_TYPE_CLASS,
            model_util::qualify(METAMODEL_NAMESPACE, "ImportType")
        );
    }

    /// P5-102 (C-2): the validation manager shares the metamodel, the DCS
    /// model and the caller's files, and parses none of them again.
    #[test]
    fn validate_shares_every_model_file() {
        let sample = sample_manager();
        let files: Vec<Arc<ModelFile>> = sample
            .shared_model_files()
            .filter(|mf| mf.namespace() == "org.acme@1.0.0")
            .cloned()
            .collect();
        let mgr = validate(&valid_command_set(), Some(&files)).unwrap();
        let shared = |namespace: &str| {
            mgr.shared_model_files()
                .find(|mf| mf.namespace() == namespace)
                .cloned()
                .unwrap()
        };
        assert!(Arc::ptr_eq(&shared("org.acme@1.0.0"), &files[0]));
        assert!(Arc::ptr_eq(
            &shared(crate::instance::metamodel::METAMODEL_NAMESPACE),
            &crate::instance::metamodel::metamodel_model_file().unwrap()
        ));
        let dcs = shared("org.accordproject.decoratorcommands@0.4.0");
        assert!(Arc::ptr_eq(
            &dcs,
            &dcs_model_file(VALIDATE_DCS_FILE_NAME).unwrap()
        ));
        assert_eq!(dcs.file_name(), Some(VALIDATE_DCS_FILE_NAME));
        // The files keep TS's order: system models, metamodel, the
        // caller's, then the DCS model.
        let order: Vec<&str> = mgr.model_files().map(ModelFile::namespace).collect();
        assert_eq!(
            order[order.len() - 3..],
            [
                crate::instance::metamodel::METAMODEL_NAMESPACE,
                "org.acme@1.0.0",
                "org.accordproject.decoratorcommands@0.4.0"
            ]
        );
        // `migrateAndValidate` names the DCS model file its own way.
        let migrate = dcs_model_file(MIGRATE_DCS_FILE_NAME).unwrap();
        assert_eq!(migrate.file_name(), Some(MIGRATE_DCS_FILE_NAME));
        assert!(!Arc::ptr_eq(&migrate, &dcs));
    }

    /// P5-102 (C-2): an invalid model file given to `validate` is still
    /// validated, and fails as before.
    #[test]
    fn validate_still_validates_the_callers_model_files() {
        let mut broken = ModelManager::new().unwrap();
        broken
            .load_model(
                &json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.broken@1.0.0",
                    "declarations": [{
                        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                        "name": "A",
                        "isAbstract": false,
                        "superType": {
                            "$class": "concerto.metamodel@1.0.0.TypeIdentifier",
                            "name": "Missing"
                        },
                        "properties": []
                    }]
                }),
                None,
            )
            .unwrap();
        let files: Vec<Arc<ModelFile>> = broken
            .shared_model_files()
            .filter(|mf| mf.namespace() == "org.broken@1.0.0")
            .cloned()
            .collect();
        let err = validate(&valid_command_set(), Some(&files)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::IllegalModel, "{err}");
    }

    /// P5-102 (C-2): an empty decorate shares the input's model files and
    /// options.
    #[test]
    fn decorate_with_no_command_sets_shares_the_model_files() {
        let sample = sample_manager();
        let same = decorate_models(&sample, &mut [], &mut DecorateOptions::default()).unwrap();
        let mine: Vec<&Arc<ModelFile>> = sample.shared_model_files().collect();
        let theirs: Vec<&Arc<ModelFile>> = same.shared_model_files().collect();
        assert_eq!(mine.len(), theirs.len());
        for (a, b) in mine.iter().zip(&theirs) {
            assert!(Arc::ptr_eq(a, b), "{}", a.namespace());
        }
    }

    #[test]
    fn migrate_to_rewrites_only_the_decoratorcommands_class_version() {
        let mut value = json!({
            "$class": "org.accordproject.decoratorcommands@0.3.0.DecoratorCommandSet",
            "commands": [
                { "$class": "org.accordproject.decoratorcommands@0.3.0.Command", "type": "UPSERT" }
            ],
            "unrelated": { "$class": "concerto.metamodel@1.0.0.Decorator" }
        });
        migrate_to(&mut value).unwrap();
        assert_eq!(
            value["$class"],
            "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet"
        );
        assert_eq!(
            value["commands"][0]["$class"],
            "org.accordproject.decoratorcommands@0.4.0.Command"
        );
        assert_eq!(
            value["unrelated"]["$class"],
            "concerto.metamodel@1.0.0.Decorator"
        );
    }

    #[test]
    fn can_migrate_only_within_the_same_major_and_to_a_strictly_higher_minor() {
        let older =
            json!({ "$class": "org.accordproject.decoratorcommands@0.3.0.DecoratorCommandSet" });
        assert!(can_migrate(&older, DCS_VERSION).unwrap());

        let same =
            json!({ "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet" });
        assert!(!can_migrate(&same, DCS_VERSION).unwrap());

        let other_major =
            json!({ "$class": "org.accordproject.decoratorcommands@1.0.0.DecoratorCommandSet" });
        assert!(!can_migrate(&other_major, DCS_VERSION).unwrap());
    }

    #[test]
    fn can_migrate_takes_strict_semver_only() {
        // BC-41 (P5-38): no leading `v` and no surrounding whitespace, with
        // the error `parseNamespace` throws for any invalid version.
        for class in [
            "org.accordproject.decoratorcommands@v0.3.0.DecoratorCommandSet",
            "org.accordproject.decoratorcommands@ 0.3.0.DecoratorCommandSet",
        ] {
            let err = can_migrate(&json!({ "$class": class }), DCS_VERSION)
                .err()
                .unwrap_or_else(|| panic!("{class} was accepted"));
            assert!(err.to_string().to_lowercase().contains("invalid"), "{err}");
        }
        // BC-02 (P5-50): an unversioned `$class` namespace is
        // `parseNamespace`'s invalid namespace, a plain `Error`.
        let err = can_migrate(
            &json!({ "$class": "org.accordproject.decoratorcommands.DecoratorCommandSet" }),
            DCS_VERSION,
        )
        .unwrap_err();
        assert_eq!(err.contract().kind, ErrorKind::InvalidArgument, "{err}");
        // Components above 2^53 are compared exactly: 2^53 + 1 and 2^53
        // are the same `f64`, but different majors.
        let big = json!({
            "$class": "org.accordproject.decoratorcommands@9007199254740993.0.0.DecoratorCommandSet"
        });
        assert!(!can_migrate(&big, "9007199254740992.1.0").unwrap());
        assert!(can_migrate(&big, "9007199254740993.1.0").unwrap());
    }

    #[test]
    fn check_for_duplicate_decorators_rejects_a_repeated_name() {
        let ast = json!({ "decorators": [ {"name": "Foo"}, {"name": "Foo"} ] });
        let err = match check_for_duplicate_decorators(&ast) {
            Ok(()) => panic!("expected a duplicate-decorator error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("Duplicate decorator Foo"));
    }

    #[test]
    fn check_for_duplicate_decorators_accepts_distinct_names() {
        let ast = json!({ "decorators": [ {"name": "Foo"}, {"name": "Bar"} ] });
        assert!(check_for_duplicate_decorators(&ast).is_ok());
    }

    #[test]
    fn apply_decorator_upsert_replaces_by_name_or_adds_a_new_one() {
        let mut decorated =
            json!({ "decorators": [ {"name": "Foo", "arguments": [{"value": 1}]} ] });
        apply_decorator(
            &mut decorated,
            "UPSERT",
            &json!({"name": "Foo", "arguments": [{"value": 2}]}),
        )
        .unwrap();
        assert_eq!(decorated["decorators"].as_array().unwrap().len(), 1);
        assert_eq!(decorated["decorators"][0]["arguments"][0]["value"], 2);

        apply_decorator(&mut decorated, "UPSERT", &json!({"name": "Bar"})).unwrap();
        assert_eq!(decorated["decorators"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn apply_decorator_append_adds_then_rejects_the_duplicate_it_created() {
        let mut decorated = json!({ "decorators": [ {"name": "Foo"} ] });
        let err = match apply_decorator(&mut decorated, "APPEND", &json!({"name": "Foo"})) {
            Ok(()) => panic!("expected a duplicate-decorator error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("Duplicate decorator Foo"));
        // TS applies (pushes) the decorator, then checks: the duplicate is
        // still there to see, not rolled back.
        assert_eq!(decorated["decorators"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn apply_decorator_rejects_an_unknown_command_type() {
        let mut decorated = json!({});
        let err = match apply_decorator(&mut decorated, "REMOVE", &json!({"name": "Foo"})) {
            Ok(()) => panic!("expected an unknown-command-type error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("Unknown command type REMOVE"));
    }

    #[test]
    fn get_decorator_maps_indexes_by_the_first_target_field_in_priority_order() {
        let commands = vec![
            json!({"target": {"type": "T"}}),
            json!({"target": {"property": "p"}}),
            json!({"target": {"properties": ["a", "b"]}}),
            json!({"target": {"mapElement": "KEY"}}),
            json!({"target": {"declaration": "D"}}),
            json!({"target": {"namespace": "N"}}),
            // `type` wins over `declaration` when a command's target sets both.
            json!({"target": {"type": "T2", "declaration": "D2"}}),
        ];
        let maps = get_decorator_maps(&commands);
        assert_eq!(maps.type_commands.get("T").unwrap().len(), 1);
        assert_eq!(maps.property_commands.get("p").unwrap().len(), 1);
        assert_eq!(maps.property_commands.get("a").unwrap().len(), 1);
        assert_eq!(maps.property_commands.get("b").unwrap().len(), 1);
        assert_eq!(maps.map_element_commands.get("KEY").unwrap().len(), 1);
        assert_eq!(maps.declaration_commands.get("D").unwrap().len(), 1);
        assert_eq!(maps.namespace_commands.get("N").unwrap().len(), 1);
        assert_eq!(maps.type_commands.get("T2").unwrap().len(), 1);
        assert!(!maps.declaration_commands.contains_key("D2"));
    }

    #[test]
    fn validate_command_rejects_a_namespace_that_does_not_exist() {
        let mgr = sample_manager();
        let command = json!({ "target": { "namespace": "does.not.exist@1.0.0" } });
        let err = match validate_command(&mgr, &command) {
            Ok(()) => panic!("expected a namespace-does-not-exist error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn validate_command_rejects_an_unversioned_target_namespace() {
        // BC-02 (P5-50, #371 option 1): `org.acme` no longer matches the
        // loaded `org.acme@1.0.0`; it is `parseNamespace`'s invalid
        // namespace, a plain `Error`.
        let mgr = sample_manager();
        for target in [
            json!({ "namespace": "org.acme" }),
            json!({ "namespace": "org.acme", "declaration": "Person", "property": "name" }),
        ] {
            let err = validate_command(&mgr, &json!({ "target": target })).unwrap_err();
            assert_eq!(err.contract().kind, ErrorKind::InvalidArgument, "{err}");
            assert!(err.to_string().contains("Invalid namespace"), "{err}");
        }
    }

    #[test]
    fn decorate_models_rejects_an_unversioned_target_namespace_without_validation() {
        // BC-02 (P5-50, #371 option 1): applying the commands rejects an
        // unversioned `target.namespace` too, with or without
        // `validateCommands`; a versioned one still applies.
        let mgr = sample_manager();
        let command_set = |namespace: &str| {
            json!({
                "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
                "name": "x",
                "version": "1.0.0",
                "commands": [{
                    "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
                    "type": "UPSERT",
                    "target": {
                        "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget",
                        "namespace": namespace,
                        "declaration": "Person"
                    },
                    "decorator": {
                        "$class": "concerto.metamodel@1.0.0.Decorator",
                        "name": "Hello",
                        "arguments": []
                    }
                }]
            })
        };
        for validate in [false, true] {
            let mut options = DecorateOptions {
                validate,
                validate_commands: validate,
                ..Default::default()
            };
            let mut sets = [command_set("org.acme")];
            let err = decorate_models(&mgr, &mut sets, &mut options).unwrap_err();
            assert_eq!(err.contract().kind, ErrorKind::InvalidArgument, "{err}");
            assert!(err.to_string().contains("Invalid namespace"), "{err}");

            let mut sets = [command_set("org.acme@1.0.0")];
            let decorated = decorate_models(&mgr, &mut sets, &mut options).unwrap();
            let person = &decorated.model_file("org.acme@1.0.0").unwrap().ast()["declarations"][0];
            assert_eq!(
                person["decorators"][0]["name"], "Hello",
                "validate={validate}"
            );
        }
    }

    #[test]
    fn validate_command_accepts_a_real_namespace_declaration_and_property() {
        let mgr = sample_manager();
        let command = json!({
            "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "property": "name" }
        });
        assert!(validate_command(&mgr, &command).is_ok());
    }

    #[test]
    fn validate_command_rejects_a_property_that_does_not_exist() {
        let mgr = sample_manager();
        let command = json!({
            "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "property": "nope" }
        });
        let err = match validate_command(&mgr, &command) {
            Ok(()) => panic!("expected a property-does-not-exist error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn validate_command_rejects_a_declaration_that_does_not_exist() {
        // `test/decoratormanager.js` "#validateCommand should detect invalid
        // target declaration": a namespace that resolves but a declaration
        // that does not, with *no* `property`/`properties` — TS's
        // `resolveType('DecoratorCommand.target.declaration', fqn)` still
        // runs and throws (this is what the missing declaration-resolution
        // check let through silently before this fix).
        let mgr = sample_manager();
        let command =
            json!({ "target": { "namespace": "org.acme@1.0.0", "declaration": "Missing" } });
        let err = match validate_command(&mgr, &command) {
            Ok(()) => panic!("expected a declaration-does-not-exist error"),
            Err(e) => e,
        };
        // TS: `No type "org.acme@1.0.0.Missing" in namespace "org.acme@1.0.0"
        // for "DecoratorCommand.target.declaration".` (golden catalogue text,
        // `modelmanager-resolvetype-notypeinnsforcontext`).
        assert_eq!(
            err.to_string(),
            "No type \"org.acme@1.0.0.Missing\" in namespace \"org.acme@1.0.0\" for \"DecoratorCommand.target.declaration\"."
        );
    }

    #[test]
    fn validate_command_rejects_an_unrecognised_target_type() {
        let mgr = sample_manager();
        let command = json!({ "target": { "type": "concerto.metamodel@1.0.0.Foo" } });
        let err = match validate_command(&mgr, &command) {
            Ok(()) => panic!("expected an unrecognised-type error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("Foo"), "{err}");
    }

    #[test]
    fn validate_command_rejects_properties_containing_a_property_that_does_not_exist() {
        let mgr = sample_manager();
        let command = json!({
            "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "properties": ["name", "nope"] }
        });
        let err = match validate_command(&mgr, &command) {
            Ok(()) => panic!("expected a property-does-not-exist error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn validate_command_rejects_both_property_and_properties() {
        let mgr = sample_manager();
        let command = json!({ "target": { "property": "a", "properties": ["b"] } });
        let err = match validate_command(&mgr, &command) {
            Ok(()) => panic!("expected a property/properties conflict error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("both property and properties"));
    }

    #[test]
    fn decorate_models_applies_a_declaration_level_upsert() {
        let mgr = sample_manager();
        let mut command_set = json!({
            "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
            "name": "test",
            "version": "0.4.0",
            "commands": [{
                "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
                "type": "UPSERT",
                "target": { "namespace": "org.acme@1.0.0", "declaration": "Person" },
                "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important" }
            }]
        });
        let decorated = decorate_models(
            &mgr,
            std::slice::from_mut(&mut command_set),
            &mut DecorateOptions::default(),
        )
        .unwrap();
        let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
        let person = &ast["declarations"][0];
        let names: Vec<&str> = person["decorators"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["Important"]);
    }

    #[test]
    fn decorate_models_applies_a_property_level_upsert() {
        let mgr = sample_manager();
        let mut command_set = json!({
            "commands": [{
                "type": "UPSERT",
                "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "property": "name" },
                "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Required" }
            }]
        });
        let decorated = decorate_models(
            &mgr,
            std::slice::from_mut(&mut command_set),
            &mut DecorateOptions::default(),
        )
        .unwrap();
        let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
        let name_prop = &ast["declarations"][0]["properties"][0];
        assert_eq!(name_prop["decorators"][0]["name"], "Required");
    }

    #[test]
    fn decorate_models_applies_a_bare_namespace_command_to_the_model_itself() {
        let mgr = sample_manager();
        let mut command_set = json!({
            "commands": [{
                "type": "UPSERT",
                "target": {
                    "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget",
                    "namespace": "org.acme@1.0.0"
                },
                "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Stamped" }
            }]
        });
        let decorated = decorate_models(
            &mgr,
            std::slice::from_mut(&mut command_set),
            &mut DecorateOptions::default(),
        )
        .unwrap();
        let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
        assert_eq!(ast["decorators"][0]["name"], "Stamped");
        // A bare namespace target has no `declaration`, so it does not also
        // land on `Person` (`checkForNamespaceTargetAndApplyDecorator`
        // requires `target.declaration`).
        assert!(ast["declarations"][0].get("decorators").is_none());
    }

    #[test]
    fn decorate_models_with_no_command_sets_leaves_the_model_untouched() {
        let mgr = sample_manager();
        let decorated = decorate_models(&mgr, &mut [], &mut DecorateOptions::default()).unwrap();
        let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
        assert!(ast["declarations"][0].get("decorators").is_none());
    }

    #[test]
    fn decorate_models_upsert_replaces_an_existing_decorator_of_the_same_name() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false,
                      "decorators": [ { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important", "arguments": [] } ],
                      "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
        let mut command_set = json!({
            "commands": [{
                "type": "UPSERT",
                "target": { "namespace": "org.acme@1.0.0", "declaration": "Person" },
                "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important",
                    "arguments": [{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "yes"}] }
            }]
        });
        let decorated = decorate_models(
            &mgr,
            std::slice::from_mut(&mut command_set),
            &mut DecorateOptions::default(),
        )
        .unwrap();
        let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
        let decorators = ast["declarations"][0]["decorators"].as_array().unwrap();
        assert_eq!(decorators.len(), 1);
        assert_eq!(decorators[0]["arguments"][0]["value"], "yes");
    }

    fn valid_command_set() -> Value {
        json!({
            "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
            "name": "web",
            "version": "1.0.0",
            "commands": [{
                "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
                "type": "UPSERT",
                "target": { "namespace": "org.acme@1.0.0", "declaration": "Person" },
                "decorator": { "name": "Important" }
            }]
        })
    }

    /// P5-27 (F6): `validate_against` on the manager `validate` built
    /// accepts and rejects what `validate` does, with the same error.
    #[test]
    fn validate_against_matches_validate_on_its_own_manager() {
        let sample = sample_manager();
        let files: Vec<Arc<ModelFile>> = sample
            .shared_model_files()
            .filter(|mf| mf.namespace() == "org.acme@1.0.0")
            .cloned()
            .collect();
        let mgr = validate(&valid_command_set(), Some(&files)).unwrap();
        assert!(validate_against(&mgr, &valid_command_set()).is_ok());

        let mut unknown_type = valid_command_set();
        unknown_type["commands"][0]["type"] = json!("DELETE");
        let mut no_class = valid_command_set();
        no_class.as_object_mut().unwrap().remove("$class");
        let mut unknown_class = valid_command_set();
        unknown_class["$class"] = json!("org.acme@1.0.0.Missing");
        for bad in [
            unknown_type,
            no_class,
            unknown_class,
            json!({ "$class": 1 }),
        ] {
            let expected = validate(&bad, Some(&files)).unwrap_err().to_string();
            let actual = validate_against(&mgr, &bad).unwrap_err().to_string();
            assert_eq!(actual, expected, "{bad}");
        }
    }

    #[test]
    fn migrate_and_validate_with_should_validate_false_accepts_a_structurally_invalid_set_unchanged()
     {
        // Matches the reference: `shouldValidateCommands` alone, with
        // `shouldValidate` false, runs no check at all (TS nests the whole
        // block, including the per-command loop, inside `if (shouldValidate)`).
        let mgr = sample_manager();
        let mut sets = [json!({ "name": "x", "version": "1.0.0" })]; // no "commands" at all
        assert!(migrate_and_validate(&mgr, &mut sets, false, false, true).is_ok());
    }

    #[test]
    fn migrate_and_validate_with_should_validate_true_rejects_a_missing_commands_array() {
        let mgr = sample_manager();
        let mut sets = [json!({
            "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
            "name": "x",
            "version": "1.0.0"
        })];
        let err = migrate_and_validate(&mgr, &mut sets, false, true, false).unwrap_err();
        assert!(err.to_string().contains("commands"), "{err}");
    }

    #[test]
    fn migrate_and_validate_with_should_validate_true_rejects_a_command_missing_target() {
        let mgr = sample_manager();
        let mut command_set = valid_command_set();
        command_set["commands"][0]
            .as_object_mut()
            .unwrap()
            .remove("target");
        let mut sets = [command_set];
        let err = migrate_and_validate(&mgr, &mut sets, false, true, false).unwrap_err();
        assert!(err.to_string().contains("target"), "{err}");
    }

    #[test]
    fn migrate_and_validate_runs_command_validation_only_when_both_flags_are_set() {
        let mgr = sample_manager();
        // A structurally valid command whose target references a namespace
        // that does not exist: only `validate_command` (semantic) catches
        // this, and only when both `should_validate` and
        // `should_validate_commands` are true.
        let mut command_set = valid_command_set();
        command_set["commands"][0]["target"] = json!({ "namespace": "does.not.exist@1.0.0" });
        let mut sets = [command_set.clone()];
        assert!(migrate_and_validate(&mgr, &mut sets, false, true, false).is_ok());

        let mut sets = [command_set];
        let err = migrate_and_validate(&mgr, &mut sets, false, true, true).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn decorate_models_validate_option_rejects_a_structurally_invalid_command_set() {
        let mgr = sample_manager();
        let mut command_set = valid_command_set();
        command_set.as_object_mut().unwrap().remove("commands");
        let mut options = DecorateOptions {
            validate: true,
            ..Default::default()
        };
        let err = decorate_models(&mgr, std::slice::from_mut(&mut command_set), &mut options)
            .unwrap_err();
        assert!(err.to_string().contains("commands"), "{err}");
    }

    #[test]
    fn decorate_models_default_options_skip_the_structural_check_and_fail_as_js_does() {
        // `validate` defaults to `false` (`DecorateOptions::default()`), so
        // the command set is not checked against `DCS_MODEL` first — but TS
        // then flattens `commandSet.commands` (`undefined` here) into one
        // `undefined` command and reads `command.decorator` through it: a
        // `TypeError`, not a silently skipped command set.
        let mgr = sample_manager();
        let mut command_set = valid_command_set();
        command_set.as_object_mut().unwrap().remove("commands");
        let err = decorate_models(
            &mgr,
            std::slice::from_mut(&mut command_set),
            &mut DecorateOptions::default(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Cannot read properties of undefined (reading 'decorator')"
        );
    }

    /// P5-56 (T2, F-A2): on a manager (system models included for
    /// `ExtractAll`/`ExtractVocab`, not for `ExtractNonVocab`),
    /// `extract` keeping its source gives the same result as without, and
    /// `encode_extract_source` over the kept models rebuilds its command
    /// sets and vocabularies.
    #[test]
    fn the_kept_source_rebuilds_the_extracted_command_sets() {
        let mut mgr = sample_manager();
        mgr.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.deco@1.0.0",
                "imports": [{ "$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.acme@1.0.0", "name": "Person" }],
                "decorators": [{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Term",
                    "arguments": [{ "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "Deco" }] }],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Staff", "isAbstract": false,
                      "decorators": [{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Ref",
                        "arguments": [{ "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" }, "isArray": false }] }],
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id", "isArray": false, "isOptional": false,
                          "decorators": [{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Term",
                            "arguments": [{ "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "Id" }] }] }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
        for action in [
            extractor::Action::ExtractAll,
            extractor::Action::ExtractVocab,
            extractor::Action::ExtractNonVocab,
        ] {
            let options = ExtractOptions::default();
            let direct = extract(&mgr, &options, action, false).unwrap();
            let mut kept = extract(&mgr, &options, action, true).unwrap();
            let source = kept.source_models.take().unwrap();
            assert_eq!(kept.decorator_command_set, direct.decorator_command_set);
            assert_eq!(kept.vocabularies, direct.vocabularies);
            let asts = |mm: &ModelManager| {
                mm.model_files()
                    .map(|f| f.ast().clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(asts(&kept.model_manager), asts(&direct.model_manager));
            let system = source
                .iter()
                .any(|m| m.get("namespace").and_then(Value::as_str) == Some("concerto@1.0.0"));
            assert_eq!(system, action != extractor::Action::ExtractNonVocab);
            for locale in ["en", "fr"] {
                let options = ExtractOptions {
                    remove_decorators_from_model: false,
                    locale: locale.to_string(),
                };
                let fresh = extract(&mgr, &options, action, false).unwrap();
                let (sets, vocabularies) =
                    encode_extract_source(&source, &options, action).unwrap();
                assert_eq!(sets, fresh.decorator_command_set, "{action:?} {locale}");
                assert_eq!(vocabularies, fresh.vocabularies, "{action:?} {locale}");
            }
        }
    }
}
