//! A port of `src/dcsconverter.ts`: `jsonToYaml`/`yamlToJson`, the DCS/YAML
//! converter the extension (`decoratorcommands` CLI et al.) uses to let a
//! `DecoratorCommandSet` be hand-edited as a short, `failsafe`-schema YAML
//! document instead of its full JSON `$class`-tagged form.
//!
//! Both directions stay on the untyped metamodel AST ([`serde_json::Value`]),
//! matching [`super`]/[`super::extractor`].
//!
//! ## Scope
//!
//! The reference calls straight into the `yaml` npm library
//! (`yaml.stringify`/`yaml.parse`, both with `{schema: 'failsafe'}`), a full
//! YAML 1.2 implementation. This port does not carry one; it reimplements
//! exactly the block-mapping/block-sequence/plain-or-double-quoted-scalar
//! subset that `jsonToYaml`'s fixed output shape ever produces (and that a
//! DCS `.yaml` file, being a `jsonToYaml` round trip or a direct edit of
//! one, is expected to use): no flow collections (`[a, b]`/`{a: b}`), no
//! anchors/aliases/tags, no block scalars (`|`/`>`), no comments. Verified
//! byte-for-byte against the reference (`yaml@2.8.3`) on
//! `test/data/decoratorcommands/possible-decorator-command-targets.{json,yaml}`
//! and the inline fixtures of `test/dcsconverter.js`.
//!
//! [`render_scalar_failsafe`]'s quoting decision (plain vs. `JSON.stringify`
//! double-quoted) matches the reference for every value these two functions
//! ever build or accept: DCS identifiers, namespaces, decorator argument
//! text stringified with [`js_value_to_string`]. It diverges, documented,
//! from `yaml.stringify`'s own choice of *style* — a single-quoted
//! rendering when a value has more `"` than `'`, or a block-literal (`|-`)
//! rendering for an embedded newline or a document-marker-like value
//! (`"---"`, `"..."`) — none of which the DCS data this converter round-trips
//! is expected to contain; see [`yaml_quote::needs_quoting_failsafe`].
use serde_json::{Map, Number, Value};

use crate::error::{ContractError, ErrorKind, Result};
use crate::model_util::{self, ParsedNamespace};

use super::{META_MODEL_NAMESPACE, yaml_quote};

