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

use crate::introspect::kept::{Kept, Location};
use crate::introspect::property::Property;
use crate::introspect::typed_ast::{
    ModelClass, ModelHeader, PropertyKept, ReadDecorators, TypedDeclaration, TypedModel,
};

/// The metamodel namespace, as a `$class` prefix.
const MM: &str = "concerto.metamodel@1.0.0.";

/// Whether the typed read of a model AST certainly passes BC-19's shape
/// check (the module doc). `false` means only "not certain".
pub(crate) fn conforms(model: &TypedModel) -> bool {
    header_conforms(&model.header) && model.declarations.iter().all(declaration_conforms)
}

/// The model's own keys (every top-level key but `declarations`).
fn header_conforms(header: &ModelHeader) -> bool {
    header.unknown.is_none()
        && match &header.class {
            Some(ModelClass::Model) => true,
            Some(ModelClass::Other(class)) => class_is(Some(class), "Model"),
            None => false,
        }
        && [
            &header.namespace,
            &header.source_uri,
            &header.concerto_version,
            &header.imports,
        ]
        .into_iter()
        .zip(MODEL)
        .all(|(value, (_, ty, need))| field_conforms(value.as_ref(), *ty, *need))
        && header
            .decorators
            .as_ref()
            .is_none_or(|decorators| match decorators {
                Ok(decorators) => decorators.conforms,
                Err(value) => decorators_conform(value),
            })
}

fn declaration_conforms(declaration: &TypedDeclaration) -> bool {
    match declaration {
        TypedDeclaration::Ast(value) => node_conforms(value, OTHER_DECLARATIONS),
        TypedDeclaration::Map(node) | TypedDeclaration::Scalar(node) => {
            kept_node_conforms(node, OTHER_DECLARATIONS)
        }
        TypedDeclaration::Class {
            node,
            properties,
            decorators,
            location,
            identified_conforms,
            ..
        } => {
            let (name, super_type) = node.name_and_super_type();
            is_name(name)
                && super_type.is_none_or(|t| type_identifier(t) && !t.name.is_empty())
                && *identified_conforms
                && ReadDecorators::conforms(decorators.as_ref())
                && optional_location(location.as_ref())
                && properties
                    .iter()
                    .all(|(property, kept)| property_conforms(property, kept, false))
        }
        TypedDeclaration::Enum {
            node,
            values,
            decorators,
            location,
        } => {
            is_name(&node.name)
                && ReadDecorators::conforms(decorators.as_ref())
                && optional_location(location.as_ref())
                && values
                    .iter()
                    .all(|(property, kept)| property_conforms(property, kept, true))
        }
    }
}

