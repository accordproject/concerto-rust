//! BC-19's AST shape check, folded into the strict typed read (P5-69,
//! BC-19-b, accordproject/concerto-rust#408).
//!
//! [`conforms`] answers, from what the typed read ([`super::typed_ast`]) has
//! already decoded, whether a model AST certainly has the metamodel's
//! shape: whether `instance::check_ast_shape` would accept it. It never
//! answers yes for an AST that check rejects; it may answer no for one the
//! check accepts (anything unusual, such as a node with no `$class`, a
//! root that is not a `Model`, or a name outside the ASCII identifier
//! subset). The caller then runs that check itself, over a `Value` of the
//! AST, and takes its verdict: so every AST is accepted or rejected exactly
//! as before, with the same error, while a well-formed AST (every AST the
//! reference parser writes, and every model of the oracle corpus and the
//! benchmark sets) is checked without the metamodel instance validator.
//!
//! What the typed read decodes into a generated struct is already checked
//! by that decode: an unknown key, a field of the wrong JSON type, a missing
//! required field, a `null` required field. [`conforms`] adds what the read
//! does not express, which is where each rule of `check_ast_shape` now
//! lives (the table test below maps every rule to its home):
//!
//! - every node the read keeps as a `Value` (the model's own keys, every
//!   decorator list, every `location`, a class's `identified`, every scalar
//!   and map declaration) is checked against the metamodel's declared
//!   fields here ([`node_conforms`]): its `$class` names a concrete type the
//!   field allows, it has no undeclared key, each declared field has the
//!   declared JSON type, and a required one is present and not `null`;
//! - the `$class` of every struct the read decoded without looking at it (a
//!   `TypeIdentifier`, a validator, an `EnumProperty`) is that type's;
//! - an `Integer` or `Long` field holds an integral number (the generated
//!   structs read every number as `f64`);
//! - a declaration's or property's name passes the metamodel's identifier
//!   validator (here: the ASCII subset `is_valid_identifier`'s fast path
//!   accepts; any other name is left to the full check);
//! - BC-20's empty super type name, BC-19's one tolerance (the string
//!   `defaultValue` the reference parser writes on a `DateTimeProperty`),
//!   and an `EnumProperty` only as an enum's value;
//! - the version check: the model's `$class` is the metamodel's `Model`.
//!
//! BC-17 (a non-array `decorators`), BC-20 (a non-string name) and the
//! node rule for `identified` and the validators (a node or `null`) are each
//! a field of the wrong JSON type, rejected by the read's decode or here.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::{Map, Value};

use crate::introspect::property::Property;
use crate::introspect::typed_ast::{TypedDeclaration, TypedModel, TypedProperty};

/// The metamodel namespace, as a `$class` prefix.
const MM: &str = "concerto.metamodel@1.0.0.";

/// Whether the typed read of a model AST certainly passes BC-19's shape
/// check (the module doc). `false` means only "not certain".
pub(crate) fn conforms(model: &TypedModel) -> bool {
    header_conforms(&model.header) && model.declarations.iter().all(declaration_conforms)
}

/// The model's own keys (every top-level key but `declarations`).
fn header_conforms(header: &Value) -> bool {
    let Value::Object(map) = header else {
        return false;
    };
    class_is(map.get("$class"), "Model") && object_conforms(map, MODEL)
}

fn declaration_conforms(declaration: &TypedDeclaration) -> bool {
    match declaration {
        TypedDeclaration::Ast(value) => node_conforms(value, OTHER_DECLARATIONS),
        TypedDeclaration::Class {
            node,
            properties,
            decorators,
            location,
            identified,
            ..
        } => {
            let (name, super_type) = node.name_and_super_type();
            is_name(name)
                && super_type.is_none_or(|t| type_identifier(t) && !t.name.is_empty())
                && optional_node(identified.as_ref(), IDENTIFIED)
                && optional_nodes(decorators.as_ref(), DECORATOR)
                && optional_node(location.as_ref(), RANGE)
                && properties.iter().all(|p| property_conforms(p, false))
        }
        TypedDeclaration::Enum {
            node,
            values,
            decorators,
            location,
        } => {
            is_name(&node.name)
                && optional_nodes(decorators.as_ref(), DECORATOR)
                && optional_node(location.as_ref(), RANGE)
                && values.iter().all(|p| property_conforms(p, true))
        }
    }
}