/// A parsed/to-be-rendered YAML document value: exactly the shapes
/// `jsonToYaml`'s output (and a `yamlToJson` input) ever takes — a block
/// mapping (order preserved), a block sequence, or a scalar. No flow style,
/// no tags: see the module doc comment.
#[derive(Debug, Clone, PartialEq)]
enum Yaml {
    Scalar(String),
    Seq(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
}

fn pre_port(message: impl Into<String>) -> ContractError {
    ContractError::pre_port(ErrorKind::Error, message.into(), None)
}

// ---------------------------------------------------------------------
// jsonToYaml
// ---------------------------------------------------------------------

/// `String(value)` (`dcsconverter.ts`'s `handleArguments`/`handleTarget`
/// call sites, JS's implicit coercion in a template-ish position): the JS
/// stringification of a decorator argument's `value`, which is always a
/// JSON string, number or boolean by construction of the AST
/// (`DecoratorString`/`DecoratorNumber`/`DecoratorBoolean`). Matches
/// `Number.prototype.toString()`/`Boolean.prototype.toString()` for every
/// finite value these argument types actually carry (whole and decimal
/// literals as the CTO grammar accepts them); not a general `Number#toString`
/// port (`NaN`/`Infinity`/exponent notation never reach a decorator
/// argument).
fn js_value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        // Not part of the reference's domain (a DecoratorString/Number/Boolean
        // argument's `value` is always one of the above); render `null`/
        // arrays/objects the way `String()` would rather than panic.
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// `handleTarget` (`src/dcsconverter.ts`): `target`'s own keys, in order,
/// minus `$class`.
fn handle_target(target: &Value) -> Yaml {
    let mut entries = Vec::new();
    if let Some(obj) = target.as_object() {
        for (key, value) in obj {
            if key == "$class" {
                continue;
            }
            entries.push((key.clone(), value_to_yaml(value)));
        }
    }
    Yaml::Map(entries)
}

/// A target field's value ([`String`] or `String[]`, the only shapes
/// `CommandTarget`'s properties take) as YAML.
fn value_to_yaml(value: &Value) -> Yaml {
    match value {
        Value::Array(items) => Yaml::Seq(items.iter().map(value_to_yaml).collect()),
        Value::Object(_) => handle_target(value),
        other => Yaml::Scalar(js_value_to_string(other)),
    }
}

/// `handleArguments` (`src/dcsconverter.ts`).
fn handle_arguments(argument: &Value) -> Yaml {
    let class = argument
        .get("$class")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if class.ends_with("TypeReference") {
        let ty = argument.get("type").cloned().unwrap_or(Value::Null);
        let mut type_reference = Vec::new();
        type_reference.push((
            "name".to_string(),
            Yaml::Scalar(js_value_to_string(ty.get("name").unwrap_or(&Value::Null))),
        ));
        if let Some(ns) = ty.get("namespace") {
            type_reference.push((
                "namespace".to_string(),
                Yaml::Scalar(js_value_to_string(ns)),
            ));
        }
        if let Some(rn) = ty.get("resolvedName") {
            type_reference.push((
                "resolvedName".to_string(),
                Yaml::Scalar(js_value_to_string(rn)),
            ));
        }
        // `String(argument.isArray)`: `"undefined"` when it has none.
        let is_array = argument
            .get("isArray")
            .map_or_else(|| "undefined".to_string(), js_value_to_string);
        type_reference.push(("isArray".to_string(), Yaml::Scalar(is_array)));
        return Yaml::Map(vec![(
            "typeReference".to_string(),
            Yaml::Map(type_reference),
        )]);
    }
    let type_name = match class {
        c if c == format!("{META_MODEL_NAMESPACE}.DecoratorString") => "String",
        c if c == format!("{META_MODEL_NAMESPACE}.DecoratorNumber") => "Number",
        c if c == format!("{META_MODEL_NAMESPACE}.DecoratorBoolean") => "Boolean",
        _ => "",
    };
    let value = argument.get("value").cloned().unwrap_or(Value::Null);
    Yaml::Map(vec![
        ("type".to_string(), Yaml::Scalar(type_name.to_string())),
        (
            "value".to_string(),
            Yaml::Scalar(js_value_to_string(&value)),
        ),
    ])
}

/// `handleDecorator` (`src/dcsconverter.ts`): an empty `arguments` array is
/// omitted entirely, matching `arguments?.length === 0 ? undefined : ...`
/// and `yaml.stringify`'s own dropping of `undefined`-valued keys.
fn handle_decorator(decorator: &Value) -> Yaml {
    let name = decorator
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut entries = vec![("name".to_string(), Yaml::Scalar(name.to_string()))];
    if let Some(args) = decorator.get("arguments").and_then(Value::as_array)
        && !args.is_empty()
    {
        entries.push((
            "arguments".to_string(),
            Yaml::Seq(args.iter().map(handle_arguments).collect()),
        ));
    }
    Yaml::Map(entries)
}

/// `handleCommands` (`src/dcsconverter.ts`).
fn handle_command(command: &Value) -> Yaml {
    let action = command
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let target = command.get("target").cloned().unwrap_or(Value::Null);
    let decorator = command.get("decorator").cloned().unwrap_or(Value::Null);
    Yaml::Map(vec![
        ("action".to_string(), Yaml::Scalar(action.to_string())),
        ("target".to_string(), handle_target(&target)),
        ("decorator".to_string(), handle_decorator(&decorator)),
    ])
}

/// `jsonToYaml` (`src/dcsconverter.ts`): renders a `DecoratorCommandSet`
/// object as the short DCS YAML format.
pub fn json_to_yaml(dcs_json: &Value) -> Result<String> {
    // `ModelUtil.getNamespace(dcsJson.$class)`: a missing (or any falsy)
    // `$class` is its "FQN is invalid." error.
    if dcs_json.is_null() {
        return Err(ContractError::new(
            ErrorKind::JsTypeError,
            "engine-typeerror-readproperties",
            vec![
                ("value", "null".to_string()),
                ("property", "$class".to_string()),
            ],
        )
        .into());
    }
    let class = dcs_json.get("$class").and_then(Value::as_str);
    let dcs_namespace = model_util::get_namespace(class.filter(|c| !c.is_empty()))?;
    // `ModelUtil.parseNamespace(dcsNamespace).version`, `undefined` for an
    // unversioned namespace.
    let version = match model_util::parse_namespace(Some(dcs_namespace), false)? {
        ParsedNamespace::Full { version, .. } => version,
        ParsedNamespace::NameOnly { .. } => None,
    };
    // `dcsJson.commands.map(handleCommands)`.
    let commands = match dcs_json.get("commands") {
        Some(Value::Array(commands)) => commands,
        None | Some(Value::Null) => {
            return Err(ContractError::new(
                ErrorKind::JsTypeError,
                "engine-typeerror-readproperties",
                vec![
                    (
                        "value",
                        if dcs_json.get("commands").is_none() {
                            "undefined"
                        } else {
                            "null"
                        }
                        .to_string(),
                    ),
                    ("property", "map".to_string()),
                ],
            )
            .into());
        }
        Some(_) => {
            return Err(ContractError::new(
                ErrorKind::JsTypeError,
                "engine-typeerror-notafunction",
                vec![("expression", "dcsJson.commands.map".to_string())],
            )
            .into());
        }
    };

    // `yaml.stringify` leaves out a key whose value is `undefined`.
    let mut entries = Vec::new();
    if let Some(version) = version {
        entries.push((
            "decoratorCommandsVersion".to_string(),
            Yaml::Scalar(version),
        ));
    }
    for key in ["name", "version"] {
        if let Some(v) = dcs_json.get(key) {
            entries.push((key.to_string(), Yaml::Scalar(js_value_to_string(v))));
        }
    }
    entries.push((
        "commands".to_string(),
        Yaml::Seq(commands.iter().map(handle_command).collect()),
    ));
    let root = Yaml::Map(entries);

    let mut out = String::new();
    emit_map(root_entries(&root), 0, &mut out);
    Ok(out)
}

fn root_entries(root: &Yaml) -> &[(String, Yaml)] {
    match root {
        Yaml::Map(entries) => entries,
        _ => unreachable!("json_to_yaml's root is always a Map"),
    }
}

/// A plain-or-double-quoted YAML scalar for the `failsafe` schema
/// (`yaml_quote::needs_quoting_failsafe` — see the module doc comment for
/// where this diverges from the reference's exact quoted-style choice).
fn render_scalar_failsafe(value: &str) -> String {
    if value.is_empty() {
        // `yaml.stringify('', {schema:'failsafe'})` renders the empty
        // string as a bare empty scalar (confirmed against the reference:
        // under `failsafe`, unlike `core`, there is no null tag for it to
        // collide with).
        return String::new();
    }
    if yaml_quote::needs_quoting_failsafe(value) {
        yaml_quote::json_quote_for_dcsconverter(value)
    } else {
        value.to_string()
    }
}

fn emit_map(entries: &[(String, Yaml)], indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for (key, value) in entries {
        match value {
            Yaml::Scalar(s) => {
                out.push_str(&pad);
                out.push_str(key);
                out.push_str(": ");
                out.push_str(&render_scalar_failsafe(s));
                out.push('\n');
            }
            Yaml::Seq(items) if items.is_empty() => {
                out.push_str(&pad);
                out.push_str(key);
                out.push_str(": []\n");
            }
            Yaml::Map(m) if m.is_empty() => {
                out.push_str(&pad);
                out.push_str(key);
                out.push_str(": {}\n");
            }
            Yaml::Seq(items) => {
                out.push_str(&pad);
                out.push_str(key);
                out.push_str(":\n");
                emit_seq(items, indent, out);
            }
            Yaml::Map(m) => {
                out.push_str(&pad);
                out.push_str(key);
                out.push_str(":\n");
                emit_map(m, indent + 2, out);
            }
        }
    }
}

fn emit_seq(items: &[Yaml], indent: usize, out: &mut String) {
    let item_indent = indent + 2;
    let item_pad = " ".repeat(item_indent);
    for item in items {
        match item {
            Yaml::Scalar(s) => {
                out.push_str(&item_pad);
                out.push_str("- ");
                out.push_str(&render_scalar_failsafe(s));
                out.push('\n');
            }
            Yaml::Map(m) if m.is_empty() => {
                out.push_str(&item_pad);
                out.push_str("- {}\n");
            }
            Yaml::Seq(s) if s.is_empty() => {
                out.push_str(&item_pad);
                out.push_str("- []\n");
            }
            Yaml::Map(m) => {
                let mut buf = String::new();
                emit_map(m, item_indent + 2, &mut buf);
                splice_dash(&item_pad, item_indent + 2, &buf, out);
            }
            Yaml::Seq(s) => {
                let mut buf = String::new();
                emit_seq(s, item_indent, &mut buf);
                splice_dash(&item_pad, item_indent + 2, &buf, out);
            }
        }
    }
}

/// Turns a block rendered at `content_indent` into a sequence item: the
/// first line's leading `content_indent` spaces become `item_pad` + `"- "`,
/// every other line is copied verbatim (`yaml.stringify`'s own block
/// sequence layout: item content is indented two past the dash, matching
/// [`emit_seq`]'s `item_indent + 2`).
fn splice_dash(item_pad: &str, content_indent: usize, block: &str, out: &mut String) {
    let mut lines = block.split_inclusive('\n');
    if let Some(first) = lines.next() {
        let stripped = first.get(content_indent.min(first.len())..).unwrap_or("");
        out.push_str(item_pad);
        out.push_str("- ");
        out.push_str(stripped);
    }
    for rest in lines {
        out.push_str(rest);
    }
}

// ---------------------------------------------------------------------
// yamlToJson
// ---------------------------------------------------------------------

struct Line<'a> {
    indent: usize,
    content: &'a str,
}

