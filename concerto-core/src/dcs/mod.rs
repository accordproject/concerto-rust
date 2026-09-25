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
//! ([`from_json_against`]); the rest — the `JSONPopulator` walk and the
//! `ResourceValidator` pass — now runs as the real, ported
//! `Serializer::from_json` (P3-01b, `src/instance/serializer.rs`), raising
//! the same `ValidationException`-style errors TS does.
//! [`validate_dcs_structure`] used to stand in for that; nothing here still
//! calls it (kept for its own unit tests).
//!
//! **Metamodel resolution.** `decorateModels` and the `extract*` statics read
//! `modelManager.getAst(true, …)`, which runs `BaseModelManager.resolveMetaModel`
//! over every model. That is `BaseModelManager`'s (P2-08/P4-08) and is not
//! ported, so this port reads the unresolved AST; see [`decorate_models`].
pub mod dcsconverter;
#[cfg(test)]
mod decoratormanager_tests;
pub mod extractor;
mod yaml_quote;

pub use dcsconverter::{json_to_yaml, yaml_to_json};
pub use yaml_quote::{DECORATOR_STRING_TYPE, quote_string_value};

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::model_file::ModelFile;
use crate::model_manager::ModelManager;
use crate::model_util::{self, ParsedNamespace};

/// `DCS_VERSION` (`src/decoratormanager.ts`): the decorator command set
/// model version this port targets.
pub const DCS_VERSION: &str = "0.4.0";

/// `MetaModelNamespace` (`@accordproject/concerto-metamodel`), as
/// `decoratormanager.ts`/`decoratorextractor.ts` import it.
const META_MODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

const MAP_DECLARATION_CLASS: &str = "concerto.metamodel@1.0.0.MapDeclaration";
const IMPORT_TYPE_CLASS: &str = "concerto.metamodel@1.0.0.ImportType";

/// `intersect(a, b)` (`src/decoratormanager.ts`): the elements `a` and `b`
/// have in common, deduplicated. TS builds this from two `Set`s and returns
/// `Array.from(...)`, whose order follows `Set`'s insertion order — the order
/// elements first appear in `a`.
pub fn intersect(a: &[String], b: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for x in a {
        if b.iter().any(|y| y == x) && seen.insert(x.clone()) {
            out.push(x.clone());
        }
    }
    out
}