/// One property of a class-like declaration, or one value of an enum
/// declaration (`in_enum`).
fn property_conforms(property: &TypedProperty, in_enum: bool) -> bool {
    if !(optional_nodes(property.decorators.as_ref(), DECORATOR)
        && optional_node(property.location.as_ref(), RANGE))
    {
        return false;
    }
    // Only a `DateTimeProperty` keeps a `defaultValue` apart, and only a
    // string one is BC-19's tolerance.
    if property
        .date_time_default
        .as_ref()
        .is_some_and(|value| !value.is_string())
    {
        return false;
    }
    match &property.property {
        Property::Enum(p) => in_enum && is(&p._class, "EnumProperty") && is_name(&p.name),
        _ if in_enum => false,
        Property::Boolean(p) => is_name(&p.name) && size(p.size_validator.as_ref()),
        Property::DateTime(p) => is_name(&p.name) && size(p.size_validator.as_ref()),
        Property::String(p) => {
            is_name(&p.name)
                && size(p.size_validator.as_ref())
                && p.validator
                    .as_ref()
                    .is_none_or(|v| is(&v._class, "StringRegexValidator"))
                && p.length_validator.as_ref().is_none_or(|v| {
                    is(&v._class, "StringLengthValidator")
                        && integers(&[v.min_length, v.max_length])
                })
        }
        Property::Integer(p) => {
            is_name(&p.name)
                && size(p.size_validator.as_ref())
                && integers(&[p.default_value])
                && p.validator.as_ref().is_none_or(|v| {
                    is(&v._class, "IntegerDomainValidator") && integers(&[v.lower, v.upper])
                })
        }
        Property::Long(p) => {
            is_name(&p.name)
                && size(p.size_validator.as_ref())
                && integers(&[p.default_value])
                && p.validator.as_ref().is_none_or(|v| {
                    is(&v._class, "LongDomainValidator") && integers(&[v.lower, v.upper])
                })
        }
        Property::Double(p) => {
            is_name(&p.name)
                && size(p.size_validator.as_ref())
                && p.validator
                    .as_ref()
                    .is_none_or(|v| is(&v._class, "DoubleDomainValidator"))
        }
        Property::Object(p) => {
            is_name(&p.name) && size(p.size_validator.as_ref()) && type_identifier(&p.type_)
        }
        Property::Relationship(p) => {
            is_name(&p.name) && size(p.size_validator.as_ref()) && type_identifier(&p.type_)
        }
    }
}

/// A decoded `TypeIdentifier`'s own `$class`.
fn type_identifier(t: &mm::TypeIdentifier) -> bool {
    is(&t._class, "TypeIdentifier")
}

/// A decoded `sizeValidator`: its `$class`, and integral bounds.
fn size(validator: Option<&mm::CollectionSizeValidator>) -> bool {
    validator.is_none_or(|v| {
        is(&v._class, "CollectionSizeValidator") && integers(&[v.min_size, v.max_size])
    })
}

/// Every given `Integer` or `Long` value is integral.
fn integers(values: &[Option<f64>]) -> bool {
    values.iter().flatten().all(|n| is_integral(*n))
}

/// The metamodel's `Integer`/`Long` rule (`from_json`, BC-10): an integral,
/// finite number.
fn is_integral(n: f64) -> bool {
    n.is_finite() && n.trunc() == n
}

/// A name the metamodel's identifier validator certainly accepts: a
/// non-empty ASCII `[A-Za-z$_][A-Za-z0-9$_]*` (every such name matches its
/// regex, as `model_util::is_valid_identifier`'s fast path says). Any other
/// name is left to the full check.
fn is_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.split_first().is_some_and(|(first, rest)| {
        (first.is_ascii_alphabetic() || *first == b'$' || *first == b'_')
            && rest
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'$' || *b == b'_')
    })
}