fn lines_of(input: &str) -> Vec<Line<'_>> {
    input
        .lines()
        .map(|raw| raw.strip_suffix('\r').unwrap_or(raw))
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let indent = l.len() - l.trim_start_matches(' ').len();
            Line {
                indent,
                content: l[indent..].trim_end(),
            }
        })
        .collect()
}

/// Unquotes a scalar as written by [`render_scalar_failsafe`]: a plain
/// scalar is used verbatim; a double-quoted one is unescaped with the JSON
/// grammar (`render_scalar_failsafe`'s quoted form is `JSON.stringify`, byte
/// for byte — see the module doc comment). A single-quoted scalar (never
/// emitted by this port, but legal YAML a hand-edited `.yaml` file might
/// use) unescapes `''` to `'` only, the one single-quote escape the
/// `failsafe` schema's plain/quoted scalars can carry without a flow
/// indicator.
fn unescape_scalar(raw: &str) -> Result<String> {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        serde_json::from_str::<String>(raw).map_err(|e| {
            pre_port(format!(
                "dcsconverter.yamlToJson: invalid quoted scalar {raw:?}: {e}"
            ))
            .into()
        })
    } else if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        Ok(raw[1..raw.len() - 1].replace("''", "'"))
    } else {
        Ok(raw.to_string())
    }
}

