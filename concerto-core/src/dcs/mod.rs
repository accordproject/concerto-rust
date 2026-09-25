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
//! [`ModelFile::ast`] and [`ModelManager::add_models`] the same way, rather
//! than adding a typed `Command`/`DecoratorCommandSet` struct the reference
//! has no counterpart for.
//!
//! ## What stays out of this port (PORTING.md 7.3-style divergence)
//!
//! `DecoratorManager.validate` and the structural-conformance half of
//! `migrateAndValidate` build a throwaway [`ModelManager`], compile
//! `DCS_MODEL` (a `.cto` string) into it with `addCTOModel`, and check the
//! command set against it with `Serializer.fromJSON`. Neither a CTO parser
//! nor a generic `Serializer` exists in this crate yet (`BaseModelManager`'s
//! own `addCTOModel`/`getAst`/`fromAst`/`filter` are ledger-classified for
//! P2-08+P4-08, not this task's P1-04/P1-05/P1-07/P2-07 dependencies), so
//! that check cannot be *run through* `Serializer.fromJSON` here. Instead,
//! [`validate_dcs_structure`] hand-checks the command set against the same
//! `DCS_MODEL` shape directly (required/optional fields, the `CommandType`/
//! `MapElement` enums) — reachable through [`migrate_and_validate`]'s new
//! `should_validate` parameter (`DecorateOptions::validate`), matching where
//! the reference's `shouldValidate` gates both the schema check and, nested
//! inside it, `shouldValidateCommands`'s per-command semantic check. This
//! closes the gap the schema-conformance check exists for (rejecting a
//! command set with no `commands` array, or a command missing `target`),
//! but its error text, class and location are this port's own, not a
//! byte-for-byte match of what `Serializer.fromJSON` throws — that remains
//! unported (module doc comment above). [`decorate_models`] separately
//! builds its `ModelManager` from real [`ModelFile::ast`] values via
//! [`ModelManager::add_models`], not from CTO text — the same substitution
//! `ModelManager::add_model`'s own doc comment already makes for the loader
//! this borrows.
pub mod dcsconverter;
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
/// `test` is JS-falsy (`None`, `Value::Null`, or an empty string — `Value`
/// has no separate `0`/`false` case here, as every caller's `test` is a
/// command target field, always a string, a string array, or absent), an
/// array intersecting `values`, or a string `values` contains.
pub fn falsy_or_equal(test: Option<&Value>, values: &[&str]) -> bool {
    match test {
        None | Some(Value::Null) => true,
        Some(Value::Array(arr)) => {
            let test_strs: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let value_strs: Vec<String> = values.iter().map(|s| (*s).to_string()).collect();
            !intersect(&test_strs, &value_strs).is_empty()
        }
        Some(Value::String(s)) if !s.is_empty() => values.contains(&s.as_str()),
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
pub fn migrate_to(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(class)) = map.get("$class").cloned().as_ref()
                && class.contains("org.accordproject.decoratorcommands")
                && let Ok(ns) = model_util::get_namespace(Some(class))
                && let Ok(parsed) = model_util::parse_namespace(Some(ns), false)
                && let ParsedNamespace::Full {
                    version: Some(version),
                    ..
                } = parsed
            {
                // `String.prototype.replace` with a string pattern replaces
                // only the first occurrence.
                let migrated = class.replacen(version.as_str(), DCS_VERSION, 1);
                map.insert("$class".to_string(), Value::String(migrated));
            }
            for v in map.values_mut() {
                migrate_to(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                migrate_to(v);
            }
        }
        _ => {}
    }
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

/// `DecoratorManager.canMigrate` (`src/decoratormanager.ts`): whether
/// `decorator_command_set`'s `$class` version can be migrated to
/// `target_version` — same major version, and strictly lower minor version.
/// Returns `false` (rather than TS's thrown/`NaN`-propagated failure) when
/// `decorator_command_set.$class` is missing or its version does not parse,
/// since every call site already only reaches this after [`migrate_to`] or
/// [`validate_command`] would themselves have rejected such a command set.
pub fn can_migrate(decorator_command_set: &Value, target_version: &str) -> bool {
    let Some(class) = decorator_command_set.get("$class").and_then(Value::as_str) else {
        return false;
    };
    let Ok(ns) = model_util::get_namespace(Some(class)) else {
        return false;
    };
    let Ok(ParsedNamespace::Full {
        version: Some(input_version),
        ..
    }) = model_util::parse_namespace(Some(ns), false)
    else {
        return false;
    };
    let (Some(input), Some(target)) =
        (parse_version(&input_version), parse_version(target_version))
    else {
        return false;
    };
    input.major == target.major && input.minor < target.minor
}

/// `DecoratorManager.checkForDuplicateDecorators` (`src/decoratormanager.ts`):
/// raises [`ConcertoError::IllegalModel`] if `decorated_ast.decorators` names
/// the same decorator twice.
pub fn check_for_duplicate_decorators(decorated_ast: &Value) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    if let Some(decorators) = decorated_ast.get("decorators").and_then(Value::as_array) {
        for d in decorators {
            let name = d.get("name").and_then(Value::as_str).unwrap_or_default();
            if !seen.insert(name.to_string()) {
                return Err(ConcertoError::IllegalModel {
                    message: format!("Duplicate decorator {name}"),
                    file_name: None,
                    location: decorated_ast.get("location").cloned(),
                });
            }
        }
    }
    Ok(())
}