/// A decoded struct's `$class` is the metamodel type `short`.
fn is(class: &str, short: &str) -> bool {
    class.strip_prefix(MM) == Some(short)
}

/// `$class` is the metamodel type `short`.
fn class_is(class: Option<&Value>, short: &str) -> bool {
    class
        .and_then(Value::as_str)
        .and_then(|class| class.strip_prefix(MM))
        == Some(short)
}

// ---------------------------------------------------------------------------
// The metamodel's declared fields, for the nodes the read keeps as a `Value`
// ---------------------------------------------------------------------------

/// A field's declared type, as the metamodel check reads it.
#[derive(Clone, Copy)]
enum Ty {
    /// A `String`.
    Str,
    /// A `String` with the metamodel's identifier validator ([`is_name`]).
    Name,
    /// A `Boolean`.
    Bool,
    /// An `Integer` or a `Long`.
    Integer,
    /// A `Double`.
    Double,
    /// A `String[]`.
    Strs,
    /// A node of one of these concrete types.
    Node(&'static [&'static str]),
    /// An array of nodes of these concrete types.
    Nodes(&'static [&'static str]),
}

/// Whether a field may be missing or `null`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    /// Neither missing nor `null`.
    Required,
    /// A required field with a default: may be missing, but not `null`.
    Defaulted,
    /// `optional`: may be missing or `null`.
    Optional,
}

type Fields = &'static [(&'static str, Ty, Need)];

const RANGE: &[&str] = &["Range"];
const POSITION: &[&str] = &["Position"];
const TYPE_IDENTIFIER: &[&str] = &["TypeIdentifier"];
const DECORATOR: &[&str] = &["Decorator"];
const IDENTIFIED: &[&str] = &["Identified", "IdentifiedBy"];
const DECORATOR_LITERAL: &[&str] = &[
    "DecoratorString",
    "DecoratorNumber",
    "DecoratorBoolean",
    "DecoratorTypeReference",
];
const IMPORT: &[&str] = &["ImportAll", "ImportType", "ImportTypes"];
const ALIASED_TYPE: &[&str] = &["AliasedType"];
/// The declarations the typed read keeps as a `Value`.
const OTHER_DECLARATIONS: &[&str] = &[
    "MapDeclaration",
    "BooleanScalar",
    "IntegerScalar",
    "LongScalar",
    "DoubleScalar",
    "StringScalar",
    "DateTimeScalar",
];
const MAP_KEY_TYPE: &[&str] = &["StringMapKeyType", "DateTimeMapKeyType", "ObjectMapKeyType"];
const MAP_VALUE_TYPE: &[&str] = &[
    "BooleanMapValueType",
    "DateTimeMapValueType",
    "StringMapValueType",
    "IntegerMapValueType",
    "LongMapValueType",
    "DoubleMapValueType",
    "ObjectMapValueType",
    "RelationshipMapValueType",
];

use Need::{Defaulted, Optional, Required};
use Ty::{Bool, Double, Integer, Name, Node, Nodes, Str, Strs};

/// `Model`, but for its `declarations`, which the typed read reads.
const MODEL: Fields = &[
    ("namespace", Str, Required),
    ("sourceUri", Str, Optional),
    ("concertoVersion", Str, Optional),
    ("imports", Nodes(IMPORT), Optional),
    ("decorators", Nodes(DECORATOR), Optional),
];