/// `falsyOrEqual(test, values)` (`src/decoratormanager.ts`): `true` when
/// `test` is JS-falsy (`None` for `undefined`, `null`, `false`, `0`, `""`),
/// an array intersecting `values`, or a string `values` contains. Any other
/// truthy `test` (a number, `true`, an object) is never in the string array
/// `values` (`Array.prototype.includes` is strict equality).
pub fn falsy_or_equal(test: Option<&Value>, values: &[&str]) -> bool {
    match test {
        Some(Value::Array(arr)) => {
            let test_strs: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let value_strs: Vec<String> = values.iter().map(|s| (*s).to_string()).collect();
            !intersect(&test_strs, &value_strs).is_empty()
        }
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

/// `isUnversionedNamespaceEqual(modelFile, unversionedNamespace)`
/// (`src/decoratormanager.ts`).
fn is_unversioned_namespace_equal(model_file: &ModelFile, unversioned_namespace: &str) -> bool {
    match model_util::parse_namespace(Some(model_file.namespace()), false) {
        Ok(ParsedNamespace::Full { name, .. }) | Ok(ParsedNamespace::NameOnly { name }) => {
            name == unversioned_namespace
        }
        Err(_) => false,
    }
}

/// `DcsIndexWrapper` (`src/decoratormanager.ts`): a decorator command
/// alongside its position in the command set it was collected from, so
/// commands collected from several of [`get_decorator_maps`]'s maps can be
/// put back into command-set order before they run.
#[derive(Debug, Clone)]
pub struct DcsIndexWrapper {
    command: Value,
    index: usize,
}

impl DcsIndexWrapper {
    /// The decorator command.
    pub fn command(&self) -> &Value {
        &self.command
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
#[derive(Debug, Clone, Default)]
pub struct DecoratorMaps {
    /// Commands targeting a `target.namespace`.
    pub namespace_commands: HashMap<String, Vec<DcsIndexWrapper>>,
    /// Commands targeting a `target.declaration`.
    pub declaration_commands: HashMap<String, Vec<DcsIndexWrapper>>,
    /// Commands targeting a `target.property` (or one entry of
    /// `target.properties`).
    pub property_commands: HashMap<String, Vec<DcsIndexWrapper>>,
    /// Commands targeting a `target.mapElement`.
    pub map_element_commands: HashMap<String, Vec<DcsIndexWrapper>>,
    /// Commands targeting a `target.type`.
    pub type_commands: HashMap<String, Vec<DcsIndexWrapper>>,
}

/// `DecoratorManager.addDcsWithIndexToMap` (`src/decoratormanager.ts`).
fn add_dcs_with_index_to_map(
    map: &mut HashMap<String, Vec<DcsIndexWrapper>>,
    key: &str,
    dcs_with_index: DcsIndexWrapper,
) {
    map.entry(key.to_string()).or_default().push(dcs_with_index);
}

/// `DecoratorManager.getDecoratorMaps` (`src/decoratormanager.ts`): indexes
/// `commands` by target type. Each command is added to exactly one map,
/// chosen by the first target field present, in this order: `type`,
/// `property`, `properties` (one entry per property named), `mapElement`,
/// `declaration`, `namespace` — matching the reference's `switch (true)`,
/// whose `case`s fall through to nothing (each `break`s) and so also try in
/// that order.
pub fn get_decorator_maps(commands: &[Value]) -> DecoratorMaps {
    let mut maps = DecoratorMaps::default();
    for (index, command) in commands.iter().enumerate() {
        let target = command.get("target");
        let dcs = || DcsIndexWrapper {
            command: command.clone(),
            index,
        };
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
    out: &mut Vec<&'a DcsIndexWrapper>,
    map: &'a HashMap<String, Vec<DcsIndexWrapper>>,
    key: &str,
) {
    if let Some(values) = map.get(key) {
        out.extend(values.iter());
    }
}

/// Sorts commands collected from [`get_decorator_maps`]'s maps back into
/// command-set order, as every `.sort((a, b) => a.getIndex() - b.getIndex())`
/// call in `decorateModels` does.
fn sorted_by_index(mut commands: Vec<&DcsIndexWrapper>) -> Vec<&DcsIndexWrapper> {
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
                } = model_util::parse_namespace(Some(ns), false)?
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

/// The [`model_util::SemVer`] for a bare version string (`"0.4.0"`), reusing
/// [`model_util::parse_namespace`]'s node-semver-compatible parser rather
/// than duplicating it — this crate exposes no standalone semver parser
/// (`model_util::semver_parse` is private).
fn parse_version(version: &str) -> Option<model_util::SemVer> {
    match model_util::parse_namespace(Some(&format!("x@{version}")), false) {
        Ok(ParsedNamespace::Full { version_parsed, .. }) => version_parsed,
        _ => None,
    }
}

/// node-semver's `new SemVer(undefined)` (`classes/semver.js`), which
/// `semver.major`/`semver.minor` raise for a `$class` namespace that has no
/// version.
fn semver_not_a_string() -> ConcertoError {
    ContractError::pre_port(
        ErrorKind::JsTypeError,
        "Invalid version. Must be a string. Got type \"undefined\".".to_string(),
        None,
    )
    .into()
}

/// A JS `TypeError` for reading `property` of `undefined` (`is_null` false)
/// or `null` (`is_null` true).
fn read_properties_error(is_null: bool, property: &str) -> ConcertoError {
    ContractError::new(
        ErrorKind::JsTypeError,
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
/// `$class` ("FQN is invalid."), `ModelUtil.parseNamespace` an invalid
/// namespace, and node-semver a namespace with no version.
pub fn can_migrate(decorator_command_set: &Value, target_version: &str) -> Result<bool> {
    let class = js_read(Some(decorator_command_set), "$class")?;
    let class = match class {
        Some(Value::String(s)) => Some(s.as_str()),
        // `fqn.lastIndexOf('.')` on a truthy non-string.
        Some(v) if crate::ecma::is_truthy(v) => {
            return Err(ContractError::new(
                ErrorKind::JsTypeError,
                "engine-typeerror-notafunction",
                vec![("expression", "fqn.lastIndexOf".to_string())],
            )
            .into());
        }
        // `if (!fqn)`: every falsy value is rejected with "FQN is invalid.".
        _ => None,
    };
    let ns = model_util::get_namespace(class)?;
    let input_version = match model_util::parse_namespace(Some(ns), false)? {
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
    let mut seen = std::collections::HashSet::new();
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
                ErrorKind::Error,
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

/// A command's `type`, `decorator` and `target`, as `const { target,
/// decorator, type } = command` reads them: `type` as JS would interpolate
/// it into `Unknown command type ${type}` (`"undefined"` when absent), and
/// `decorator` as `null` when absent.
fn command_parts(command: &Value) -> (String, Value, Value) {
    let command_type = command
        .get("type")
        .map_or_else(|| "undefined".to_string(), crate::ecma::to_js_string);
    let decorator = command.get("decorator").cloned().unwrap_or(Value::Null);
    let target = command.get("target").cloned().unwrap_or(Value::Null);
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
    let name = match model_util::parse_namespace(namespace.as_deref(), false)? {
        ParsedNamespace::Full { name, .. } | ParsedNamespace::NameOnly { name } => name,
    };
    let namespace = namespace.unwrap_or_default();
    if falsy_or_equal(
        target.get("namespace"),
        &[namespace.as_str(), name.as_str()],
    ) {
        apply_decorator(model, &command_type, &decorator)?;
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
        apply_decorator(property, &command_type, &decorator)?;
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
    let name = match model_util::parse_namespace(Some(namespace), true)? {
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
                        &target,
                        declaration,
                        &command_type,
                        &decorator,
                    )?;
                }
                Some("KEY_VALUE") => {
                    apply_decorator_for_map_element(
                        "KEY",
                        &target,
                        declaration,
                        &command_type,
                        &decorator,
                    )?;
                    apply_decorator_for_map_element(
                        "VALUE",
                        &target,
                        declaration,
                        &command_type,
                        &decorator,
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
                        apply_decorator(decl, &command_type, &decorator)?;
                    }
                }
            }
        } else {
            check_for_namespace_target_and_apply_decorator(
                declaration,
                &command_type,
                &decorator,
                &target,
            )?;
        }
    } else if truthy("property").is_none()
        && truthy("properties").is_none()
        && truthy("type").is_none()
    {
        check_for_namespace_target_and_apply_decorator(
            declaration,
            &command_type,
            &decorator,
            &target,
        )?;
    } else if let Some(property) = property {
        execute_property_command(property, command)?;
    }
    Ok(())
}

/// `DecoratorManager.validateCommand` (`src/decoratormanager.ts`): checks a
/// single command's target resolves against `model_manager` — its
/// `target.type` names a real type, its `target.namespace` (allowing an
/// unversioned namespace when exactly one loaded model file matches it) a
/// loaded model, and, together with a `target.namespace` and
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
        if resolved_model_file.is_none()
            && let ParsedNamespace::Full { name, version, .. } =
                model_util::parse_namespace(Some(namespace), false)?
            && version.is_none()
        {
            // TS `getModelFiles()`: the user's model files only.
            resolved_model_file = model_manager
                .model_files()
                .filter(|m| !crate::model_manager::EXCLUDE_NS.contains(&m.namespace()))
                .find(|m| is_unversioned_namespace_equal(m, &name));
        }
        if resolved_model_file.is_none() {
            return Err(ContractError::pre_port(
                ErrorKind::Error,
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
            ErrorKind::Error,
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
            let found = model_manager.get_property(&fqn, property)?;
            if found.is_none() {
                return Err(ContractError::pre_port(
                    ErrorKind::Error,
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
                let found = model_manager.get_property(&fqn, property)?;
                if found.is_none() {
                    return Err(ContractError::pre_port(
                        ErrorKind::Error,
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
    let short = model_util::get_short_name(type_name);
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

fn structural_error(message: impl Into<String>) -> ContractError {
    ContractError::pre_port(ErrorKind::Error, message.into(), None)
}

fn require_string_field(obj: &Map<String, Value>, key: &str, context: &str) -> Result<()> {
    match obj.get(key) {
        Some(Value::String(_)) => Ok(()),
        Some(_) => Err(structural_error(format!("{context} must be a string")).into()),
        None => Err(structural_error(format!("{context} is required")).into()),
    }
}

/// A hand-written structural conformance check of `decorator_command_set`
/// against `DCS_MODEL` (`org.accordproject.decoratorcommands@0.4.0`,
/// `src/decoratormanager.ts`) — required/optional fields and the
/// `CommandType`/`MapElement` enums — standing in for the part of
/// `Serializer.fromJSON(decoratorCommandSet)` after its `$class` lookup,
/// for which no `Serializer` exists in this crate yet (module doc).
/// Reachable through [`validate`], [`migrate_and_validate`]'s
/// `should_validate` and [`DecorateOptions::validate`].
///
/// This closes the gap the schema-conformance check exists for — reject a
/// command set with no `commands` array, a command missing `target`, an
/// unrecognised `CommandType`/`MapElement` value — but its error text,
/// class and location are this port's own, not a match for what
/// `Serializer.fromJSON` throws.
pub fn validate_dcs_structure(decorator_command_set: &Value) -> Result<()> {
    let obj = decorator_command_set
        .as_object()
        .ok_or_else(|| structural_error("a decorator command set must be an object"))?;
    require_string_field(obj, "name", "DecoratorCommandSet.name")?;
    require_string_field(obj, "version", "DecoratorCommandSet.version")?;
    if let Some(includes) = obj.get("includes") {
        let arr = includes
            .as_array()
            .ok_or_else(|| structural_error("DecoratorCommandSet.includes must be an array"))?;
        for (i, inc) in arr.iter().enumerate() {
            let inc_obj = inc.as_object().ok_or_else(|| {
                structural_error(format!(
                    "DecoratorCommandSet.includes[{i}] must be an object"
                ))
            })?;
            require_string_field(
                inc_obj,
                "name",
                &format!("DecoratorCommandSet.includes[{i}].name"),
            )?;
            require_string_field(
                inc_obj,
                "version",
                &format!("DecoratorCommandSet.includes[{i}].version"),
            )?;
        }
    }
    let commands = obj
        .get("commands")
        .ok_or_else(|| structural_error("DecoratorCommandSet.commands is required"))?
        .as_array()
        .ok_or_else(|| structural_error("DecoratorCommandSet.commands must be an array"))?;
    for (i, command) in commands.iter().enumerate() {
        validate_command_structure(command, i)?;
    }
    Ok(())
}

fn validate_command_structure(command: &Value, index: usize) -> Result<()> {
    let obj = command
        .as_object()
        .ok_or_else(|| structural_error(format!("commands[{index}] must be an object")))?;
    let target = obj
        .get("target")
        .ok_or_else(|| structural_error(format!("commands[{index}].target is required")))?;
    validate_command_target_structure(target, index)?;
    let decorator = obj
        .get("decorator")
        .ok_or_else(|| structural_error(format!("commands[{index}].decorator is required")))?;
    validate_decorator_structure(decorator, index)?;
    let ty = obj
        .get("type")
        .ok_or_else(|| structural_error(format!("commands[{index}].type is required")))?
        .as_str()
        .ok_or_else(|| structural_error(format!("commands[{index}].type must be a string")))?;
    if !matches!(ty, "UPSERT" | "APPEND") {
        return Err(structural_error(format!(
            "commands[{index}].type must be UPSERT or APPEND, found {ty:?}"
        ))
        .into());
    }
    if let Some(dn) = obj.get("decoratorNamespace")
        && !dn.is_string()
    {
        return Err(structural_error(format!(
            "commands[{index}].decoratorNamespace must be a string"
        ))
        .into());
    }
    Ok(())
}

fn validate_command_target_structure(target: &Value, index: usize) -> Result<()> {
    let obj = target
        .as_object()
        .ok_or_else(|| structural_error(format!("commands[{index}].target must be an object")))?;
    for key in ["namespace", "declaration", "property", "type"] {
        if let Some(v) = obj.get(key)
            && !v.is_string()
        {
            return Err(structural_error(format!(
                "commands[{index}].target.{key} must be a string"
            ))
            .into());
        }
    }
    if let Some(v) = obj.get("properties") {
        let arr = v.as_array().ok_or_else(|| {
            structural_error(format!(
                "commands[{index}].target.properties must be an array"
            ))
        })?;
        if arr.iter().any(|p| !p.is_string()) {
            return Err(structural_error(format!(
                "commands[{index}].target.properties must be an array of strings"
            ))
            .into());
        }
    }
    if let Some(v) = obj.get("mapElement") {
        let s = v.as_str().ok_or_else(|| {
            structural_error(format!(
                "commands[{index}].target.mapElement must be a string"
            ))
        })?;
        if !matches!(s, "KEY" | "VALUE" | "KEY_VALUE") {
            return Err(structural_error(format!(
                "commands[{index}].target.mapElement must be KEY, VALUE or KEY_VALUE, found {s:?}"
            ))
            .into());
        }
    }
    Ok(())
}

fn validate_decorator_structure(decorator: &Value, index: usize) -> Result<()> {
    let obj = decorator.as_object().ok_or_else(|| {
        structural_error(format!("commands[{index}].decorator must be an object"))
    })?;
    require_string_field(obj, "name", &format!("commands[{index}].decorator.name"))?;
    if let Some(args) = obj.get("arguments") {
        let arr = args.as_array().ok_or_else(|| {
            structural_error(format!(
                "commands[{index}].decorator.arguments must be an array"
            ))
        })?;
        for (j, arg) in arr.iter().enumerate() {
            if !arg.is_object() {
                return Err(structural_error(format!(
                    "commands[{index}].decorator.arguments[{j}] must be an object"
                ))
                .into());
            }
        }
    }
    Ok(())
}

/// `MetaModelUtil.metaModelAst` (`@accordproject/concerto-metamodel`
/// 3.17.0's `lib/metamodel.json`, the copy `concerto-metamodel/vendor/`
/// pins by checksum): the model `new ModelManager({ addMetamodel: true })`
/// adds.
const METAMODEL_AST_JSON: &str = include_str!("metamodel.json");

/// The AST of `DCS_MODEL` (`src/decoratormanager.ts`), the CTO text that
/// `DecoratorManager.validate`/`migrateAndValidate` compile with
/// `addCTOModel`. It is concerto-metamodel 3.17.0's `lib/dcsmodel.json`
/// with the one field where the two differ, `concertoVersion`, set to
/// `DCS_MODEL`'s own `">3.0.0"` (checked against the oracle's recorded
/// `DecoratorManager.validate` outcomes).
const DCS_MODEL_AST_JSON: &str = include_str!("dcsmodel.json");

/// `new ModelManager({ metamodelValidation: true, addMetamodel: true })`,
/// the validation model manager `DecoratorManager.validate` and
/// `migrateAndValidate` build: the decorator and root models, then the
/// metamodel, added and validated as the constructor's `addModelFile` does.
///
/// `metamodelValidation` (each added model's AST checked with
/// `Serializer.fromJSON` against the metamodel) is not run:
/// `Serializer.fromJSON` is not ported yet (P3-01b).
fn new_validation_model_manager() -> Result<ModelManager> {
    let mut model_manager = ModelManager::new()?;
    let metamodel: Value =
        serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored metamodel AST is JSON");
    model_manager.add_models([(&metamodel, Some(META_MODEL_NAMESPACE.to_string()))])?;
    Ok(model_manager)
}

/// `validationModelManager.addModelFiles(modelFiles)`: `model_files`' ASTs,
/// under their own file names, added and validated together.
fn add_model_files(model_manager: &mut ModelManager, model_files: &[&ModelFile]) -> Result<()> {
    model_manager.add_models(
        model_files
            .iter()
            .map(|mf| (mf.ast(), mf.file_name().map(str::to_string))),
    )?;
    Ok(())
}

/// `validationModelManager.addCTOModel(DCS_MODEL, file_name)`.
fn add_dcs_model(model_manager: &mut ModelManager, file_name: &str) -> Result<()> {
    let dcs_model: Value =
        serde_json::from_str(DCS_MODEL_AST_JSON).expect("the DCS model AST is JSON");
    model_manager.add_models([(&dcs_model, Some(file_name.to_string()))])?;
    Ok(())
}

/// `Factory.newId`/the `dayjs.utc()` clock `Serializer.fromJSON` reads while
/// building and validating a decorator command set instance
/// ([`from_json_against`]). Neither is reachable in practice: no
/// declaration in `DCS_MODEL` is system-identified or timestamped, so this
/// exists only to satisfy [`crate::instance::InstanceEnv`].
struct DcsInstanceEnv;

impl crate::instance::InstanceEnv for DcsInstanceEnv {
    fn new_id(&mut self) -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("dcs-unused-id-{n:016x}")
    }

    fn now_ms(&mut self) -> f64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_millis() as f64)
    }
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
/// `ResourceValidator` pass, ported in full by P3-01b
/// (`src/instance/serializer.rs`). [`validate_dcs_structure`] used to stand
/// in for that; it is kept only for its own unit tests below, and is no
/// longer reachable from here.
fn from_json_against(model_manager: &ModelManager, instance: &Value) -> Result<()> {
    let class = js_read(Some(instance), "$class")?;
    let class = match class {
        Some(Value::String(s)) if !s.is_empty() => s.as_str(),
        Some(v) if crate::ecma::is_truthy(v) => {
            // `ModelUtil.getNamespace` calls `fqn.lastIndexOf('.')`.
            return Err(ContractError::new(
                ErrorKind::JsTypeError,
                "engine-typeerror-notafunction",
                vec![("expression", "fqn.lastIndexOf".to_string())],
            )
            .into());
        }
        _ => {
            return Err(ContractError::pre_port(
                ErrorKind::Error,
                "Invalid JSON data. Does not contain a $class type identifier.".to_string(),
                None,
            )
            .into());
        }
    };
    get_type(model_manager, class)?;
    let serializer = crate::instance::Serializer::new(true, true, None)
        .expect("Serializer::new with a truthy factory and model manager cannot fail");
    let json_instance = crate::instance::JsValue::from_json(instance);
    let mut env = DcsInstanceEnv;
    serializer
        .from_json(model_manager, &json_instance, None, &mut env)
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
                (
                    "type",
                    model_util::get_short_name(qualified_name).to_string(),
                ),
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
/// ([`from_json_against`]), and returns it.
pub fn validate(
    decorator_command_set: &Value,
    model_files: Option<&[&ModelFile]>,
) -> Result<ModelManager> {
    let mut validation_model_manager = new_validation_model_manager()?;
    if let Some(model_files) = model_files {
        add_model_files(&mut validation_model_manager, model_files)?;
    }
    add_dcs_model(&mut validation_model_manager, "decoratorcommands@0.3.0.cto")?;
    from_json_against(&validation_model_manager, decorator_command_set)?;
    Ok(validation_model_manager)
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
/// ([`from_json_against`]) and, when also `should_validate_commands`, runs
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
        let mut validation_model_manager = new_validation_model_manager()?;
        let user_files: Vec<&ModelFile> = model_manager
            .model_files()
            .filter(|mf| !crate::model_manager::EXCLUDE_NS.contains(&mf.namespace()))
            .collect();
        add_model_files(&mut validation_model_manager, &user_files)?;
        add_dcs_model(&mut validation_model_manager, "decoratorcommands@0.4.0.cto")?;
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
/// `decoratorCommandSet`) returns a manager over the same model files,
/// unvalidated as they were, rather than `model_manager` itself (TS returns
/// the same instance; a caller that only reads it back cannot tell).
///
/// **Metamodel resolution is not ported.** Unless
/// `disableMetamodelResolution`, TS decorates `getAst(true, true)`, whose
/// every model `BaseModelManager.resolveMetaModel` has run over (adding the
/// resolved `namespace` to each type reference, super type and scalar); this
/// port decorates `getAst(false, true)` either way, since `resolveMetaModel`
/// belongs to `BaseModelManager` (P2-08/P4-08). The two agree whenever
/// resolution changes nothing, and otherwise differ only in those resolved
/// `namespace` fields.
pub fn decorate_models(
    model_manager: &ModelManager,
    decorator_command_sets: &mut [Value],
    options: &mut DecorateOptions,
) -> Result<ModelManager> {
    match prepare_decoration(model_manager, decorator_command_sets, options)? {
        None => {
            let mut same = ModelManager::new()?;
            same.set_decorator_validation(model_manager.decorator_validation().clone());
            for mf in model_manager
                .model_files()
                .filter(|mf| !crate::model_manager::EXCLUDE_NS.contains(&mf.namespace()))
            {
                same.add_model(mf.ast(), mf.file_name().map(str::to_string))?;
            }
            Ok(same)
        }
        Some(prepared) => apply_decoration(model_manager, &prepared, options),
    }
}

/// What [`prepare_decoration`] computes from the command sets before
/// `decorateModels` reads the model manager's AST: the synthetic imports and
/// the commands indexed by target.
#[derive(Debug, Clone)]
pub struct PreparedDecoration {
    decorator_imports: Vec<Value>,
    maps: DecoratorMaps,
}

/// The first half of [`decorate_models`], everything `decorateModels` does
/// before it calls `modelManager.getAst(…)`: the empty-input early return
/// (`None`), the `skipValidationAndResolution` option check, migration and
/// validation ([`migrate_and_validate`]), then the synthetic imports and
/// the target maps. It is the half that never depends on metamodel
/// resolution (see [`decorate_models`]).
pub fn prepare_decoration(
    model_manager: &ModelManager,
    decorator_command_sets: &mut [Value],
    options: &mut DecorateOptions,
) -> Result<Option<PreparedDecoration>> {
    if decorator_command_sets.is_empty() {
        return Ok(None);
    }

    if options.skip_validation_and_resolution {
        if options.disable_metamodel_resolution == Some(false)
            || options.disable_metamodel_validation == Some(false)
        {
            return Err(ContractError::pre_port(
                ErrorKind::Error,
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

    // `decoratorCommandSets.flatMap(commandSet => commandSet.commands)`: an
    // array of commands is spread, anything else (`undefined` included) is
    // kept as one element.
    let mut combined_commands: Vec<Option<Value>> = Vec::new();
    for command_set in decorator_command_sets.iter() {
        match js_read(Some(command_set), "commands")? {
            Some(Value::Array(commands)) => {
                combined_commands.extend(commands.iter().cloned().map(Some));
            }
            other => combined_commands.push(other.cloned()),
        }
    }

    let decorator_imports =
        synthetic_decorator_imports(&combined_commands, options.default_namespace.as_ref())?;
    // Every element is a command object: `synthetic_decorator_imports` has
    // already read `command.decorator` from each.
    let combined_commands: Vec<Value> = combined_commands.into_iter().flatten().collect();
    let maps = get_decorator_maps(&combined_commands);
    Ok(Some(PreparedDecoration {
        decorator_imports,
        maps,
    }))
}

/// The second half of [`decorate_models`]: applies `prepared` to every
/// model of `model_manager` (the system ones included, as `getAst(…,
/// true)` returns them), then builds the result as `new ModelManager({
/// decoratorValidation })` and `fromAst(decoratedAst, { disableValidation })`
/// do — every model but the system ones, validated unless
/// `disable_metamodel_validation`.
pub fn apply_decoration(
    model_manager: &ModelManager,
    prepared: &PreparedDecoration,
    options: &DecorateOptions,
) -> Result<ModelManager> {
    let mut models: Vec<Value> = model_manager
        .model_files()
        .map(|mf| mf.ast().clone())
        .collect();
    for model in models.iter_mut() {
        decorate_model(model, &prepared.decorator_imports, &prepared.maps)?;
    }

    let mut decorated = ModelManager::new()?;
    decorated.set_decorator_validation(model_manager.decorator_validation().clone());
    for model in models.iter().filter(|m| {
        !m.get("namespace")
            .and_then(Value::as_str)
            .is_some_and(|ns| crate::model_manager::EXCLUDE_NS.contains(&ns))
    }) {
        decorated.add_model(model, None)?;
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
    commands: &[Option<Value>],
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
        let decorator = js_read(command.as_ref(), "decorator")?;
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
                    ErrorKind::JsTypeError,
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
    maps: &DecoratorMaps,
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

    let namespace_name = match model_util::parse_namespace(Some(&namespace), false)? {
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

/// `modelManager.getAst(true, include_concerto_namespaces)`, less the
/// metamodel resolution (see [`decorate_models`]' doc comment: TS resolves
/// here too, and `resolveMetaModel` belongs to `BaseModelManager`).
fn source_ast(model_manager: &ModelManager, include_concerto_namespaces: bool) -> Value {
    let models: Vec<Value> = model_manager
        .model_files()
        .filter(|mf| {
            include_concerto_namespaces
                || !crate::model_manager::EXCLUDE_NS.contains(&mf.namespace())
        })
        .map(|mf| mf.ast().clone())
        .collect();
    let mut m = Map::new();
    m.insert(
        "$class".to_string(),
        Value::String(format!("{META_MODEL_NAMESPACE}.Models")),
    );
    m.insert("models".to_string(), Value::Array(models));
    Value::Object(m)
}

/// `DecoratorManager.extractDecorators(modelManager, options)`
/// (`src/decoratormanager.ts`): every decorator of every model, the system
/// models included, extracted into command sets and vocabularies.
pub fn extract_decorators(
    model_manager: &ModelManager,
    options: &ExtractOptions,
) -> Result<extractor::ExtractResult> {
    extractor::DecoratorExtractor::new(
        options.remove_decorators_from_model,
        options.locale.clone(),
        DCS_VERSION,
        source_ast(model_manager, true),
        extractor::Action::ExtractAll,
    )
    .extract()
}

/// `DecoratorManager.extractVocabularies(modelManager, options)`
/// (`src/decoratormanager.ts`): the vocabulary (`Term`/`Term_*`) decorators
/// only; the result's `decorator_command_set` is always empty (TS returns
/// no `decoratorCommandSet` at all).
pub fn extract_vocabularies(
    model_manager: &ModelManager,
    options: &ExtractOptions,
) -> Result<extractor::ExtractResult> {
    extractor::DecoratorExtractor::new(
        options.remove_decorators_from_model,
        options.locale.clone(),
        DCS_VERSION,
        source_ast(model_manager, true),
        extractor::Action::ExtractVocab,
    )
    .extract()
}

/// `DecoratorManager.extractNonVocabDecorators(modelManager, options)`
/// (`src/decoratormanager.ts`): the non-vocabulary decorators of the user's
/// models only (TS reads `getAst(true)`, without the system namespaces); the
/// result's `vocabularies` is always empty (TS returns none).
pub fn extract_non_vocab_decorators(
    model_manager: &ModelManager,
    options: &ExtractOptions,
) -> Result<extractor::ExtractResult> {
    extractor::DecoratorExtractor::new(
        options.remove_decorators_from_model,
        options.locale.clone(),
        DCS_VERSION,
        source_ast(model_manager, false),
        extractor::Action::ExtractNonVocab,
    )
    .extract()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `org.acme@1.0.0` with a single `Person { name: String }`.
    fn sample_manager() -> ModelManager {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
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
    fn intersect_keeps_first_appearance_order_and_drops_duplicates() {
        let a = vec![
            "b".to_string(),
            "a".to_string(),
            "a".to_string(),
            "c".to_string(),
        ];
        let b = vec!["a".to_string(), "c".to_string()];
        assert_eq!(intersect(&a, &b), vec!["a".to_string(), "c".to_string()]);
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
        mgr.add_model(
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

    #[test]
    fn validate_dcs_structure_accepts_a_well_formed_command_set() {
        assert!(validate_dcs_structure(&valid_command_set()).is_ok());
    }

    #[test]
    fn validate_dcs_structure_rejects_a_command_set_with_no_commands_array() {
        // The bug this closes: `.and_then(Value::as_array)` silently
        // skipping a missing `commands` (used to make `migrate_and_validate`
        // accept this).
        let mut command_set = valid_command_set();
        command_set.as_object_mut().unwrap().remove("commands");
        let err = validate_dcs_structure(&command_set).unwrap_err();
        assert!(err.to_string().contains("commands"), "{err}");
    }

    #[test]
    fn validate_dcs_structure_rejects_a_command_missing_target() {
        let mut command_set = valid_command_set();
        command_set["commands"][0]
            .as_object_mut()
            .unwrap()
            .remove("target");
        let err = validate_dcs_structure(&command_set).unwrap_err();
        assert!(err.to_string().contains("target"), "{err}");
    }

    #[test]
    fn validate_dcs_structure_rejects_an_unknown_command_type() {
        let mut command_set = valid_command_set();
        command_set["commands"][0]["type"] = json!("DELETE");
        assert!(validate_dcs_structure(&command_set).is_err());
    }

    #[test]
    fn validate_dcs_structure_rejects_a_non_object_command_set() {
        assert!(validate_dcs_structure(&json!("not a command set")).is_err());
        assert!(validate_dcs_structure(&json!(null)).is_err());
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
}