/// One property of a class-like declaration, or one value of an enum
/// declaration (`in_enum`).
fn property_conforms(property: &Property, kept: &PropertyKept, in_enum: bool) -> bool {
    if kept.unusual_decorators || !optional_location(kept.location.as_ref()) {
        return false;
    }
    // Only a `DateTimeProperty` keeps a `defaultValue` apart, and only a
    // string one is BC-19's tolerance.
    if kept
        .date_time_default
        .as_ref()
        .is_some_and(|value| !value.is_string())
    {
        return false;
    }
    match property {
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
/// The declarations the typed read keeps as a `Value` (or, a map
/// declaration, as a [`Kept`]).
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

/// `Model`, but for its `declarations`, which the typed read reads, and
/// its `decorators`, which it keeps as a [`Kept`] ([`decorators_conform`]),
/// in [`ModelHeader`]'s field order.
const MODEL: Fields = &[
    ("namespace", Str, Required),
    ("sourceUri", Str, Optional),
    ("concertoVersion", Str, Optional),
    ("imports", Nodes(IMPORT), Optional),
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
        && fields
            .iter()
            .all(|(key, ty, need)| field_conforms(map.get(*key), *ty, *need))
}

/// A declared field's value (`None` when missing) is as declared.
fn field_conforms(value: Option<&Value>, ty: Ty, need: Need) -> bool {
    match value {
        None => need != Required,
        Some(Value::Null) => need == Optional,
        Some(value) => value_conforms(value, ty),
    }
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

/// A class's `identified` value, as the read reads it (P5-76: as a
/// [`Kept`], not a `Value`; the verdict is the same): `null`, or a node of
/// the metamodel's `Identified` or `IdentifiedBy`.
pub(crate) fn identified_conforms(value: &Kept) -> bool {
    matches!(value, Kept::Other(Value::Null)) || kept_node_conforms(value, IDENTIFIED)
}

/// A node's `decorators` value, as the read reads it (P5-76: as a [`Kept`],
/// not a `Value`; the verdict is the same): `null`, or an array of
/// `Decorator` nodes.
pub(crate) fn decorators_conform(value: &Kept) -> bool {
    optional_nodes(Some(value), DECORATOR)
}

/// A node's optional `location` (a [`Location`], P5-76): the same
/// verdict as for its `Value`.
fn optional_location(value: Option<&Location>) -> bool {
    value.is_none_or(|value| match value {
        Location::Range(range) => range.conforms(),
        Location::Kept(kept) => kept.is_null() || kept_node_conforms(kept, RANGE),
    })
}

/// [`node_conforms`], for a [`Kept`]: the same verdict as for its `Value`.
fn kept_node_conforms(value: &Kept, allowed: &[&str]) -> bool {
    let Kept::Object(entries) = value else {
        // Not an object: no node at all.
        return false;
    };
    let Some(short) = (match value.get("$class") {
        Some(Kept::Other(Value::String(class))) => class.strip_prefix(MM),
        _ => None,
    }) else {
        return false;
    };
    allowed.contains(&short)
        && fields(short).is_some_and(|fields| {
            entries
                .iter()
                .all(|(key, _)| key == "$class" || fields.iter().any(|(name, ..)| name == key))
                && fields.iter().all(|(key, ty, need)| match value.get(key) {
                    None => *need != Required,
                    Some(Kept::Other(Value::Null)) => *need == Optional,
                    Some(value) => kept_value_conforms(value, *ty),
                })
        })
}

/// [`value_conforms`], for a [`Kept`]: the same verdict as for its `Value`.
fn kept_value_conforms(value: &Kept, ty: Ty) -> bool {
    match (value, ty) {
        (Kept::Other(value), _) => value_conforms(value, ty),
        (Kept::Object(_), Node(allowed)) => kept_node_conforms(value, allowed),
        (Kept::Array(items), Strs) => items
            .iter()
            .all(|item| matches!(item, Kept::Other(Value::String(_)))),
        (Kept::Array(items), Nodes(allowed)) => {
            items.iter().all(|item| kept_node_conforms(item, allowed))
        }
        // An object or an array of any other type.
        _ => false,
    }
}

/// An optional array-of-nodes field the read kept as given (a
/// `decorators`): missing, `null`, or an array of nodes of the types
/// `allowed`. Since P5-76 the read keeps it as a [`Kept`], not a `Value`;
/// the verdict is the same.
fn optional_nodes(value: Option<&Kept>, allowed: &'static [&'static str]) -> bool {
    value.is_none_or(|value| {
        matches!(value, Kept::Other(Value::Null)) || kept_value_conforms(value, Nodes(allowed))
    })
}

/// [`conforms`], for an AST given as a `Value`: the typed read of `ast`
/// ([`super::typed_ast::from_value`]) succeeds and conforms.
#[cfg_attr(
    not(feature = "js-compat"),
    expect(dead_code, reason = "js-compat seam only")
)]
pub(crate) fn ast_conforms(ast: &Value) -> bool {
    super::typed_ast::from_value(ast).is_ok_and(|model| conforms(&model))
}

#[cfg(test)]
mod tests;