/// The fields of the concrete metamodel type `short`, inherited ones
/// included, for every type a `Value` node may have; `None` for any other.
fn fields(short: &str) -> Option<Fields> {
    const LOCATION: (&str, Ty, Need) = ("location", Node(RANGE), Optional);
    const DECORATORS: (&str, Ty, Need) = ("decorators", Nodes(DECORATOR), Optional);
    const NAME: (&str, Ty, Need) = ("name", Name, Required);
    const NAMESPACE: (&str, Ty, Need) = ("namespace", Str, Optional);
    const TYPE: (&str, Ty, Need) = ("type", Node(TYPE_IDENTIFIER), Required);
    const LOWER_UPPER_INT: Fields = &[("lower", Integer, Optional), ("upper", Integer, Optional)];
    Some(match short {
        "Position" => &[
            ("line", Integer, Required),
            ("column", Integer, Required),
            ("offset", Integer, Required),
        ],
        "Range" => &[
            ("start", Node(POSITION), Required),
            ("end", Node(POSITION), Required),
            ("source", Str, Optional),
        ],
        "TypeIdentifier" => &[
            ("name", Str, Required),
            ("namespace", Str, Optional),
            ("resolvedName", Str, Optional),
        ],
        "DecoratorString" => &[LOCATION, ("value", Str, Required)],
        "DecoratorNumber" => &[LOCATION, ("value", Double, Required)],
        "DecoratorBoolean" => &[LOCATION, ("value", Bool, Required)],
        "DecoratorTypeReference" => &[LOCATION, TYPE, ("isArray", Bool, Defaulted)],
        "Decorator" => &[
            ("name", Str, Required),
            ("arguments", Nodes(DECORATOR_LITERAL), Optional),
            LOCATION,
        ],
        "Identified" => &[],
        "IdentifiedBy" => &[("name", Str, Required)],
        "MapDeclaration" => &[
            NAME,
            DECORATORS,
            LOCATION,
            ("key", Node(MAP_KEY_TYPE), Required),
            ("value", Node(MAP_VALUE_TYPE), Required),
        ],
        "StringMapKeyType"
        | "DateTimeMapKeyType"
        | "BooleanMapValueType"
        | "DateTimeMapValueType"
        | "StringMapValueType"
        | "IntegerMapValueType"
        | "LongMapValueType"
        | "DoubleMapValueType" => &[DECORATORS, LOCATION],
        "ObjectMapKeyType" | "ObjectMapValueType" | "RelationshipMapValueType" => {
            &[DECORATORS, LOCATION, TYPE]
        }
        "BooleanScalar" => &[
            NAME,
            DECORATORS,
            LOCATION,
            NAMESPACE,
            ("defaultValue", Bool, Optional),
        ],
        "IntegerScalar" => &[
            NAME,
            DECORATORS,
            LOCATION,
            NAMESPACE,
            ("defaultValue", Integer, Optional),
            ("validator", Node(&["IntegerDomainValidator"]), Optional),
        ],
        "LongScalar" => &[
            NAME,
            DECORATORS,
            LOCATION,
            NAMESPACE,
            ("defaultValue", Integer, Optional),
            ("validator", Node(&["LongDomainValidator"]), Optional),
        ],
        "DoubleScalar" => &[
            NAME,
            DECORATORS,
            LOCATION,
            NAMESPACE,
            ("defaultValue", Double, Optional),
            ("validator", Node(&["DoubleDomainValidator"]), Optional),
        ],
        "StringScalar" => &[
            NAME,
            DECORATORS,
            LOCATION,
            NAMESPACE,
            ("defaultValue", Str, Optional),
            ("validator", Node(&["StringRegexValidator"]), Optional),
            (
                "lengthValidator",
                Node(&["StringLengthValidator"]),
                Optional,
            ),
        ],
        "DateTimeScalar" => &[
            NAME,
            DECORATORS,
            LOCATION,
            NAMESPACE,
            ("defaultValue", Str, Optional),
        ],
        "IntegerDomainValidator" | "LongDomainValidator" => LOWER_UPPER_INT,
        "DoubleDomainValidator" => &[("lower", Double, Optional), ("upper", Double, Optional)],
        "StringRegexValidator" => &[("pattern", Str, Required), ("flags", Str, Required)],
        "StringLengthValidator" => &[
            ("minLength", Integer, Optional),
            ("maxLength", Integer, Optional),
        ],
        "AliasedType" => &[("name", Str, Required), ("aliasedName", Str, Required)],
        "ImportAll" => &[("namespace", Str, Required), ("uri", Str, Optional)],
        "ImportType" => &[
            ("namespace", Str, Required),
            ("uri", Str, Optional),
            ("name", Str, Required),
        ],
        "ImportTypes" => &[
            ("namespace", Str, Required),
            ("uri", Str, Optional),
            ("types", Strs, Required),
            ("aliasedTypes", Nodes(ALIASED_TYPE), Optional),
        ],
        _ => return None,
    })
}