/// `DecoratorManager.applyDecorator` (`src/decoratormanager.ts`): applies
/// `new_decorator` to `decorated.decorators` — replacing (or adding) the
/// entry of the same name for `"UPSERT"`, or always adding it (then checking
/// for the duplicate it may just have created) for `"APPEND"`.
pub fn apply_decorator(
    decorated: &mut Value,
    command_type: &str,
    new_decorator: &Value,
) -> Result<()> {
    let Value::Object(map) = decorated else {
        return Ok(());
    };
    match command_type {
        "UPSERT" => {
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
    let Value::Object(decl_map) = declaration else {
        return Ok(());
    };
    let Some(decl) = decl_map.get_mut(field) else {
        return Ok(());
    };
    let target_type = target.get("type");
    let matches = match target_type {
        Some(Value::String(t)) if !t.is_empty() => {
            decl.get("$class").and_then(Value::as_str) == Some(t.as_str())
        }
        _ => true,
    };
    if matches {
        apply_decorator(decl, command_type, new_decorator)?;
    }
    Ok(())
}

/// `DecoratorManager.checkForNamespaceTargetAndApplyDecorator`
/// (`src/decoratormanager.ts`): applies the decorator to `declaration` only
/// when the command actually targets a declaration (`target.declaration`
/// set) — a bare namespace-level command is handled instead by
/// [`execute_namespace_command`], not here.
fn check_for_namespace_target_and_apply_decorator(
    declaration: &mut Value,
    command_type: &str,
    decorator: &Value,
    target: &Value,
) -> Result<()> {
    if target.get("declaration").and_then(Value::as_str).is_some() {
        apply_decorator(declaration, command_type, decorator)?;
    }
    Ok(())
}

/// `DecoratorManager.executeNamespaceCommand` (`src/decoratormanager.ts`):
/// applies a bare `{ $class, namespace }` command target — no `declaration`,
/// `property`, `properties`, `type` or `mapElement` — directly to the model
/// itself.
pub fn execute_namespace_command(model: &mut Value, command: &Value) -> Result<()> {
    let Some(target) = command.get("target").cloned() else {
        return Ok(());
    };
    let is_bare_namespace_target = target
        .as_object()
        .is_some_and(|m| m.len() == 2 && m.contains_key("namespace"));
    if !is_bare_namespace_target {
        return Ok(());
    }
    let namespace = model
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let name = match model_util::parse_namespace(Some(&namespace), false) {
        Ok(ParsedNamespace::Full { name, .. }) | Ok(ParsedNamespace::NameOnly { name }) => name,
        Err(_) => return Ok(()),
    };
    if falsy_or_equal(
        target.get("namespace"),
        &[namespace.as_str(), name.as_str()],
    ) {
        let command_type = command
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let decorator = command.get("decorator").cloned().unwrap_or(Value::Null);
        apply_decorator(model, command_type, &decorator)?;
    }
    Ok(())
}

/// `DecoratorManager.executePropertyCommand` (`src/decoratormanager.ts`):
/// applies `command` to `property` when its target names it, by name (or by
/// membership of `target.properties`) and, if given, by `target.type`.
pub fn execute_property_command(property: &mut Value, command: &Value) -> Result<()> {
    let Some(target) = command.get("target").cloned() else {
        return Ok(());
    };
    let has_property_target = target.get("properties").is_some()
        || target.get("property").is_some()
        || target.get("type").is_some();
    if !has_property_target {
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
    let by_name = target.get("property").or_else(|| target.get("properties"));
    if falsy_or_equal(by_name, &[property_name.as_str()])
        && falsy_or_equal(target.get("type"), &[property_class.as_str()])
    {
        let command_type = command
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let decorator = command.get("decorator").cloned().unwrap_or(Value::Null);
        apply_decorator(property, command_type, &decorator)?;
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
    let Some(target) = command.get("target").cloned() else {
        return Ok(());
    };
    let command_type = command
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let decorator = command.get("decorator").cloned().unwrap_or(Value::Null);
    // The namespace version is already validated by `decorate_models`.
    let name = match model_util::parse_namespace(Some(namespace), true) {
        Ok(ParsedNamespace::NameOnly { name }) | Ok(ParsedNamespace::Full { name, .. }) => name,
        Err(_) => return Ok(()),
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
    let is_map = declaration.get("$class").and_then(Value::as_str) == Some(MAP_DECLARATION_CLASS);
    if is_map {
        if let Some(map_element) = target.get("mapElement").and_then(Value::as_str) {
            match map_element {
                "KEY" | "VALUE" => {
                    apply_decorator_for_map_element(
                        map_element,
                        &target,
                        declaration,
                        command_type,
                        &decorator,
                    )?;
                }
                "KEY_VALUE" => {
                    apply_decorator_for_map_element(
                        "KEY",
                        &target,
                        declaration,
                        command_type,
                        &decorator,
                    )?;
                    apply_decorator_for_map_element(
                        "VALUE",
                        &target,
                        declaration,
                        command_type,
                        &decorator,
                    )?;
                }
                _ => {}
            }
        } else if let Some(t) = target.get("type").and_then(Value::as_str) {
            let Value::Object(decl_map) = declaration else {
                return Ok(());
            };
            if let Some(key) = decl_map.get_mut("key")
                && key.get("$class").and_then(Value::as_str) == Some(t)
            {
                apply_decorator(key, command_type, &decorator)?;
            }
            if let Some(value) = decl_map.get_mut("value")
                && value.get("$class").and_then(Value::as_str) == Some(t)
            {
                apply_decorator(value, command_type, &decorator)?;
            }
        } else {
            check_for_namespace_target_and_apply_decorator(
                declaration,
                command_type,
                &decorator,
                &target,
            )?;
        }
    } else if target.get("property").is_none()
        && target.get("properties").is_none()
        && target.get("type").is_none()
    {
        check_for_namespace_target_and_apply_decorator(
            declaration,
            command_type,
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
    let target = command.get("target").cloned().unwrap_or(Value::Null);

    if let Some(t) = target.get("type").and_then(Value::as_str) {
        resolve_type(model_manager, "DecoratorCommand.type", t)?;
    }

    let mut resolved_model_file: Option<&ModelFile> = None;
    if let Some(namespace) = target.get("namespace").and_then(Value::as_str) {
        resolved_model_file = model_manager.model_file(namespace);
        if resolved_model_file.is_none()
            && let Ok(parsed) = model_util::parse_namespace(Some(namespace), false)
            && let ParsedNamespace::Full { name, version, .. } = parsed
            && version.is_none()
        {
            resolved_model_file = model_manager
                .model_files()
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
        target.get("namespace").and_then(Value::as_str),
        target.get("declaration").and_then(Value::as_str),
    ) {
        let model_file = resolved_model_file.expect("guarded above: namespace resolved or errored");
        let fqn = format!("{}.{declaration}", model_file.namespace());
        resolve_type(model_manager, "DecoratorCommand.target.declaration", &fqn)?;
    }

    if target.get("properties").is_some() && target.get("property").is_some() {
        return Err(ContractError::pre_port(
            ErrorKind::Error,
            "Decorator Command references both property and properties. You must either reference a single property or a list of properites.".to_string(),
            None,
        )
        .into());
    }

    if let (Some(namespace), Some(declaration)) = (
        target.get("namespace").and_then(Value::as_str),
        target.get("declaration").and_then(Value::as_str),
    ) {
        let model_file = resolved_model_file.expect("guarded above: namespace resolved or errored");
        let fqn = format!("{}.{declaration}", model_file.namespace());

        if let Some(property) = target.get("property").and_then(Value::as_str) {
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
/// `CommandType`/`MapElement` enums — standing in for
/// `Serializer.fromJSON(decoratorCommandSet)`, which no generic
/// `Serializer`/`addCTOModel` exists yet in this crate to run (module doc
/// comment: `DecoratorManager.validate`/`migrateAndValidate`). Reachable
/// through [`migrate_and_validate`]'s `should_validate` and
/// [`DecorateOptions::validate`].
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

/// `DecoratorManager.migrateAndValidate` (`src/decoratormanager.ts`):
/// migrates each command set's `$class` to [`DCS_VERSION`] when
/// `should_migrate`, then, when `should_validate` — matching the reference's
/// nesting, *only* then — checks each command set against [`DCS_VERSION`]'s
/// shape with [`validate_dcs_structure`] and, when also
/// `should_validate_commands`, runs [`validate_command`] over every command.
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
            if can_migrate(command_set, DCS_VERSION) {
                migrate_to(command_set);
            }
        }
    }
    if should_validate {
        for command_set in decorator_command_sets.iter() {
            validate_dcs_structure(command_set)?;
            if should_validate_commands {
                // `validate_dcs_structure` already established `commands` is
                // an array of objects with a `target`.
                for command in command_set["commands"].as_array().expect("checked above") {
                    validate_command(model_manager, command)?;
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
    /// Check every command set against `DCS_MODEL`'s shape first
    /// ([`validate_dcs_structure`]). Gates [`validate_commands`](Self::validate_commands),
    /// matching the reference (see [`migrate_and_validate`]'s doc comment).
    pub validate: bool,
    /// Run [`validate_command`] over every command, when [`validate`](Self::validate) is
    /// also set.
    pub validate_commands: bool,
    /// The namespace to use for a decorator (or a type-reference argument)
    /// that names none of its own.
    pub default_namespace: Option<String>,
}

/// `DecoratorManager.decorateModels` (`src/decoratormanager.ts`): applies
/// every command of every set in `decorator_command_sets`, in order, across
/// `model_manager`'s (non-system) loaded models, and returns a new
/// [`ModelManager`] built from the result. An empty `decorator_command_sets`
/// returns a manager over the same models unchanged (a fresh `ModelManager`,
/// not `model_manager` itself — TS instead returns the same `modelManager`
/// reference, invisible to a caller that only reads it back).
///
/// TS's synthetic-import step (declaring the decorators and their
/// type-reference arguments as imports of every namespace that gets one
/// applied) and its `skipValidationAndResolution`/`disableMetamodelResolution`/
/// `disableMetamodelValidation` options, both of which route through
/// `BaseModelManager.getAst`'s metamodel-resolution flag, are ported here
/// as: every synthetic import is added (TS's per-model filtering by
/// `i.namespace !== model.namespace` is ported), and the rebuilt manager
/// always validates on load ([`ModelManager::add_models`]) — there is no
/// unvalidated-load option yet ([`ModelManager::add_model`]'s own doc
/// comment already notes the same gap for `addModel`/`addModelFile`).
pub fn decorate_models(
    model_manager: &ModelManager,
    decorator_command_sets: &[Value],
    options: &DecorateOptions,
) -> Result<ModelManager> {
    if decorator_command_sets.is_empty() {
        return rebuild_from(model_manager);
    }

    let mut command_sets: Vec<Value> = decorator_command_sets.to_vec();
    migrate_and_validate(
        model_manager,
        &mut command_sets,
        options.migrate,
        options.validate,
        options.validate_commands,
    )?;

    let combined_commands: Vec<Value> = command_sets
        .iter()
        .filter_map(|cs| cs.get("commands").and_then(Value::as_array))
        .flatten()
        .cloned()
        .collect();

    let decorator_imports =
        synthetic_decorator_imports(&combined_commands, options.default_namespace.as_deref());
    let maps = get_decorator_maps(&combined_commands);

    let mut models: Vec<Value> = model_manager
        .model_files()
        .filter(|mf| !crate::model_manager::EXCLUDE_NS.contains(&mf.namespace()))
        .map(|mf| mf.ast().clone())
        .collect();

    for model in models.iter_mut() {
        decorate_model(model, &decorator_imports, &maps)?;
    }

    let mut fresh = ModelManager::new()?;
    let refs: Vec<(&Value, Option<String>)> = models.iter().map(|m| (m, None)).collect();
    fresh.add_models(refs)?;
    Ok(fresh)
}

/// A fresh [`ModelManager`] over `model_manager`'s own (non-system) models,
/// for [`decorate_models`]'s empty-command-set case.
fn rebuild_from(model_manager: &ModelManager) -> Result<ModelManager> {
    let models: Vec<Value> = model_manager
        .model_files()
        .filter(|mf| !crate::model_manager::EXCLUDE_NS.contains(&mf.namespace()))
        .map(|mf| mf.ast().clone())
        .collect();
    let mut fresh = ModelManager::new()?;
    let refs: Vec<(&Value, Option<String>)> = models.iter().map(|m| (m, None)).collect();
    fresh.add_models(refs)?;
    Ok(fresh)
}

/// The synthetic `ImportType` AST nodes `decorateModels` declares for every
/// command's decorator, and for each of its type-reference arguments, so a
/// decorator applied to a model that does not already import it still
/// resolves. Only entries that end up with a namespace (their own, or
/// `default_namespace`) are kept, as TS's trailing `.filter(i => i.namespace)`
/// does.
fn synthetic_decorator_imports(commands: &[Value], default_namespace: Option<&str>) -> Vec<Value> {
    let mut imports = Vec::new();
    for command in commands {
        let Some(decorator) = command.get("decorator") else {
            continue;
        };
        let name = decorator
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let namespace = decorator
            .get("namespace")
            .and_then(Value::as_str)
            .or(default_namespace);
        imports.push(import_type(name, namespace));

        if let Some(args) = decorator.get("arguments").and_then(Value::as_array) {
            for arg in args {
                let Some(t) = arg.get("type") else { continue };
                let t_name = t.get("name").and_then(Value::as_str).unwrap_or_default();
                let t_namespace = t
                    .get("namespace")
                    .and_then(Value::as_str)
                    .or(default_namespace);
                imports.push(import_type(t_name, t_namespace));
            }
        }
    }
    imports
        .into_iter()
        .filter(|i| {
            i.get("namespace")
                .and_then(Value::as_str)
                .is_some_and(|n| !n.is_empty())
        })
        .collect()
}

fn import_type(name: &str, namespace: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert(
        "$class".to_string(),
        Value::String(IMPORT_TYPE_CLASS.to_string()),
    );
    m.insert("name".to_string(), Value::String(name.to_string()));
    if let Some(ns) = namespace {
        m.insert("namespace".to_string(), Value::String(ns.to_string()));
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

    let needed_imports: Vec<Value> = decorator_imports
        .iter()
        .filter(|i| i.get("namespace").and_then(Value::as_str) != Some(namespace.as_str()))
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

    let namespace_name = match model_util::parse_namespace(Some(&namespace), false) {
        Ok(ParsedNamespace::Full { name, .. }) | Ok(ParsedNamespace::NameOnly { name }) => name,
        Err(_) => namespace.clone(),
    };

    // Detach `declarations` into an owned local: once it is out of `model`,
    // `execute_namespace_command` below (which mutates `model` itself, for a
    // bare namespace-level command) and the declaration it is iterating over
    // are no longer borrowed from the same JSON tree, so both can be passed
    // as `&mut` in the same loop body.
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
        migrate_to(&mut value);
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
        assert!(can_migrate(&older, DCS_VERSION));

        let same =
            json!({ "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet" });
        assert!(!can_migrate(&same, DCS_VERSION));

        let other_major =
            json!({ "$class": "org.accordproject.decoratorcommands@1.0.0.DecoratorCommandSet" });
        assert!(!can_migrate(&other_major, DCS_VERSION));
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
        let command_set = json!({
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
            std::slice::from_ref(&command_set),
            &DecorateOptions::default(),
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
        let command_set = json!({
            "commands": [{
                "type": "UPSERT",
                "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "property": "name" },
                "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Required" }
            }]
        });
        let decorated = decorate_models(
            &mgr,
            std::slice::from_ref(&command_set),
            &DecorateOptions::default(),
        )
        .unwrap();
        let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
        let name_prop = &ast["declarations"][0]["properties"][0];
        assert_eq!(name_prop["decorators"][0]["name"], "Required");
    }

    #[test]
    fn decorate_models_applies_a_bare_namespace_command_to_the_model_itself() {
        let mgr = sample_manager();
        let command_set = json!({
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
            std::slice::from_ref(&command_set),
            &DecorateOptions::default(),
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
        let decorated = decorate_models(&mgr, &[], &DecorateOptions::default()).unwrap();
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
        let command_set = json!({
            "commands": [{
                "type": "UPSERT",
                "target": { "namespace": "org.acme@1.0.0", "declaration": "Person" },
                "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important",
                    "arguments": [{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "yes"}] }
            }]
        });
        let decorated = decorate_models(
            &mgr,
            std::slice::from_ref(&command_set),
            &DecorateOptions::default(),
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
        let mut sets = [json!({ "name": "x", "version": "1.0.0" })];
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
        let options = DecorateOptions {
            validate: true,
            ..Default::default()
        };
        let err = decorate_models(&mgr, std::slice::from_ref(&command_set), &options).unwrap_err();
        assert!(err.to_string().contains("commands"), "{err}");
    }

    #[test]
    fn decorate_models_default_options_still_accept_a_structurally_invalid_command_set() {
        // `validate` defaults to `false` (`DecorateOptions::default()`), so
        // `decorate_models` on its own does not reject this — matching the
        // reference, where `options?.validate` is likewise opt-in.
        let mgr = sample_manager();
        let mut command_set = valid_command_set();
        command_set.as_object_mut().unwrap().remove("commands");
        assert!(
            decorate_models(
                &mgr,
                std::slice::from_ref(&command_set),
                &DecorateOptions::default()
            )
            .is_ok()
        );
    }
}