fn split_map_entry(content: &str) -> Option<(&str, &str)> {
    if let Some(rest) = content.strip_suffix(':') {
        return Some((rest, ""));
    }
    content.split_once(": ")
}

fn parse_value(lines: &[Line<'_>], pos: &mut usize, indent: usize) -> Result<Yaml> {
    let Some(line) = lines.get(*pos) else {
        return Ok(Yaml::Map(Vec::new()));
    };
    if line.indent != indent {
        return Err(pre_port(format!(
            "dcsconverter.yamlToJson: expected indent {indent}, found {} at {:?}",
            line.indent, line.content
        ))
        .into());
    }
    if line.content == "-" || line.content.starts_with("- ") {
        parse_seq(lines, pos, indent)
    } else {
        parse_map(lines, pos, indent)
    }
}

fn parse_seq(lines: &[Line<'_>], pos: &mut usize, indent: usize) -> Result<Yaml> {
    let mut items = Vec::new();
    while let Some(line) = lines.get(*pos) {
        if line.indent != indent || !(line.content == "-" || line.content.starts_with("- ")) {
            break;
        }
        let rest = line.content.strip_prefix('-').unwrap_or("").trim_start();
        if rest.is_empty() {
            *pos += 1;
            items.push(parse_value(lines, pos, indent + 2)?);
        } else if rest == "[]" {
            *pos += 1;
            items.push(Yaml::Seq(Vec::new()));
        } else if rest == "{}" {
            *pos += 1;
            items.push(Yaml::Map(Vec::new()));
        } else if let Some((key, value)) = split_map_entry(rest) {
            let map_indent = indent + 2;
            let mut entries = vec![parse_map_value(key, value, lines, pos, map_indent)?];
            while let Some(next) = lines.get(*pos) {
                if next.indent != map_indent {
                    break;
                }
                entries.push(parse_map_line(lines, pos, map_indent)?);
            }
            items.push(Yaml::Map(entries));
        } else {
            *pos += 1;
            items.push(Yaml::Scalar(unescape_scalar(rest)?));
        }
    }
    Ok(Yaml::Seq(items))
}

fn parse_map(lines: &[Line<'_>], pos: &mut usize, indent: usize) -> Result<Yaml> {
    let mut entries = Vec::new();
    while let Some(line) = lines.get(*pos) {
        if line.indent != indent {
            break;
        }
        entries.push(parse_map_line(lines, pos, indent)?);
    }
    Ok(Yaml::Map(entries))
}

fn parse_map_line(lines: &[Line<'_>], pos: &mut usize, indent: usize) -> Result<(String, Yaml)> {
    let content = lines[*pos].content;
    let (key, value) = split_map_entry(content).ok_or_else(|| {
        pre_port(format!(
            "dcsconverter.yamlToJson: not a mapping entry: {content:?}"
        ))
    })?;
    parse_map_value(key, value, lines, pos, indent)
}

/// Parses one `key: value` (or `key:` with a nested block) line already
/// split into `key`/`value`; `lines[*pos]` is that line itself (not yet
/// consumed) in both callers ([`parse_map_line`], and [`parse_seq`] for a
/// sequence item's spliced first line).
fn parse_map_value(
    key: &str,
    value: &str,
    lines: &[Line<'_>],
    pos: &mut usize,
    indent: usize,
) -> Result<(String, Yaml)> {
    let key = key.to_string();
    *pos += 1;
    if value.is_empty() {
        if let Some(next) = lines.get(*pos)
            && next.indent > indent
        {
            let nested_indent = next.indent;
            return Ok((key, parse_value(lines, pos, nested_indent)?));
        }
        return Ok((key, Yaml::Scalar(String::new())));
    }
    if value == "[]" {
        return Ok((key, Yaml::Seq(Vec::new())));
    }
    if value == "{}" {
        return Ok((key, Yaml::Map(Vec::new())));
    }
    Ok((key, Yaml::Scalar(unescape_scalar(value)?)))
}

fn yaml_get<'a>(entries: &'a [(String, Yaml)], key: &str) -> Option<&'a Yaml> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn yaml_scalar(entries: &[(String, Yaml)], key: &str) -> Result<String> {
    match yaml_get(entries, key) {
        Some(Yaml::Scalar(s)) => Ok(s.clone()),
        _ => Err(pre_port(format!(
            "dcsconverter.yamlToJson: missing or non-scalar \"{key}\""
        ))
        .into()),
    }
}

/// `restoreArguments` (`src/dcsconverter.ts`).
fn restore_argument(argument: &Yaml) -> Result<Value> {
    let Yaml::Map(entries) = argument else {
        return Err(
            pre_port("dcsconverter.yamlToJson: a decorator argument must be a mapping").into(),
        );
    };
    if let Some(Yaml::Map(tr)) = yaml_get(entries, "typeReference") {
        let mut type_obj = Map::new();
        type_obj.insert(
            "$class".to_string(),
            Value::String(model_util::get_fully_qualified_name(
                META_MODEL_NAMESPACE,
                "TypeIdentifier",
            )),
        );
        type_obj.insert("name".to_string(), Value::String(yaml_scalar(tr, "name")?));
        if let Some(Yaml::Scalar(ns)) = yaml_get(tr, "namespace") {
            type_obj.insert("namespace".to_string(), Value::String(ns.clone()));
        }
        if let Some(Yaml::Scalar(rn)) = yaml_get(tr, "resolvedName") {
            type_obj.insert("resolvedName".to_string(), Value::String(rn.clone()));
        }
        let is_array = yaml_scalar(tr, "isArray")? == "true";
        let mut out = Map::new();
        out.insert(
            "$class".to_string(),
            Value::String(model_util::get_fully_qualified_name(
                META_MODEL_NAMESPACE,
                "DecoratorTypeReference",
            )),
        );
        out.insert("type".to_string(), Value::Object(type_obj));
        out.insert("isArray".to_string(), Value::Bool(is_array));
        return Ok(Value::Object(out));
    }

    let type_name = yaml_scalar(entries, "type")?;
    let raw_value = yaml_scalar(entries, "value")?;
    let class = match type_name.as_str() {
        "String" => format!("{META_MODEL_NAMESPACE}.DecoratorString"),
        "Number" => format!("{META_MODEL_NAMESPACE}.DecoratorNumber"),
        "Boolean" => format!("{META_MODEL_NAMESPACE}.DecoratorBoolean"),
        other => {
            return Err(pre_port(format!(
                "dcsconverter.yamlToJson: unknown decorator argument type \"{other}\""
            ))
            .into());
        }
    };
    let value = match type_name.as_str() {
        "Number" => parse_js_number(&raw_value)?,
        "Boolean" => Value::Bool(raw_value == "true"),
        _ => Value::String(raw_value),
    };
    let mut out = Map::new();
    out.insert("$class".to_string(), Value::String(class));
    out.insert("value".to_string(), value);
    Ok(Value::Object(out))
}

/// `Number(value)` (`restoreArguments`, `src/dcsconverter.ts`), for the
/// decimal literal text a DCS decorator's `Number` argument round-trips as
/// (see [`js_value_to_string`]); not a general `Number()` port.
fn parse_js_number(raw: &str) -> Result<Value> {
    if let Ok(i) = raw.parse::<i64>() {
        return Ok(Value::Number(Number::from(i)));
    }
    let f: f64 = raw
        .parse()
        .map_err(|_| pre_port(format!("dcsconverter.yamlToJson: not a number: {raw:?}")))?;
    Number::from_f64(f).map(Value::Number).ok_or_else(|| {
        pre_port(format!(
            "dcsconverter.yamlToJson: not a finite number: {raw:?}"
        ))
        .into()
    })
}

/// `restoreDecorator` (`src/dcsconverter.ts`).
fn restore_decorator(decorator: &Yaml) -> Result<Value> {
    let Yaml::Map(entries) = decorator else {
        return Err(pre_port("dcsconverter.yamlToJson: a decorator must be a mapping").into());
    };
    let name = yaml_scalar(entries, "name")?;
    let arguments = match yaml_get(entries, "arguments") {
        Some(Yaml::Seq(items)) => items
            .iter()
            .map(restore_argument)
            .collect::<Result<Vec<_>>>()?,
        _ => Vec::new(),
    };
    let mut out = Map::new();
    out.insert(
        "$class".to_string(),
        Value::String(model_util::get_fully_qualified_name(
            META_MODEL_NAMESPACE,
            "Decorator",
        )),
    );
    out.insert("name".to_string(), Value::String(name));
    out.insert("arguments".to_string(), Value::Array(arguments));
    Ok(Value::Object(out))
}

/// `restoreCommands` (`src/dcsconverter.ts`).
fn restore_command(dcs_namespace: &str, command: &Yaml) -> Result<Value> {
    let Yaml::Map(entries) = command else {
        return Err(pre_port("dcsconverter.yamlToJson: a command must be a mapping").into());
    };
    let action = yaml_scalar(entries, "action")?;
    let mut target_obj = Map::new();
    target_obj.insert(
        "$class".to_string(),
        Value::String(model_util::get_fully_qualified_name(
            dcs_namespace,
            "CommandTarget",
        )),
    );
    if let Some(Yaml::Map(target_entries)) = yaml_get(entries, "target") {
        for (key, value) in target_entries {
            target_obj.insert(key.clone(), yaml_to_value(value)?);
        }
    }
    let decorator =
        restore_decorator(yaml_get(entries, "decorator").ok_or_else(|| {
            pre_port("dcsconverter.yamlToJson: a command needs a \"decorator\"")
        })?)?;
    let mut out = Map::new();
    out.insert(
        "$class".to_string(),
        Value::String(model_util::get_fully_qualified_name(
            dcs_namespace,
            "Command",
        )),
    );
    out.insert("type".to_string(), Value::String(action));
    out.insert("target".to_string(), Value::Object(target_obj));
    out.insert("decorator".to_string(), decorator);
    Ok(Value::Object(out))
}

fn yaml_to_value(value: &Yaml) -> Result<Value> {
    match value {
        Yaml::Scalar(s) => Ok(Value::String(s.clone())),
        Yaml::Seq(items) => Ok(Value::Array(
            items
                .iter()
                .map(yaml_to_value)
                .collect::<Result<Vec<_>>>()?,
        )),
        Yaml::Map(entries) => {
            let mut out = Map::new();
            for (k, v) in entries {
                out.insert(k.clone(), yaml_to_value(v)?);
            }
            Ok(Value::Object(out))
        }
    }
}

/// `yamlToJson` (`src/dcsconverter.ts`): parses the short DCS YAML format
/// back into a `DecoratorCommandSet` object.
pub fn yaml_to_json(yaml_string: &str) -> Result<Value> {
    let lines = lines_of(yaml_string);
    let mut pos = 0usize;
    let Yaml::Map(entries) = parse_value(&lines, &mut pos, 0)? else {
        return Err(
            pre_port("dcsconverter.yamlToJson: expected a mapping at the document root").into(),
        );
    };
    // `'…@' + parsedJson.decoratorCommandsVersion`: `undefined` when absent.
    let dcs_version = match yaml_get(&entries, "decoratorCommandsVersion") {
        None => "undefined".to_string(),
        Some(_) => yaml_scalar(&entries, "decoratorCommandsVersion")?,
    };
    let dcs_namespace = format!("org.accordproject.decoratorcommands@{dcs_version}");
    // `parsedJson.commands.map(...)`.
    let commands = match yaml_get(&entries, "commands") {
        Some(Yaml::Seq(items)) => items
            .iter()
            .map(|c| restore_command(&dcs_namespace, c))
            .collect::<Result<Vec<_>>>()?,
        None => {
            return Err(ContractError::new(
                ErrorKind::JsTypeError,
                "engine-typeerror-readproperties",
                vec![
                    ("value", "undefined".to_string()),
                    ("property", "map".to_string()),
                ],
            )
            .into());
        }
        _ => return Err(pre_port("dcsconverter.yamlToJson: missing \"commands\" sequence").into()),
    };

    let mut out = Map::new();
    out.insert(
        "$class".to_string(),
        Value::String(model_util::get_fully_qualified_name(
            &dcs_namespace,
            "DecoratorCommandSet",
        )),
    );
    // `name: parsedJson.name`, `version: parsedJson.version`: a key left
    // `undefined` in TS is left out here.
    for key in ["name", "version"] {
        if yaml_get(&entries, key).is_some() {
            out.insert(key.to_string(), Value::String(yaml_scalar(&entries, key)?));
        }
    }
    out.insert("commands".to_string(), Value::Array(commands));
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    //! Ports `test/dcsconverter.js` (`describe('DCS Converter')`, 8 `it`s):
    //! the golden `#jsonToYaml`/`#yamlToJson` round trip against
    //! `test/data/decoratorcommands/possible-decorator-command-targets.{json,yaml}`
    //! (vendored verbatim under `testdata/`, as `rootmodel.rs` vendors
    //! `rootmodel.json`), plus its three inline `DecoratorTypeReference`
    //! fixtures (resolved-but-unaliased, resolved-and-aliased, unresolved).
    //! Every expected string here was checked byte for byte against the
    //! reference (`yaml@2.8.3`).
    use super::*;

    const POSSIBLE_TARGETS_JSON: &str =
        include_str!("testdata/possible-decorator-command-targets.json");
    const POSSIBLE_TARGETS_YAML: &str =
        include_str!("testdata/possible-decorator-command-targets.yaml");

    #[test]
    fn json_to_yaml_matches_the_golden_fixture_byte_for_byte() {
        let dcs_json: Value = serde_json::from_str(POSSIBLE_TARGETS_JSON).unwrap();
        let out = json_to_yaml(&dcs_json).unwrap();
        assert_eq!(out, POSSIBLE_TARGETS_YAML);
    }

    #[test]
    fn yaml_to_json_matches_the_golden_fixture() {
        let dcs_json: Value = serde_json::from_str(POSSIBLE_TARGETS_JSON).unwrap();
        let out = yaml_to_json(POSSIBLE_TARGETS_YAML).unwrap();
        assert_eq!(out, dcs_json);
    }

    fn type_reference_dcs_json(namespace: Option<&str>, resolved_name: Option<&str>) -> Value {
        let mut type_obj = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.TypeIdentifier",
            "name": "Info",
        });
        if let Some(ns) = namespace {
            type_obj["namespace"] = Value::String(ns.to_string());
        }
        if let Some(rn) = resolved_name {
            type_obj["resolvedName"] = Value::String(rn.to_string());
        }
        serde_json::json!({
            "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
            "name": "exampleDCS",
            "version": "1.0.0",
            "commands": [{
                "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
                "type": "UPSERT",
                "target": {
                    "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget",
                    "namespace": "test@1.0.0"
                },
                "decorator": {
                    "$class": "concerto.metamodel@1.0.0.Decorator",
                    "name": "exampleDecorator",
                    "arguments": [{
                        "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
                        "type": type_obj,
                        "isArray": false
                    }]
                }
            }]
        })
    }

    #[test]
    fn json_to_yaml_handles_a_resolved_but_unaliased_type_reference() {
        let dcs_json = type_reference_dcs_json(Some("test@1.0.0"), None);
        let expected = "decoratorCommandsVersion: 0.4.0\n\
             name: exampleDCS\n\
             version: 1.0.0\n\
             commands:\n\
             \x20 - action: UPSERT\n\
             \x20   target:\n\
             \x20     namespace: test@1.0.0\n\
             \x20   decorator:\n\
             \x20     name: exampleDecorator\n\
             \x20     arguments:\n\
             \x20       - typeReference:\n\
             \x20           name: Info\n\
             \x20           namespace: test@1.0.0\n\
             \x20           isArray: false\n";
        assert_eq!(json_to_yaml(&dcs_json).unwrap(), expected);
    }

    #[test]
    fn json_to_yaml_handles_a_resolved_and_aliased_type_reference() {
        let dcs_json = type_reference_dcs_json(Some("test@1.0.0"), Some("Data"));
        let expected = "decoratorCommandsVersion: 0.4.0\n\
             name: exampleDCS\n\
             version: 1.0.0\n\
             commands:\n\
             \x20 - action: UPSERT\n\
             \x20   target:\n\
             \x20     namespace: test@1.0.0\n\
             \x20   decorator:\n\
             \x20     name: exampleDecorator\n\
             \x20     arguments:\n\
             \x20       - typeReference:\n\
             \x20           name: Info\n\
             \x20           namespace: test@1.0.0\n\
             \x20           resolvedName: Data\n\
             \x20           isArray: false\n";
        assert_eq!(json_to_yaml(&dcs_json).unwrap(), expected);
    }

    #[test]
    fn json_to_yaml_handles_an_unresolved_and_unaliased_type_reference() {
        let dcs_json = type_reference_dcs_json(None, None);
        let expected = "decoratorCommandsVersion: 0.4.0\n\
             name: exampleDCS\n\
             version: 1.0.0\n\
             commands:\n\
             \x20 - action: UPSERT\n\
             \x20   target:\n\
             \x20     namespace: test@1.0.0\n\
             \x20   decorator:\n\
             \x20     name: exampleDecorator\n\
             \x20     arguments:\n\
             \x20       - typeReference:\n\
             \x20           name: Info\n\
             \x20           isArray: false\n";
        assert_eq!(json_to_yaml(&dcs_json).unwrap(), expected);
    }

    #[test]
    fn yaml_to_json_handles_a_resolved_but_unaliased_type_reference() {
        let input = "decoratorCommandsVersion: 0.4.0\n\
             name: exampleDCS\n\
             version: 1.0.0\n\
             commands:\n\
             \x20 - action: UPSERT\n\
             \x20   target:\n\
             \x20     namespace: test@1.0.0\n\
             \x20   decorator:\n\
             \x20     name: exampleDecorator\n\
             \x20     arguments:\n\
             \x20       - typeReference:\n\
             \x20           name: Info\n\
             \x20           namespace: test@1.0.0\n\
             \x20           isArray: false\n";
        let expected = type_reference_dcs_json(Some("test@1.0.0"), None);
        assert_eq!(yaml_to_json(input).unwrap(), expected);
    }

    #[test]
    fn yaml_to_json_handles_a_resolved_and_aliased_type_reference() {
        let input = "decoratorCommandsVersion: 0.4.0\n\
             name: exampleDCS\n\
             version: 1.0.0\n\
             commands:\n\
             \x20 - action: UPSERT\n\
             \x20   target:\n\
             \x20     namespace: test@1.0.0\n\
             \x20   decorator:\n\
             \x20     name: exampleDecorator\n\
             \x20     arguments:\n\
             \x20       - typeReference:\n\
             \x20           name: Info\n\
             \x20           namespace: test@1.0.0\n\
             \x20           resolvedName: Data\n\
             \x20           isArray: false\n";
        let expected = type_reference_dcs_json(Some("test@1.0.0"), Some("Data"));
        assert_eq!(yaml_to_json(input).unwrap(), expected);
    }

    #[test]
    fn yaml_to_json_handles_an_unresolved_and_unaliased_type_reference() {
        let input = "decoratorCommandsVersion: 0.4.0\n\
             name: exampleDCS\n\
             version: 1.0.0\n\
             commands:\n\
             \x20 - action: UPSERT\n\
             \x20   target:\n\
             \x20     namespace: test@1.0.0\n\
             \x20   decorator:\n\
             \x20     name: exampleDecorator\n\
             \x20     arguments:\n\
             \x20       - typeReference:\n\
             \x20           name: Info\n\
             \x20           isArray: false\n";
        let expected = type_reference_dcs_json(None, None);
        assert_eq!(yaml_to_json(input).unwrap(), expected);
    }

    #[test]
    fn round_trips_a_command_with_string_number_and_boolean_arguments() {
        let dcs_json = serde_json::json!({
            "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
            "name": "argsDCS",
            "version": "1.0.0",
            "commands": [{
                "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
                "type": "APPEND",
                "target": { "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget", "namespace": "test@1.0.0" },
                "decorator": {
                    "$class": "concerto.metamodel@1.0.0.Decorator",
                    "name": "argumentsTest",
                    "arguments": [
                        { "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "inputString" },
                        { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 2.5 },
                        { "$class": "concerto.metamodel@1.0.0.DecoratorBoolean", "value": true }
                    ]
                }
            }]
        });
        let yaml = json_to_yaml(&dcs_json).unwrap();
        assert_eq!(
            yaml,
            "decoratorCommandsVersion: 0.4.0\n\
             name: argsDCS\n\
             version: 1.0.0\n\
             commands:\n\
             \x20 - action: APPEND\n\
             \x20   target:\n\
             \x20     namespace: test@1.0.0\n\
             \x20   decorator:\n\
             \x20     name: argumentsTest\n\
             \x20     arguments:\n\
             \x20       - type: String\n\
             \x20         value: inputString\n\
             \x20       - type: Number\n\
             \x20         value: 2.5\n\
             \x20       - type: Boolean\n\
             \x20         value: true\n"
        );
        // Round trip: yamlToJson(jsonToYaml(x)) restores x, including
        // `Number`/`Boolean` argument values as their JSON types (not the
        // stringified form the YAML carries them as in between).
        assert_eq!(yaml_to_json(&yaml).unwrap(), dcs_json);
    }

    #[test]
    fn a_decorator_with_no_arguments_omits_the_arguments_key() {
        let decorator = serde_json::json!({ "name": "NoArgs", "arguments": [] });
        assert_eq!(
            handle_decorator(&decorator),
            Yaml::Map(vec![(
                "name".to_string(),
                Yaml::Scalar("NoArgs".to_string())
            )])
        );
    }

    #[test]
    fn json_to_yaml_rejects_a_command_set_with_no_commands_array() {
        let dcs_json = serde_json::json!({
            "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
            "name": "x",
            "version": "1.0.0"
        });
        assert!(json_to_yaml(&dcs_json).is_err());
    }

    #[test]
    fn yaml_to_json_rejects_malformed_yaml() {
        assert!(yaml_to_json("not: a: dcs\n  - broken").is_err());
    }

    /// `yamlToJson` itself does not check the command set: with no
    /// `decoratorCommandsVersion`, TS builds the `$class` from `undefined`
    /// and leaves the checking to `DecoratorManager.yamlToJson`.
    #[test]
    fn yaml_to_json_builds_the_class_from_an_absent_version_as_undefined() {
        let out = yaml_to_json("name: x\nversion: 1.0.0\ncommands: []").unwrap();
        assert_eq!(
            out["$class"],
            "org.accordproject.decoratorcommands@undefined.DecoratorCommandSet"
        );
        assert!(
            crate::dcs::validated_yaml_to_json("name: x\nversion: 1.0.0\ncommands: []").is_err()
        );
    }

    /// `test/decoratormanager.js` "#jsonToYaml should throw error if input
    /// is not valid DCS JSON", through `DecoratorManager.jsonToYaml`
    /// ([`crate::dcs::validated_json_to_yaml`]).
    #[test]
    fn json_to_yaml_rejects_every_reference_invalid_input() {
        for invalid in [
            serde_json::json!({ "invalid": "dcsJson" }),
            serde_json::json!({ "version": "1.0.0", "commands": [] }),
            serde_json::json!({ "name": "test", "commands": [] }),
            serde_json::json!({ "name": "test", "version": "1.0.0" }),
        ] {
            assert!(
                crate::dcs::validated_json_to_yaml(&invalid).is_err(),
                "expected an error for {invalid}"
            );
        }
    }

    /// `test/decoratormanager.js` "#yamlToJson should throw error if input
    /// is not valid DCS YAML", through `DecoratorManager.yamlToJson`
    /// ([`crate::dcs::validated_yaml_to_json`]).
    #[test]
    fn yaml_to_json_rejects_every_reference_invalid_input() {
        for invalid in [
            "decoratorCommandsVersion: 0.4.0\ncommands: []\n",
            "decoratorCommandsVersion: 0.4.0\nname: test\ncommands: []\n",
            "name: test\nversion: 1.0.0\ncommands: []\n",
        ] {
            assert!(
                crate::dcs::validated_yaml_to_json(invalid).is_err(),
                "expected an error for {invalid:?}"
            );
        }
    }
}