/// A node of one of the concrete types `allowed`, every field as declared.
fn node_conforms(value: &Value, allowed: &[&str]) -> bool {
    let Value::Object(map) = value else {
        return false;
    };
    let Some(short) = map
        .get("$class")
        .and_then(Value::as_str)
        .and_then(|class| class.strip_prefix(MM))
    else {
        return false;
    };
    allowed.contains(&short) && fields(short).is_some_and(|fields| object_conforms(map, fields))
}

/// `map`'s keys are `$class` and `fields`, each holding a value of its type.
fn object_conforms(map: &Map<String, Value>, fields: Fields) -> bool {
    map.keys()
        .all(|key| key == "$class" || fields.iter().any(|(name, ..)| name == key))
        && fields.iter().all(|(key, ty, need)| match map.get(*key) {
            None => *need != Required,
            Some(Value::Null) => *need == Optional,
            Some(value) => value_conforms(value, *ty),
        })
}

fn value_conforms(value: &Value, ty: Ty) -> bool {
    match ty {
        Str => value.is_string(),
        Name => value.as_str().is_some_and(is_name),
        Bool => value.is_boolean(),
        Integer => value.as_f64().is_some_and(is_integral),
        Double => value.is_number(),
        Strs => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
        Node(allowed) => node_conforms(value, allowed),
        Nodes(allowed) => value
            .as_array()
            .is_some_and(|items| items.iter().all(|item| node_conforms(item, allowed))),
    }
}

/// An optional node field the read kept as given: missing, `null`, or a
/// node of one of the types `allowed`.
fn optional_node(value: Option<&Value>, allowed: &[&str]) -> bool {
    value.is_none_or(|value| value.is_null() || node_conforms(value, allowed))
}

/// An optional array-of-nodes field the read kept as given.
fn optional_nodes(value: Option<&Value>, allowed: &'static [&'static str]) -> bool {
    value.is_none_or(|value| value.is_null() || value_conforms(value, Nodes(allowed)))
}

/// [`conforms`], for an AST given as a `Value`: the typed read of `ast`
/// ([`super::typed_ast::from_value`]) succeeds and conforms.
pub(crate) fn ast_conforms(ast: &Value) -> bool {
    super::typed_ast::from_value(ast).is_ok_and(|model| conforms(&model))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::ast_conforms;
    use crate::error::Error;
    use crate::instance::metamodel::{check_ast_shape, check_ast_shape_exact};
    use crate::introspect::model_file::ModelFile;

    fn model(declarations: Value) -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": declarations,
        })
    }

    fn concept(properties: Value) -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Thing",
            "isAbstract": false,
            "properties": properties,
        })
    }

    fn string_property() -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "a",
            "isArray": false,
            "isOptional": false,
        })
    }

    /// An error's kind, code and message, comparable across two loads.
    fn describe(err: &Error) -> String {
        format!("{:?} {} {err}", err.kind(), err.code())
    }

    /// What a load gives: the loaded file's namespace, or the error.
    fn outcome(
        result: Result<Result<(ModelFile, Option<Value>), Error>, serde_json::Error>,
    ) -> String {
        match result {
            Err(e) => format!("not JSON: {e}"),
            Ok(Ok((file, imports))) => format!("loaded {} {imports:?}", file.namespace()),
            Ok(Err(e)) => describe(&e),
        }
    }

    /// The fold is exact for `ast`: when the typed read vouches for it the
    /// full check accepts it; `check_ast_shape` gives the full check's
    /// verdict and error; and the checked load of its text is the full
    /// check's error, or else the unchecked load. Returns whether the read
    /// vouched for it, and whether the full check accepts it.
    fn assert_exact(ast: &Value) -> (bool, bool) {
        let fast = ast_conforms(ast);
        let exact = check_ast_shape_exact(ast);
        assert!(!fast || exact.is_ok(), "vouched for, but rejected: {ast}");
        assert_eq!(
            check_ast_shape(ast).map_err(|e| describe(&e)),
            exact.as_ref().map(|_| ()).map_err(describe),
            "{ast}"
        );
        let text = ast.to_string();
        let checked = outcome(ModelFile::load_text(&text, None, None, true));
        let expected = match &exact {
            Err(e) => describe(e),
            Ok(()) => outcome(ModelFile::load_text(&text, None, None, false)),
        };
        assert_eq!(checked, expected, "{text}");
        (fast, exact.is_ok())
    }

    /// The table of `check_ast_shape`'s rules (P5-49 and P5-61, with
    /// BC-17, BC-19 and BC-20) and where each now lives, with an AST each
    /// rule rejects: every one is still rejected, with the same error, and
    /// not one of them is vouched for by the typed read.
    #[test]
    fn every_shape_rule_has_a_new_home() {
        let with_property = |key: &str, value: Value| {
            let mut property = string_property();
            property[key] = value;
            model(json!([concept(json!([property]))]))
        };
        let with_declaration = |key: &str, value: Value| {
            let mut declaration = concept(json!([string_property()]));
            declaration[key] = value;
            model(json!([declaration]))
        };
        let type_identifier = |name: Value| json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": name});
        // (rule, where it lives now, an AST it rejects, the error code)
        let table: Vec<(&str, &str, Value, &str)> = vec![
            (
                "BC-17 decorators not an array",
                "typed decode (Option<Vec<Decorator>>)",
                with_declaration("decorators", json!("d")),
                "modelfile-load-decoratorsnotarray",
            ),
            (
                "BC-17 decorators not an array (property)",
                "typed decode (Option<Vec<Decorator>>)",
                with_property("decorators", json!({})),
                "modelfile-load-decoratorsnotarray",
            ),
            (
                "BC-20 super type name not a non-empty string",
                "shape::declaration_conforms",
                with_declaration("superType", type_identifier(json!(""))),
                "modelfile-load-supertypename",
            ),
            (
                "BC-20 super type name not a string",
                "typed decode (TypeIdentifier.name: String)",
                with_declaration("superType", type_identifier(json!(7))),
                "modelfile-load-supertypename",
            ),
            (
                "BC-20 name not a string",
                "typed decode (name: String)",
                with_property("name", json!(["a"])),
                "modelfile-load-namenotstring",
            ),
            (
                "BC-20 name not a string (Value node)",
                "shape::node_conforms (Ty::Name)",
                model(json!([{"$class": "concerto.metamodel@1.0.0.StringScalar", "name": 1}])),
                "modelfile-load-namenotstring",
            ),
            (
                "node rule: identified",
                "shape::optional_node (IDENTIFIED)",
                with_declaration("identified", json!({})),
                "modelfile-load-nodenotobject",
            ),
            (
                "node rule: sizeValidator",
                "typed decode (CollectionSizeValidator)",
                with_property("sizeValidator", json!(1)),
                "modelfile-load-nodenotobject",
            ),
            (
                "node rule: lengthValidator",
                "typed decode (StringLengthValidator)",
                with_property("lengthValidator", json!({"minLength": 1})),
                "modelfile-load-nodenotobject",
            ),
            (
                "node rule: validator",
                "typed decode (StringRegexValidator)",
                with_property("validator", json!([])),
                "modelfile-load-nodenotobject",
            ),
            (
                "version check",
                "shape::header_conforms (Model $class)",
                json!({"$class": "concerto.metamodel@2.0.0.Model", "namespace": "org.acme@1.0.0"}),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: unknown key",
                "typed decode (Strict) / shape::object_conforms",
                with_property("undeclared", json!(1)),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: unknown key in a decorator argument",
                "shape::node_conforms",
                with_declaration(
                    "decorators",
                    json!([{"$class": "concerto.metamodel@1.0.0.Decorator", "name": "d",
                    "arguments": [{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "v", "x": 1}]}]),
                ),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: unknown key in identified",
                "shape::optional_node (IDENTIFIED)",
                with_declaration(
                    "identified",
                    json!({"$class": "concerto.metamodel@1.0.0.Identified", "x": 1}),
                ),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: wrong $class on a TypeIdentifier",
                "shape::type_identifier",
                with_declaration(
                    "superType",
                    json!({"$class": "concerto.metamodel@1.0.0.AliasedType", "name": "X"}),
                ),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: wrong $class on a validator",
                "shape::property_conforms",
                with_property(
                    "validator",
                    json!({"$class": "concerto.metamodel@1.0.0.AliasedType", "pattern": "a", "flags": ""}),
                ),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: Integer field with a fraction",
                "shape::property_conforms (integers)",
                with_property(
                    "lengthValidator",
                    json!({"$class": "concerto.metamodel@1.0.0.StringLengthValidator", "minLength": 1.5}),
                ),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: name fails the identifier validator",
                "shape::is_name, then the full check",
                with_property("name", json!("1a")),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: EnumProperty in a concept",
                "shape::property_conforms",
                model(json!([concept(
                    json!([{"$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "A"}])
                )])),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: required field null",
                "typed decode / shape::object_conforms (Need)",
                with_property("isArray", Value::Null),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: import unknown key",
                "shape::header_conforms (IMPORT)",
                json!({"$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0",
                    "imports": [{"$class": "concerto.metamodel@1.0.0.ImportAll", "namespace": "x@1.0.0", "x": 1}]}),
                "modelfile-load-astshape",
            ),
            (
                "metamodel: map key type of the wrong type",
                "shape::node_conforms (MAP_KEY_TYPE)",
                model(
                    json!([{"$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "M",
                    "key": {"$class": "concerto.metamodel@1.0.0.StringMapValueType"},
                    "value": {"$class": "concerto.metamodel@1.0.0.StringMapValueType"}}]),
                ),
                "modelfile-load-astshape",
            ),
            (
                "tolerance: DateTimeProperty defaultValue not a string",
                "shape::property_conforms (date_time_default)",
                model(json!([concept(
                    json!([{"$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "d",
                    "isArray": false, "isOptional": false, "defaultValue": 1}])
                )])),
                "modelfile-load-astshape",
            ),
        ];
        for (rule, home, ast, code) in table {
            let (fast, accepted) = assert_exact(&ast);
            assert!(!fast && !accepted, "{rule} ({home}): {ast}");
            let err = check_ast_shape(&ast).unwrap_err();
            assert_eq!(err.code(), code, "{rule} ({home})");
        }
        // BC-19's one tolerance is vouched for.
        let date_time = model(json!([concept(
            json!([{"$class": "concerto.metamodel@1.0.0.DateTimeProperty",
            "name": "d", "isArray": false, "isOptional": false, "defaultValue": "2020-01-01T00:00:00Z"}])
        )]));
        assert_eq!(assert_exact(&date_time), (true, true));
    }

    // -----------------------------------------------------------------------
    // Differential test over every model AST available
    // -----------------------------------------------------------------------

    fn collect_models(value: &Value, out: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                if map.get("$class").and_then(Value::as_str)
                    == Some("concerto.metamodel@1.0.0.Model")
                {
                    out.insert(value.to_string());
                }
                map.values().for_each(|v| collect_models(v, out));
            }
            Value::Array(items) => items.iter().for_each(|v| collect_models(v, out)),
            _ => {}
        }
    }

    fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                json_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "json") {
                out.push(path);
            }
        }
    }

    /// Every object node of `value`, by its path of keys and indexes.
    fn object_paths(value: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        match value {
            Value::Object(map) => {
                out.push(path.clone());
                for (key, child) in map {
                    path.push(key.clone());
                    object_paths(child, path, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    path.push(i.to_string());
                    object_paths(child, path, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }

    fn at<'a>(value: &'a mut Value, path: &[String]) -> &'a mut Value {
        path.iter().fold(value, |node, step| match node {
            Value::Array(items) => &mut items[step.parse::<usize>().unwrap()],
            other => &mut other[step.as_str()],
        })
    }

    /// One-change copies of `ast` at the object node at `path`: each key
    /// removed, set to `null` and to a value of every other JSON type; an
    /// undeclared key added; and its `$class` removed or replaced.
    fn mutants(ast: &Value, path: &[String]) -> Vec<Value> {
        let mut out = Vec::new();
        let node = at(&mut ast.clone(), path).clone();
        let Value::Object(map) = node else {
            return out;
        };
        let replacements = [
            Value::Null,
            json!(1.5),
            json!(2),
            json!(""),
            json!("x y"),
            json!("Name"),
            json!(true),
            json!([]),
            json!([null]),
            json!({}),
            json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "X"}),
        ];
        for key in map.keys() {
            let mut copy = ast.clone();
            at(&mut copy, path)
                .as_object_mut()
                .unwrap()
                .shift_remove(key);
            out.push(copy);
            for replacement in &replacements {
                let mut copy = ast.clone();
                at(&mut copy, path)[key.as_str()] = replacement.clone();
                out.push(copy);
            }
        }
        let mut copy = ast.clone();
        at(&mut copy, path)["undeclared"] = json!(null);
        out.push(copy);
        for class in [
            "concerto.metamodel@1.0.0.Identified",
            "concerto.metamodel@1.0.0.Range",
            "concerto.metamodel@1.0.0.StringScalar",
            "concerto.metamodel@1.0.0.Declaration",
            "concerto.metamodel@1.0.0.DateTimeMapKeyType",
            "concerto.metamodel@1.0.0.ImportTypes",
        ] {
            let mut copy = ast.clone();
            at(&mut copy, path)["$class"] = json!(class);
            out.push(copy);
        }
        out
    }

    /// Over every model AST of the benchmark sets and the oracle corpus
    /// (with `CONCERTO_ORACLE_FIXTURES` set) and this crate's own, and
    /// one-change mutants of every object node of a sample of them: the
    /// fold is exact ([`assert_exact`]). Every unmutated model the full
    /// check accepts is vouched for by the typed read (so the fold's fast
    /// path is the one a well-formed model takes).
    #[test]
    fn the_fold_is_exact_over_every_model_and_its_mutants() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut roots = Vec::new();
        if let Ok(fixtures) = std::env::var("CONCERTO_ORACLE_FIXTURES") {
            let fixtures = PathBuf::from(fixtures);
            if let Some(oracle) = fixtures.parent() {
                roots.push(oracle.join("cto-cache"));
                if let Some(migration) = oracle.parent() {
                    roots.push(migration.join("bench/fixtures/model-sets"));
                }
            }
            roots.push(fixtures);
        }
        roots.push(manifest.join("src"));
        roots.push(manifest.join("tests/typed_ast"));
        let mut files = Vec::new();
        roots.iter().for_each(|root| json_files(root, &mut files));
        let mut models = BTreeSet::new();
        for file in &files {
            if let Ok(text) = std::fs::read_to_string(file)
                && let Ok(value) = serde_json::from_str::<Value>(&text)
            {
                collect_models(&value, &mut models);
            }
        }
        assert!(!models.is_empty());

        let (mut accepted, mut vouched, mut mutated, mut mutants_vouched) = (0, 0, 0, 0);
        let mut not_vouched = Vec::new();
        for (index, text) in models.iter().enumerate() {
            let ast: Value = serde_json::from_str(text).unwrap();
            let (fast, ok) = assert_exact(&ast);
            accepted += usize::from(ok);
            vouched += usize::from(fast);
            if ok && !fast {
                not_vouched.push(text.clone());
            }
            // Mutants of up to 40 object nodes of every 13th model.
            if index % 13 != 0 {
                continue;
            }
            let mut paths = Vec::new();
            object_paths(&ast, &mut Vec::new(), &mut paths);
            let stride = paths.len().div_ceil(40).max(1);
            for path in paths.iter().step_by(stride) {
                for mutant in mutants(&ast, path) {
                    mutated += 1;
                    mutants_vouched += usize::from(assert_exact(&mutant).0);
                }
            }
        }
        eprintln!(
            "BC-19 fold: {} models from {} files: {accepted} pass the full check, {vouched} vouched for by the typed read; {mutated} mutants, {mutants_vouched} vouched for",
            models.len(),
            files.len()
        );
        assert!(
            not_vouched.is_empty(),
            "{} model(s) pass the full check but are not vouched for, e.g. {}",
            not_vouched.len(),
            not_vouched.first().map(String::as_str).unwrap_or_default()
        );
    }
}
