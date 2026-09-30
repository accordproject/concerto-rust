//! Properties, with their types kept intact.
//!
//! [`Property`] is a sum type whose variants are newtypes over the generated
//! metamodel structs: the eight field kinds of [`mm::Property`] and
//! [`mm::EnumProperty`]. Each node is deserialized into the concrete struct its
//! fully qualified metamodel `$class` names (`property_kind`), so the
//! validators and the referenced `type` are kept whole. A
//! class declaration keeps its properties as this type too, because it also
//! accepts an `EnumProperty`, which the generated `mm::Property` union does
//! not cover. The getters hang off the enum directly, and those it shares
//! with the declarations come from the traits in [`crate::introspect`].

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::Value;

use crate::derive::Named;
use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::decorator::{
    Decorated, Decorator, WithDecorators, null_decorator, parse_decorators,
};
use crate::introspect::validators;
use crate::introspect::{FullyQualified, METAMODEL_NAMESPACE, Named, Typed, declared_class};
use crate::model_util::{is_system_property, is_valid_identifier};

js_compat_pub! {
    /// What `Property.process` computes, after `super.process()` (which belongs
    /// to `Decorated`).
    ///
    /// TS: `Property.process` (src/introspect/property.ts). `property_type` is
    /// `this.type`; `type_set` says whether TS assigns `this.type` at all —
    /// the `EnumProperty` arm of the source switch falls through without an
    /// assignment, so `this.type` is left `undefined` there, which the WASM view
    /// tells apart from the explicit `null` an `ObjectProperty` with no `type`
    /// AST node gets.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ProcessedProperty {
        /// `this.name`.
        pub name: String,
        /// `this.type`, when the switch sets it.
        pub property_type: Option<String>,
        /// Whether the switch sets `this.type` at all (`false` for `EnumProperty`).
        pub type_set: bool,
        /// `this.array`.
        pub array: bool,
        /// `this.optional`.
        pub optional: bool,
    }
}

js_compat_pub! {
    /// Computes `Property.process`'s fields directly from the AST, in the TS
    /// order: the identifier check, the name, the `$class` switch for `type`,
    /// then `array` and `optional`. `this.sizeValidator` is not computed here:
    /// TS builds it by constructing a `CollectionSizeValidator`, which the WASM
    /// view still does directly (its own binding already ports the TS
    /// constructor).
    ///
    /// TS: `Property.process` (src/introspect/property.ts). TS's check is
    /// `ID_REGEX.test(this.ast.name)`, and `RegExp.prototype.test` runs
    /// `ToString` on a non-string argument rather than rejecting it outright
    /// (`ecma::to_js_string`, matching the `ID_REGEX.test(undefined)` quirk
    /// `model_util::is_valid_identifier`'s own tests document, DV-002): a
    /// fuzz-mutated `name` that is present but not a JSON string (a bool, a
    /// number, an array, `null`, an object) must go through the same coercion,
    /// not be read as an absent name (accordproject/concerto-rust#217): e.g.
    /// `name: true` stringifies to `"true"`, which passes `ID_REGEX` in both
    /// engines, so TS accepts the model and a naive `Value::as_str` default of
    /// `""` made Rust wrongly reject it as `Invalid property name ''`.
    pub fn process<E: From<ContractError>>(ast: &Value) -> std::result::Result<ProcessedProperty, E> {
        // TS interpolates the raw `this.ast.name` into a template literal
        // (`Invalid property name '${this.ast.name}'`) and into `ID_REGEX.test`,
        // both of which apply JS `ToString` to whatever value the AST carries —
        // not only a string. A fuzzer-mutated AST can put a number, boolean,
        // `null`, array or object there (or omit the key, `ToString`d as
        // `"undefined"`), so this must go through the same `ToString` coercion
        // `ecma::to_js_string` gives every other port of a template literal,
        // rather than treating a non-string name as absent
        // (accordproject/concerto-rust#217, #219).
        let raw_name = ast.get("name");
        let name = raw_name
            .map(crate::ecma::to_js_string)
            .unwrap_or_else(|| "undefined".to_string());
        if !is_valid_identifier(&name) {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "property-process-invalidname",
                vec![("name", name)],
            );
            // TS: `throw new IllegalModelException(..., this.getModelFile(),
            // this.ast.location)` — the WASM binding (`propertyProcess` in
            // concerto-wasm/src/lib.rs) supplies the real JS model file once
            // `model_file` says one belongs on this error; the location comes
            // from this AST node directly, as every other site on this path
            // does (e.g. `Property::try_from`'s own `invalidname` throw).
            err.location = ast.get("location").cloned();
            err.model_file = Some(None);
            return Err(err.into());
        }
        // TS: `this.name = this.ast.name; if (!this.name) { throw new
        // Error('No name for type ' + JSON.stringify(this.ast)); }` — a
        // *second*, separate check, on the *raw* `this.ast.name` value's own JS
        // truthiness, not on the `ToString`'d `name` the identifier check just
        // validated above. `ID_REGEX.test` can accept a falsy value whose
        // stringified form still looks like an identifier (`false` stringifies
        // to `"false"`, a valid identifier shape) while the value itself is
        // falsy (`false`, `0`, `""`, `null`, absent), so this must re-test the
        // untouched AST value, not `name` (accordproject/concerto-rust#219,
        // P5-05 stage-2 T2c: minimised sample sets a property's `name` to the
        // JSON boolean `false`).
        if !raw_name.is_some_and(crate::ecma::is_truthy) {
            return Err(ContractError::new(
                ErrorKind::InvalidArgument,
                "property-process-noname",
                vec![("ast", ast.to_string())],
            )
            .into());
        }

        let class = ast
            .get("$class")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // TS `switch (this.ast.$class)` matches the full metamodel `$class`
        // (`===`); anything else takes no arm (accordproject/concerto-rust#285).
        let short = property_kind(class).unwrap_or_default();
        let object_or_relationship_type = || {
            ast.get("type")
                .and_then(|t| t.get("name"))
                .and_then(Value::as_str)
                .map(String::from)
        };
        let (property_type, type_set): (Option<String>, bool) = match short {
            "BooleanProperty" => (Some("Boolean".to_string()), true),
            "DateTimeProperty" => (Some("DateTime".to_string()), true),
            "DoubleProperty" => (Some("Double".to_string()), true),
            "IntegerProperty" => (Some("Integer".to_string()), true),
            "LongProperty" => (Some("Long".to_string()), true),
            "StringProperty" => (Some("String".to_string()), true),
            "ObjectProperty" => (object_or_relationship_type(), true),
            "RelationshipProperty" => {
                // DV-017: TS reads `this.ast.type.name` unguarded here and throws
                // a `TypeError` for a missing or `null` `type`; Rust rejects it
                // with an `IllegalModelException` instead (maintainer decision
                // on accordproject/concerto-rust#218).
                if let Some(mut err) = relationship_without_type(ast, &name) {
                    // TS passes `this.getModelFile()` to the exception; the WASM
                    // shim (`propertyProcess`) substitutes the real JS model
                    // file when this is `Some`.
                    err.model_file = Some(None);
                    return Err(err.into());
                }
                (object_or_relationship_type(), true)
            }
            // `EnumProperty`, or anything else: the TS switch has no matching
            // `case`, so `this.type` is left unassigned.
            _ => (None, false),
        };

        let array = ast.get("isArray").and_then(Value::as_bool).unwrap_or(false);
        let optional = ast
            .get("isOptional")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        Ok(ProcessedProperty {
            name,
            property_type,
            type_set,
            array,
            optional,
        })
    }
}

/// DV-017 (maintainer-accepted, accordproject/concerto-rust#218): a
/// `RelationshipProperty` node whose `type` is missing or `null`.
///
/// TS `Property.process`'s `RelationshipProperty` arm (property.ts:165) reads
/// `this.ast.type.name` with no guard (its `ObjectProperty` arm, two lines
/// above, has one), so V8 throws `TypeError: Cannot read properties of
/// undefined (reading 'name')` (or `of null`) from the `ModelFile`
/// constructor. Rust does not port that crash: it raises
/// `IllegalModelException: Relationship <name> must have a type`
/// (`property-process-relationshipnotype`), worded like TS's own
/// `RelationshipDeclaration.validate` rejections, with this property's AST
/// `location` and its model file (attached by the caller: the declaration
/// loader natively, the WASM shim in rust mode). Any other `type`
/// value (a string, a number, an object with no `name`) does not crash TS,
/// so it is not this check's.
///
/// Returns `None` when the node is not a `RelationshipProperty` or has a
/// non-null `type`. `name` is the (already validated) property name.
pub(crate) fn relationship_without_type(ast: &Value, name: &str) -> Option<ContractError> {
    let class = ast.get("$class").and_then(Value::as_str)?;
    if property_kind(class) != Some("RelationshipProperty") {
        return None;
    }
    if !ast.get("type").is_none_or(Value::is_null) {
        return None;
    }
    let mut err = ContractError::new(
        ErrorKind::IllegalModel,
        "property-process-relationshipnotype",
        vec![("name", name.to_string())],
    );
    err.location = ast.get("location").cloned();
    Some(err)
}

/// A single property of a concept-like or enum declaration. Each variant also
/// carries its processed decorators (module doc on
/// [`crate::introspect::decorator::WithDecorators`]).
#[derive(Debug, Clone, Named)]
#[non_exhaustive]
pub enum Property {
    /// A `Boolean` primitive field.
    Boolean(WithDecorators<mm::BooleanProperty>),
    /// A `String` primitive field (may carry regex/length validators).
    String(WithDecorators<mm::StringProperty>),
    /// An `Integer` primitive field (may carry a domain validator).
    Integer(WithDecorators<mm::IntegerProperty>),
    /// A `Long` primitive field (may carry a domain validator).
    Long(WithDecorators<mm::LongProperty>),
    /// A `Double` primitive field (may carry a domain validator).
    Double(WithDecorators<mm::DoubleProperty>),
    /// A `DateTime` primitive field.
    DateTime(WithDecorators<mm::DateTimeProperty>),
    /// A field whose type is another declared concept/scalar.
    Object(WithDecorators<mm::ObjectProperty>),
    /// A relationship reference to an identifiable declaration.
    Relationship(WithDecorators<mm::RelationshipProperty>),
    /// A value member of an enum declaration.
    Enum(WithDecorators<mm::EnumProperty>),
}

/// Picks the same field out of whichever generated struct a [`Property`]
/// holds. The eight field kinds share the metamodel's property fields; an
/// enum value has fewer, so its arm is given separately.
macro_rules! property_field {
    ($property:expr, $p:ident => $field:expr, $value:pat => $enum_value:expr) => {
        match $property {
            Property::Boolean($p) => $field,
            Property::String($p) => $field,
            Property::Integer($p) => $field,
            Property::Long($p) => $field,
            Property::Double($p) => $field,
            Property::DateTime($p) => $field,
            Property::Object($p) => $field,
            Property::Relationship($p) => $field,
            Property::Enum($value) => $enum_value,
        }
    };
}

impl Property {
    /// Whether the property is an array (`[]`). Enum members are never arrays.
    pub fn is_array(&self) -> bool {
        property_field!(self, p => p.is_array, _ => false)
    }

    /// Whether the property is optional. Enum members are never optional.
    pub fn is_optional(&self) -> bool {
        property_field!(self, p => p.is_optional, _ => false)
    }

    /// `true` for the six primitive property kinds.
    pub fn is_primitive(&self) -> bool {
        matches!(
            self,
            Self::Boolean(_)
                | Self::String(_)
                | Self::Integer(_)
                | Self::Long(_)
                | Self::Double(_)
                | Self::DateTime(_)
        )
    }

    /// `true` if this is a relationship reference.
    pub fn is_relationship(&self) -> bool {
        matches!(self, Self::Relationship(_))
    }

    /// `true` if this is an enum value member.
    pub fn is_enum_value(&self) -> bool {
        matches!(self, Self::Enum(_))
    }

    /// The referenced type identifier, for object and relationship properties.
    pub fn type_identifier(&self) -> Option<&mm::TypeIdentifier> {
        match self {
            Self::Object(p) => Some(&p.type_),
            Self::Relationship(p) => Some(&p.type_),
            _ => None,
        }
    }

    /// The collection size validator, if one is declared on this property.
    pub fn size_validator(&self) -> Option<&mm::CollectionSizeValidator> {
        property_field!(self, p => p.size_validator.as_ref(), _ => None)
    }

    /// This property's own AST `location`, if the node carried one. Every
    /// generated property struct (including `EnumProperty`) has a `location`
    /// field, so — unlike `Decorator`, which still has none (7.2) — a
    /// property can report its own location rather than borrowing its owning
    /// class's the way validation used to (P2-08 review carry-over (c) from
    /// P2-04's review, #48: TS `Property.validate`/`Decorated.validate` throw
    /// with `this.ast.location`, the property's own).
    pub fn location(&self) -> Option<&mm::Range> {
        property_field!(self, p => p.location.as_ref(), p => p.location.as_ref())
    }
}

impl Typed for Property {
    /// The name of the property's type. For primitives that's the primitive
    /// itself; for object/relationship properties it's the type they point at.
    /// Enum members don't have a type, so they get `None`.
    fn type_name(&self) -> Option<&str> {
        match self {
            Self::Boolean(_) => Some("Boolean"),
            Self::String(_) => Some("String"),
            Self::Integer(_) => Some("Integer"),
            Self::Long(_) => Some("Long"),
            Self::Double(_) => Some("Double"),
            Self::DateTime(_) => Some("DateTime"),
            Self::Object(p) => Some(&p.type_.name),
            Self::Relationship(p) => Some(&p.type_.name),
            Self::Enum(_) => None,
        }
    }
}

impl Property {
    /// The property's name.
    ///
    /// TS: Property.getName (src/introspect/property.ts)
    pub fn name(&self) -> &str {
        Named::name(self)
    }

    /// The name of the property's type: the primitive itself for a primitive
    /// property, the type it points at for an object or relationship
    /// property, and `None` for an enum value, which has no type.
    ///
    /// TS: Property.getType (src/introspect/property.ts)
    pub fn type_name(&self) -> Option<&str> {
        Typed::type_name(self)
    }

    /// The decorators attached to the property, in the order they are given.
    ///
    /// TS: `Decorated.getDecorators` (src/introspect/decorated.ts)
    pub fn decorators(&self) -> &[Decorator] {
        Decorated::decorators(self)
    }
}

impl Decorated for Property {
    fn decorators(&self) -> &[Decorator] {
        property_field!(self, p => p.decorators(), p => p.decorators())
    }
}

/// The property `$class` short names TS `ClassDeclaration.process` builds a
/// property view for.
const PROPERTY_KINDS: [&str; 9] = [
    "RelationshipProperty",
    "EnumProperty",
    "BooleanProperty",
    "StringProperty",
    "IntegerProperty",
    "LongProperty",
    "DoubleProperty",
    "DateTimeProperty",
    "ObjectProperty",
];

/// A property node's kind (its `$class` short name), when its `$class` is
/// one of the nine full metamodel property classes; `None` otherwise.
///
/// TS: `ClassDeclaration.process`'s properties loop
/// (src/introspect/classdeclaration.ts) compares `thing.$class` with each
/// `` `${MetaModelNamespace}.<Kind>Property` `` by `===`, and throws
/// "Unrecognised model element" for anything else: a bare short name
/// (`StringProperty`), another namespace's (`foo.StringProperty`), or any
/// other text that merely ends in a property class's short name
/// (`concerto.metamodel@1.0.0.StringPropertyconcerto.metamodel@1.0.0.StringProperty`).
/// Matching by `get_short_name` (the text after the last `.`) accepted
/// all three (accordproject/concerto-rust#285, BC-25).
pub(crate) fn property_kind(class: &str) -> Option<&str> {
    let kind = class.strip_prefix(METAMODEL_NAMESPACE)?.strip_prefix('.')?;
    PROPERTY_KINDS.contains(&kind).then_some(kind)
}

/// TS: the `else` branch of `ClassDeclaration.process`'s properties loop
/// (classdeclaration.ts): `IllegalModelException` naming the unrecognised
/// `thing.$class`. `this.modelFile`/`this.ast.location` there are the
/// *class's*, which [`Property::try_from`] has no way to reach (the module
/// doc on [`BoundElement`]), so this carries neither.
fn unrecognised_property(class: &str) -> Error {
    ContractError::new(
        ErrorKind::IllegalModel,
        "classdeclaration-process-unrecmodelelem",
        vec![("type", class.to_string())],
    )
    .into()
}

/// The keys of a property node of kind `kind` (its `$class` short name)
/// that [`Property::try_from`] keeps out of the strict decode into the
/// generated struct, and rebuilds from the raw AST instead
/// ([`Property::set_ast_validators`]).
pub(crate) fn ast_validator_keys(kind: &str) -> &'static [&'static str] {
    if kind == "StringProperty" {
        &["sizeValidator", "lengthValidator", "validator"]
    } else {
        &["sizeValidator"]
    }
}

/// The `type` [`Property::try_from`] gives an `ObjectProperty` whose AST has
/// none (or `null`): an empty `TypeIdentifier`, standing in for TS's `null`.
pub(crate) fn object_type_placeholder() -> Value {
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.TypeIdentifier",
        "name": ""
    })
}

impl Property {
    /// Sets the validators [`Property::try_from`] rebuilds from the raw AST
    /// ([`ast_validator_keys`]), `raw` giving the property node's value for
    /// a key. An enum value has none.
    pub(crate) fn set_ast_validators<'v>(&mut self, raw: impl Fn(&str) -> Option<&'v Value>) {
        let size_validator = || validators::size_validator_from_ast(raw("sizeValidator"));
        match self {
            Self::Boolean(p) => p.node_mut().size_validator = size_validator(),
            Self::String(p) => {
                let node = p.node_mut();
                node.size_validator = size_validator();
                node.length_validator =
                    validators::length_validator_from_ast(raw("lengthValidator"));
                node.validator = validators::regex_validator_from_ast(raw("validator"));
            }
            Self::Integer(p) => p.node_mut().size_validator = size_validator(),
            Self::Long(p) => p.node_mut().size_validator = size_validator(),
            Self::Double(p) => p.node_mut().size_validator = size_validator(),
            Self::DateTime(p) => p.node_mut().size_validator = size_validator(),
            Self::Object(p) => p.node_mut().size_validator = size_validator(),
            Self::Relationship(p) => p.node_mut().size_validator = size_validator(),
            Self::Enum(_) => {}
        }
    }
}

impl TryFrom<&serde_json::Value> for Property {
    type Error = Error;

    fn try_from(value: &serde_json::Value) -> Result<Self> {
        // Concerto keeps a set of property names for itself, so a model may
        // not declare a field with one of them. TS: the first check of
        // `ClassDeclaration.process`'s properties loop, ahead of its `$class`
        // match.
        if let Some(name) = value.get("name").and_then(|n| n.as_str())
            && is_system_property(name)
        {
            return Err(Error::illegal_model(
                format!("Invalid field name '{name}'"),
                None,
                None,
            ));
        }
        let class = declared_class(value);
        if class.is_empty() {
            return Err(Error::illegal_model(
                "property node is missing its $class",
                None,
                None,
            ));
        }
        // TS: `ClassDeclaration.process`'s loop matches the full `$class`
        // (`===`) before it constructs the property, so an unrecognised one
        // is reported ahead of everything `Property.process` checks.
        let Some(kind) = property_kind(class) else {
            return Err(unrecognised_property(class));
        };
        // TS `Property.process` (property.ts) starts with `super.process()`
        // (`Decorated.process`), so a `null` decorator node (DV-018,
        // `null_decorator`) is reported ahead of every other property check.
        // The model file's name is filled in by `Declaration::from_model_json`
        // (`with_model_file`).
        if let Some(err) = null_decorator(value) {
            return Err(err.into());
        }
        // TS `Property.process` (property.ts): `ModelUtil.isValidIdentifier`
        // treats a nullish `ast.name` as valid (DV-002: `String(undefined)`/
        // `String(null)` are valid identifiers), so it is the very next
        // check, `if (!this.name)`, that rejects it — with a plain `Error`,
        // not an `IllegalModelException`, before the `$class` switch (and so
        // before any deserialization into the concrete property struct,
        // which would otherwise fail first on the missing `name` field with
        // an unrelated message).
        if value.get("name").is_none_or(|n| n.is_null()) {
            return Err(ContractError::new(
                ErrorKind::InvalidArgument,
                "property-process-noname",
                vec![("ast", value.to_string())],
            )
            .into());
        }

        // DV-017: a `RelationshipProperty` with a missing or `null` `type`
        // (see [`relationship_without_type`]). TS's `Property.process` checks
        // the name before its `$class` switch, so an invalid name is still
        // reported first; a non-string name is left to serde below.
        if kind == "RelationshipProperty"
            && let Some(name) = value.get("name").and_then(Value::as_str)
            && let Some(no_type) = relationship_without_type(value, name)
        {
            if is_valid_identifier(name) {
                return Err(no_type.into());
            }
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "property-process-invalidname",
                vec![("name", name.to_string())],
            );
            err.location = value.get("location").cloned();
            return Err(err.into());
        }

        // Parse into whatever struct the `$class` says this is. If serde
        // chokes, the JSON is malformed for the kind it claims to be.
        let bad =
            |e: serde_json::Error| Error::illegal_model(format!("invalid {kind}: {e}"), None, None);

        // TS `Property.process`'s `ObjectProperty` arm (property.ts):
        // `this.type = this.ast.type ? this.ast.type.name : null` — a
        // missing (or `null`) `type` node is not an error, unlike every
        // other check on this path; `RelationshipProperty`'s own arm has no
        // such guard (`this.ast.type.name` unconditionally), so this is
        // `ObjectProperty` only. `mm::ObjectProperty::type_` has no `Option`
        // (the generated struct always requires it), so a placeholder empty
        // `TypeIdentifier` stands in for TS's `null`: [`Property::type_identifier`]
        // then reads an empty name, which every other check on this path
        // already treats the same as "no type" (TS's own `this.type` is
        // equally falsy for `""` and `null`).
        let value = if kind == "ObjectProperty" && value.get("type").is_none_or(|t| t.is_null()) {
            let mut patched = value.clone();
            if let Some(map) = patched.as_object_mut() {
                map.insert("type".into(), object_type_placeholder());
            }
            std::borrow::Cow::Owned(patched)
        } else {
            std::borrow::Cow::Borrowed(value)
        };
        let value = value.as_ref();

        let decorators = parse_decorators(value);
        // Every non-enum kind's own `sizeValidator` (and, for a
        // `StringProperty`, its `lengthValidator`/`validator`) is set aside
        // before the strict struct decode below and rebuilt straight from
        // this untouched `value` with `validators::size_validator_from_ast`/
        // `length_validator_from_ast`/`regex_validator_from_ast`
        // ([`Property::set_ast_validators`]): `serde`'s
        // derived `Deserialize` requires an actual JSON number/string for
        // their nested fields, but TS reads every one of them completely
        // untyped (those functions' own doc comments), so a fuzz-mutated,
        // wrongly-typed field there must not fail the whole property's
        // parse (accordproject/concerto-rust#217).
        let mut sanitized = value.clone();
        if let Some(map) = sanitized.as_object_mut() {
            for key in ast_validator_keys(kind) {
                map.remove(*key);
            }
        }
        let mut property = match kind {
            "BooleanProperty" => Self::Boolean(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            "StringProperty" => Self::String(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            "IntegerProperty" => Self::Integer(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            "LongProperty" => Self::Long(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            "DoubleProperty" => Self::Double(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            "DateTimeProperty" => Self::DateTime(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            "ObjectProperty" => Self::Object(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            "RelationshipProperty" => Self::Relationship(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            "EnumProperty" => Self::Enum(WithDecorators::new(
                serde_json::from_value(sanitized).map_err(bad)?,
                decorators,
            )),
            _ => return Err(unrecognised_property(class)),
        };
        property.set_ast_validators(|key| value.get(key));
        if !is_valid_identifier(property.name()) {
            // TS: `Property.process` (property.ts) — `this.getModelFile()`
            // and `this.ast.location`; `try_from` has no model file in
            // scope (the module doc on [`BoundElement`]), so only the
            // location, which is this property's own AST node, is set here.
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "property-process-invalidname",
                vec![("name", property.name().to_string())],
            );
            err.location = value.get("location").cloned();
            return Err(err.into());
        }
        Ok(property)
    }
}

/// The property as the element its own validator is attached to, once the
/// fully qualified name is known — TS: `this` (`Property`/`Field`), whose
/// `getFullyQualifiedName()` needs the owning class and namespace that
/// [`Property::try_from`] (and so [`parse_properties`](super::declaration))
/// never has. [`Property::check_bound_validators`] builds this once that
/// context is known.
struct BoundElement<'a> {
    fqn: &'a str,
    name: &'a str,
    default_value: Option<Value>,
}

impl FullyQualified for BoundElement<'_> {
    type Error = Error;

    fn fully_qualified_name(&self) -> Result<String> {
        Ok(self.fqn.to_string())
    }
}

impl crate::model_manager::ValidatedElement for BoundElement<'_> {
    fn default_value(&self) -> Result<Option<Value>> {
        Ok(self.default_value.clone())
    }

    fn name(&self) -> Result<String> {
        Ok(self.name.to_string())
    }
}

/// Converts a generated numeric domain validator struct (`IntegerDomainValidator`,
/// `LongDomainValidator` or `DoubleDomainValidator` — all three share the same
/// `{$class, lower, upper}` shape) to the raw JSON [`validators::NumberValidator::new`]
/// reads, the same shape [`ScalarDeclaration::process`](super::scalar::ScalarDeclaration::process)
/// and [`field::process`](super::field::process) read straight from the AST.
fn domain_validator_json<T: serde::Serialize>(validator: &T) -> Value {
    // Infallible: every field of these generated structs serializes (no
    // floating `NaN`/`Infinity`, which `serde_json` alone cannot represent —
    // OD-3's widened numeric AST fields never hold one).
    serde_json::to_value(validator).unwrap_or(Value::Null)
}

impl Property {
    /// Checks a collection size validator's own bounds, as TS's
    /// `CollectionSizeValidator` constructor does while the property is
    /// processed. Whether the property may carry one at all (an array, or a
    /// map-typed property) is not checked here: TS checks that only in
    /// `Property.validate` (property.ts), once the property's type can be
    /// resolved — `check_property_type` in [`crate::validation`] (P2-08: a
    /// `ModelFile` with such a property must still construct).
    fn check_size_validator(
        fqn: &str,
        name: &str,
        validator: Option<&mm::CollectionSizeValidator>,
        raw: Option<&Value>,
    ) -> Result<()> {
        let Some(v) = validator else { return Ok(()) };
        let element = BoundElement {
            fqn,
            name,
            default_value: None,
        };
        validators::CollectionSizeValidator::new(&element, v, raw)?;
        Ok(())
    }

    /// Whether [`Property::check_bound_validators`] has a validator to
    /// rebuild: a size validator, or a String, Integer, Long or Double
    /// domain (or length) validator (P5-48).
    pub(crate) fn has_bound_validators(&self) -> bool {
        self.size_validator().is_some()
            || match self {
                Self::String(p) => p.validator.is_some() || p.length_validator.is_some(),
                Self::Integer(p) => p.validator.is_some(),
                Self::Long(p) => p.validator.is_some(),
                Self::Double(p) => p.validator.is_some(),
                _ => false,
            }
    }

    js_compat_pub! {
        /// Rebuilds and discards this property's own numeric, string and
        /// collection-size validators, purely to surface the
        /// `IllegalModelException` their constructors raise (BC-39, R1:
        /// `ErrorKind::IllegalModel`, keeping the errorType; 5.0.0 raised a
        /// `BaseException` through `Validator.reportError`) for a bound out of order, a
        /// negative size, an uncompilable regex, or a default value outside the
        /// validator's own range — `Property::try_from` itself has no
        /// [`FullyQualified`] context to build these messages with (the module
        /// doc on `BoundElement`), so this is called once that context is
        /// known, from `ClassDeclaration::from_json`
        /// (`super::declaration::ClassDeclaration`), never from `try_from`
        /// itself: a property whose validator does not check out must still
        /// *parse*, exactly as TS's own two-phase load (parse, then
        /// `ClassDeclaration.process`'s validator construction) does.
        ///
        /// TS: `Property.process`/`Field.process` (property.ts, field.ts) build
        /// the size validator (any non-enum property) first, then, for a
        /// non-array Integer/Long/Double/String, its own domain or
        /// length-and-regex validator — the same order as this method's own
        /// `match`.
        ///
        /// `raw` is this property's own AST node, when the caller has it (only
        /// `super::declaration::ClassDeclaration::from_json` does — it is what
        /// lets [`validators::CollectionSizeValidator::new`]/
        /// [`validators::StringValidator::new`] compare a fuzzed `sizeValidator`/
        /// `lengthValidator`'s `minSize`/`maxSize`/`minLength`/`maxLength` with
        /// JS's own untyped `>` instead of a value already coerced to `f64`
        /// (accordproject/concerto-rust#219): `None` falls back to the `f64`
        /// comparison, the same question for already-validated data.
        pub fn check_bound_validators(&self, class_fqn: &str, raw: Option<&Value>) -> Result<()> {
            // P5-48: a property with no validator to rebuild (most) returns
            // before its fully-qualified name is built; every arm below is
            // then a no-op.
            if !self.has_bound_validators() {
                return Ok(());
            }
            let name = self.name().to_string();
            // TS: `Validator.getFieldOrScalarDeclaration().getFullyQualifiedName()`
            // — a property's own, `<namespace>.<Class>.<property>` (property.ts
            // `getFullyQualifiedName`), not its owning class's.
            let fqn = format!("{class_fqn}.{name}");
            let fqn = fqn.as_str();
            let raw_field = |key: &str| raw.and_then(|r| r.get(key));
            Self::check_size_validator(
                fqn,
                &name,
                self.size_validator(),
                raw_field("sizeValidator"),
            )?;
            let element = |default_value: Option<Value>| BoundElement {
                fqn,
                name: &name,
                default_value,
            };
            match self {
                Self::String(p) if p.validator.is_some() || p.length_validator.is_some() => {
                    let default_value = p.default_value.clone().map(Value::String);
                    validators::StringValidator::new(
                        &element(default_value),
                        p.validator.as_ref(),
                        p.length_validator.as_ref(),
                        raw_field("lengthValidator"),
                    )?;
                    Ok(())
                }
                Self::Integer(p) if p.validator.is_some() => {
                    let ast = domain_validator_json(p.validator.as_ref().unwrap());
                    let default_value = p.default_value.map(Value::from);
                    validators::NumberValidator::new(&element(default_value), &ast)?;
                    Ok(())
                }
                Self::Long(p) if p.validator.is_some() => {
                    let ast = domain_validator_json(p.validator.as_ref().unwrap());
                    let default_value = p.default_value.map(Value::from);
                    validators::NumberValidator::new(&element(default_value), &ast)?;
                    Ok(())
                }
                Self::Double(p) if p.validator.is_some() => {
                    let ast = domain_validator_json(p.validator.as_ref().unwrap());
                    let default_value = p.default_value.map(Value::from);
                    validators::NumberValidator::new(&element(default_value), &ast)?;
                    Ok(())
                }
                _ => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_derives_type_array_and_optional() {
        let processed = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "email",
            "isArray": true,
            "isOptional": true
        }))
        .expect("valid");
        assert_eq!(processed.name, "email");
        assert_eq!(processed.property_type.as_deref(), Some("String"));
        assert!(processed.type_set);
        assert!(processed.array);
        assert!(processed.optional);
    }

    #[test]
    fn process_object_property_type_is_the_referenced_name() {
        let processed = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty",
            "name": "address",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Address" }
        }))
        .expect("valid");
        assert_eq!(processed.property_type.as_deref(), Some("Address"));
    }

    #[test]
    fn process_enum_property_leaves_type_unset() {
        let processed = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.EnumProperty",
            "name": "RED"
        }))
        .expect("valid");
        assert!(!processed.type_set);
        assert_eq!(processed.property_type, None);
        assert!(!processed.array);
        assert!(!processed.optional);
    }

    #[test]
    fn process_rejects_an_invalid_identifier() {
        let err = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "1bad",
            "isArray": false,
            "isOptional": false
        }))
        .unwrap_err();
        assert!(err.to_string().contains("Invalid property name '1bad'"));
    }

    // accordproject/concerto-rust#219 (P5-05 stage-2 T2c, cluster 1): a
    // fuzzer-mutated AST can put any JSON type in `name`, and TS's
    // `${this.ast.name}` reports it through JS `ToString`, not as an
    // absent/empty name.
    #[test]
    fn process_rejects_a_non_string_name_with_its_js_stringified_form() {
        let err = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": 1e308,
            "isArray": false,
            "isOptional": false
        }))
        .unwrap_err();
        assert!(err.to_string().contains("Invalid property name '1e+308'"));
    }

    // accordproject/concerto-rust#219 review (P5-05 stage-2 T2c, "the
    // location-suffix cluster"): the `IllegalModelException` an invalid name
    // raises carries the AST node's own `location` and a `model_file`
    // placeholder, so `ModelManager.addModelFile`'s WASM binding
    // (`propertyProcess`) can attach the real JS model file and reproduce
    // TS's `File '…': line <n> column <n>, to line <n> column <n>.` suffix.
    // This test reproduces that suffix text directly, with no WASM boundary
    // to cross: `ContractError::final_message` is the same pure-Rust
    // function the native oracle harness itself uses to decorate a message
    // exactly as TS's `IllegalModelException` constructor does (OD-2) — once
    // a real (not placeholder) file name is filled in, in place of the WASM
    // binding, it renders the identical suffix. A prior version of this test
    // only asserted `err.location.is_some()`/`err.model_file == Some(None)`
    // (the placeholder itself), which checks that a location and a
    // model-file slot exist but not that the slot, once filled, actually
    // renders TS's suffix text — that is what this asserts.
    #[test]
    fn process_carries_the_ast_location_and_a_model_file_placeholder_for_an_invalid_name() {
        let mut err = process::<ContractError>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": 1e308,
            "isArray": false,
            "isOptional": false,
            "location": {
                "$class": "concerto.metamodel@1.0.0.Range",
                "start": {
                    "$class": "concerto.metamodel@1.0.0.Position",
                    "line": 3, "column": 5, "offset": 20
                },
                "end": {
                    "$class": "concerto.metamodel@1.0.0.Position",
                    "line": 3, "column": 30, "offset": 45
                }
            }
        }))
        .unwrap_err();
        assert!(
            err.location.is_some(),
            "expected the AST's own location on the error"
        );
        assert_eq!(
            err.model_file,
            Some(None),
            "expected a model-file placeholder for the WASM binding to fill in"
        );
        // Fill in the placeholder the way `propertyProcess` fills it from
        // the real JS `ModelFile`, and check the fully decorated message —
        // TS's own `ModelManager.addModelFile` suffix — matches verbatim.
        err.model_file = Some(Some("test.cto".to_string()));
        assert_eq!(
            err.final_message(),
            "Invalid property name '1e+308' File 'test.cto': line 3 column 5, to line 3 column 30. "
        );
    }

    // accordproject/concerto-rust#219 (P5-05 stage-2 T2c): a `name` whose
    // *stringified* form still looks like a valid identifier (`false` ->
    // `"false"`) passes the identifier check, but its own raw JS falsiness
    // fails TS's second, separate `if (!this.name)` check, which raises a
    // plain `Error`, not an `IllegalModelException`.
    #[test]
    fn process_rejects_a_falsy_name_that_stringifies_to_a_valid_identifier() {
        let ast = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": false,
            "isArray": false,
            "isOptional": false
        });
        let err = process::<Error>(&ast).unwrap_err();
        assert!(
            err.to_string().contains("No name for type"),
            "unexpected error: {err}"
        );
    }

    // accordproject/concerto-rust#217 (T2a): `ID_REGEX.test(name)` in TS
    // coerces a non-string `name` with `ToString` rather than rejecting it,
    // so a fuzz-mutated `name` that isn't a JSON string but stringifies to
    // a valid identifier, and is itself JS-truthy, is accepted by TS and
    // must be accepted here too. Minimised repro:
    // `declarations[0].properties[0].name = true`
    // (conformance/ModelManager.addModelFile/16267c5478a5f2840469e147.json,
    // stage2/triage-clusters.json).
    #[test]
    fn process_accepts_a_boolean_name_like_ts_string_coercion() {
        let processed = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": true,
            "isArray": false,
            "isOptional": false
        }))
        .expect("TS: ID_REGEX.test(true) tests \"true\", which matches, and true is truthy");
        assert_eq!(processed.name, "true");
    }

    // Same `ID_REGEX.test` coercion theme, but an absent or `null` `name`
    // stringifies to an identifier-shaped string ("undefined"/"null") that
    // passes the *first* check, then fails TS's second, separate
    // `if (!this.name)` raw-truthiness check (property.ts:
    // `this.name = this.ast.name` is a plain, uncoerced assignment) —
    // `undefined` and `null` are both JS-falsy, so TS throws `Error('No name
    // for type ...')` for both, same as an explicit `false` (the
    // `process_rejects_a_falsy_name_that_stringifies_to_a_valid_identifier`
    // test above). A prior version of this test wrongly asserted these two
    // shapes were *accepted*, conflating "passes the identifier regex" with
    // "has a name" (accordproject/concerto-rust#219 review, merge of #217
    // and #219's overlapping work on this function).
    #[test]
    fn process_rejects_a_missing_name_that_stringifies_to_a_valid_identifier() {
        let err = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "isArray": false,
            "isOptional": false
        }))
        .unwrap_err();
        assert!(
            err.to_string().contains("No name for type"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn process_rejects_a_null_name_that_stringifies_to_a_valid_identifier() {
        let err = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": null,
            "isArray": false,
            "isOptional": false
        }))
        .unwrap_err();
        assert!(
            err.to_string().contains("No name for type"),
            "unexpected error: {err}"
        );
    }

    /// A `RelationshipProperty` node named `name` whose `type` is `ty`
    /// (`None`: no `type` key at all).
    fn relationship(name: &str, ty: Option<Value>) -> Value {
        let mut node = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
            "name": name,
            "isArray": false,
            "isOptional": false,
            "location": {
                "$class": "concerto.metamodel@1.0.0.Range",
                "start": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 107, "line": 5, "column": 3 },
                "end": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 129, "line": 6, "column": 1 }
            }
        });
        if let Some(ty) = ty {
            node["type"] = ty;
        }
        node
    }

    fn contract(err: Error) -> ContractError {
        match err.into_ported() {
            Some(contract) => contract,
            other => panic!("expected a contract error, got {other:?}"),
        }
    }

    /// DV-017 (#218): TS throws `TypeError: Cannot read properties of
    /// undefined|null (reading 'name')` from `Property.process` for a
    /// `RelationshipProperty` with a missing or `null` `type`; Rust raises
    /// an `IllegalModelException` instead, through `propertyProcess` (the
    /// rust-mode view), with the node's location and the model file to be
    /// filled in by the shim.
    #[test]
    fn process_rejects_a_relationship_with_a_missing_or_null_type() {
        for ty in [None, Some(Value::Null)] {
            let ast = relationship("managerId", ty.clone());
            let err = contract(process::<Error>(&ast).unwrap_err());
            assert_eq!(err.kind, ErrorKind::IllegalModel, "{ty:?}");
            assert_eq!(err.code, "property-process-relationshipnotype");
            assert_eq!(err.message(), "Relationship managerId must have a type");
            assert_eq!(err.location, ast.get("location").cloned());
            assert_eq!(err.model_file, Some(None));
            assert_eq!(
                err.final_message(),
                "Relationship managerId must have a type Line 5 column 3, to line 6 column 1. "
            );
        }
    }

    /// Only a missing or `null` `type` crashes TS: any other value's `.name`
    /// is just `undefined`, which `RelationshipDeclaration.validate` rejects
    /// later ("Relationship must have a type"), so `process` still accepts
    /// it with no type, as before. `ObjectProperty` has TS's own guard.
    #[test]
    fn process_keeps_other_typeless_relationships_and_object_properties() {
        for ty in [
            serde_json::json!({}),
            serde_json::json!("x"),
            serde_json::json!(0),
        ] {
            let processed = process::<Error>(&relationship("home", Some(ty))).unwrap();
            assert_eq!(processed.property_type, None);
            assert!(processed.type_set);
        }
        let processed = process::<Error>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty",
            "name": "address",
            "type": null
        }))
        .unwrap();
        assert_eq!(processed.property_type, None);
    }

    /// TS checks the name before its `$class` switch, so an invalid name is
    /// still what a typeless relationship reports first.
    #[test]
    fn process_reports_an_invalid_name_before_a_missing_relationship_type() {
        let err = contract(process::<Error>(&relationship("1bad", None)).unwrap_err());
        assert_eq!(err.code, "property-process-invalidname");
    }

    /// The native construction path (`Property::try_from`) raises the same
    /// DV-017 error, ahead of the serde step that used to reject it with
    /// "invalid RelationshipProperty: missing field `type`".
    #[test]
    fn try_from_rejects_a_relationship_with_a_missing_or_null_type() {
        for ty in [None, Some(Value::Null)] {
            let ast = relationship("dept", ty);
            let err = contract(Property::try_from(&ast).unwrap_err());
            assert_eq!(err.kind, ErrorKind::IllegalModel);
            assert_eq!(err.code, "property-process-relationshipnotype");
            assert_eq!(err.message(), "Relationship dept must have a type");
            assert_eq!(err.location, ast.get("location").cloned());
            // Left for the declaration loader to attach the file name.
            assert_eq!(err.model_file, None);
        }
        let err = contract(Property::try_from(&relationship("1bad", None)).unwrap_err());
        assert_eq!(err.code, "property-process-invalidname");
    }

    fn prop(json: serde_json::Value) -> Property {
        Property::try_from(&json).expect("valid property")
    }

    #[test]
    fn parses_string_property_with_validators() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "email",
            "isArray": false,
            "isOptional": true,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
                "pattern": ".*@.*",
                "flags": ""
            }
        }));
        assert_eq!(p.name(), "email");
        assert!(p.is_optional());
        assert!(!p.is_array());
        assert!(p.is_primitive());
        assert_eq!(p.type_name(), Some("String"));
        match &p {
            Property::String(s) => assert!(s.validator.is_some()),
            _ => panic!("expected String"),
        }
    }

    #[test]
    fn parses_object_and_relationship_type_refs() {
        let o = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty",
            "name": "address",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Address" }
        }));
        assert!(!o.is_primitive());
        assert_eq!(o.type_name(), Some("Address"));
        assert_eq!(
            o.type_identifier().map(|t| t.name.as_str()),
            Some("Address")
        );

        let r = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
            "name": "owner",
            "isArray": true,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" }
        }));
        assert!(r.is_relationship());
        assert!(r.is_array());
        assert_eq!(r.type_name(), Some("Person"));
    }

    #[test]
    fn enum_member_has_no_type() {
        let e = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.EnumProperty",
            "name": "RED"
        }));
        assert!(e.is_enum_value());
        assert_eq!(e.type_name(), None);
        assert!(!e.is_array());
        assert!(!e.is_optional());
    }

    #[test]
    fn unknown_property_kind_errors() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryProperty",
            "name": "x"
        }));
        assert!(err.is_err());
    }

    #[test]
    fn missing_class_is_rejected() {
        let err = Property::try_from(&serde_json::json!({ "name": "x" }));
        assert!(err.unwrap_err().to_string().contains("$class"));
    }

    /// A `Double` property carrying the given range validator.
    fn ranged(lower: Option<f64>, upper: Option<f64>) -> serde_json::Value {
        let mut validator = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.DoubleDomainValidator"
        });
        if let Some(lower) = lower {
            validator["lower"] = lower.into();
        }
        if let Some(upper) = upper {
            validator["upper"] = upper.into();
        }
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.DoubleProperty",
            "name": "value", "isArray": false, "isOptional": false,
            "validator": validator
        })
    }

    /// A `String` property carrying the given length validator.
    fn sized(min: Option<i32>, max: Option<i32>) -> serde_json::Value {
        let mut validator = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringLengthValidator"
        });
        if let Some(min) = min {
            validator["minLength"] = min.into();
        }
        if let Some(max) = max {
            validator["maxLength"] = max.into();
        }
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "text", "isArray": false, "isOptional": false,
            "lengthValidator": validator
        })
    }

    #[test]
    fn a_property_name_must_be_an_identifier() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "1bad", "isArray": false, "isOptional": false
        }));
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("Invalid property name '1bad'")
        );
    }

    /// A `String` property carrying the given regex validator.
    fn matching(pattern: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "text", "isArray": false, "isOptional": false,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
                "pattern": pattern, "flags": ""
            }
        })
    }

    #[test]
    fn a_regex_validator_must_compile() {
        assert!(Property::try_from(&matching(r"^.+@.+\..+$")).is_ok());
        for pattern in ["*invalid", "[unclosed", "(unclosed"] {
            let p = Property::try_from(&matching(pattern)).expect("construction accepts it");
            let err = p.check_bound_validators("test@1.0.0.Box", None);
            assert!(
                err.unwrap_err().to_string().contains("regular expression"),
                "{pattern} should be rejected"
            );
        }
    }

    #[test]
    fn range_lower_above_upper_is_rejected() {
        let p =
            Property::try_from(&ranged(Some(10.0), Some(5.0))).expect("construction accepts it");
        let err = p.check_bound_validators("test@1.0.0.Box", None);
        assert!(err.unwrap_err().to_string().contains("Lower bound"));
    }

    #[test]
    fn range_with_one_open_end_is_accepted() {
        assert!(Property::try_from(&ranged(Some(1.0), None)).is_ok());
        assert!(Property::try_from(&ranged(None, Some(1.0))).is_ok());
        assert!(Property::try_from(&ranged(Some(1.0), Some(10.0))).is_ok());
    }

    #[test]
    fn range_without_either_bound_is_rejected() {
        let p = Property::try_from(&ranged(None, None)).expect("construction accepts it");
        let err = p.check_bound_validators("test@1.0.0.Box", None);
        assert!(err.unwrap_err().to_string().contains("lower and-or upper"));
    }

    /// OD-3: an Integer domain bound that overflows `i32` loads and
    /// validates, matching TS (which reads it as a plain JS number).
    ///
    /// Checked against the frozen TS 5.0.0 reference (`migration/oracle/reference`
    /// in the `/home/user/concerto` workspace): `ModelManager.fromAst` loading
    /// the same `IntegerDomainValidator` AST returns a `NumberValidator` whose
    /// `upperBound` is `2147483648`, matching `upper` here.
    #[test]
    fn integer_domain_bound_above_i32_max_loads() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.IntegerProperty",
            "name": "value", "isArray": false, "isOptional": false,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
                "lower": 0,
                "upper": (i32::MAX as i64) + 1
            }
        }));
        match &p {
            Property::Integer(i) => {
                assert_eq!(
                    i.validator.as_ref().unwrap().upper,
                    Some((i32::MAX as f64) + 1.0)
                );
            }
            _ => panic!("expected Integer"),
        }
    }

    /// OD-3: a Long domain bound above `i64::MAX` loads, as JS rounds it to
    /// the nearest f64 and TS accepts it.
    ///
    /// Checked against the frozen TS 5.0.0 reference (`migration/oracle/reference`
    /// in the `/home/user/concerto` workspace): `ModelManager.fromAst` loading
    /// the same `LongDomainValidator` AST returns a `NumberValidator` whose
    /// `upperBound` is `10000000000000000000` (`1e19`), matching `upper` here.
    #[test]
    fn long_domain_bound_above_i64_max_loads() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.LongProperty",
            "name": "value", "isArray": false, "isOptional": false,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.LongDomainValidator",
                "lower": 0,
                "upper": 1e19
            }
        }));
        match &p {
            Property::Long(l) => {
                assert_eq!(l.validator.as_ref().unwrap().upper, Some(1e19));
            }
            _ => panic!("expected Long"),
        }
    }

    #[test]
    fn negative_string_length_is_rejected() {
        let p = Property::try_from(&sized(Some(-1), Some(5))).expect("construction accepts it");
        let err = p.check_bound_validators("test@1.0.0.Box", None);
        assert!(err.unwrap_err().to_string().contains("positive integers"));
    }

    #[test]
    fn string_length_min_above_max_is_rejected() {
        let p = Property::try_from(&sized(Some(10), Some(5))).expect("construction accepts it");
        let err = p.check_bound_validators("test@1.0.0.Box", None);
        assert!(err.unwrap_err().to_string().contains("minLength"));
    }

    /// accordproject/concerto-rust#219 (P5-05 stage-2 T2c): the fuzz-triage
    /// minimised repro `15270a3d46ae76b3adf549eb` — a `lengthValidator` with
    /// `minLength: "__proto__"` and `maxLength: [10]`. TS's own
    /// `this.minLength > this.maxLength` compares these two *raw* AST values
    /// with JS's untyped `>`: `ToPrimitive([10])` is the string `"10"`, and
    /// since both sides are then strings, JS compares them lexicographically
    /// (`"__proto__" > "10"` is `true`, `'_'`'s code point exceeding
    /// `'1'`'s), so TS rejects the model. Coercing each bound to a number
    /// first (`ToNumber("__proto__")` is `NaN`) makes the comparison always
    /// false, so Rust used to wrongly accept this model — the raw AST
    /// comparison this test pins fixes that.
    #[test]
    fn string_length_min_above_max_by_raw_string_comparison_is_rejected() {
        let raw = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "text", "isArray": false, "isOptional": false,
            "lengthValidator": {
                "$class": "concerto.metamodel@1.0.0.StringLengthValidator",
                "minLength": "__proto__",
                "maxLength": [10]
            }
        });
        let p = Property::try_from(&raw).expect("construction accepts it (a two-phase load)");
        let err = p
            .check_bound_validators("test@1.0.0.Box", Some(&raw))
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("minLength must be less than or equal to maxLength")
        );
    }

    /// The same fixture, but checked with no raw AST (as every call site but
    /// `ClassDeclaration::from_json` passes): without the raw comparison,
    /// each bound coerces to `NaN` and the order check never fires — the
    /// pre-existing, still-correct behaviour for already-validated data.
    #[test]
    fn string_length_min_above_max_by_raw_string_comparison_is_accepted_without_raw_ast() {
        let raw = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "text", "isArray": false, "isOptional": false,
            "lengthValidator": {
                "$class": "concerto.metamodel@1.0.0.StringLengthValidator",
                "minLength": "__proto__",
                "maxLength": [10]
            }
        });
        let p = Property::try_from(&raw).expect("construction accepts it (a two-phase load)");
        assert!(p.check_bound_validators("test@1.0.0.Box", None).is_ok());
    }

    #[test]
    fn string_length_within_bounds_is_accepted() {
        assert!(Property::try_from(&sized(Some(1), Some(5))).is_ok());
        assert!(Property::try_from(&sized(None, Some(5))).is_ok());
    }

    /// A `String[]` property with a collection size validator.
    fn collection_sized(is_array: bool, min: Option<i32>, max: Option<i32>) -> serde_json::Value {
        let mut validator = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator"
        });
        if let Some(min) = min {
            validator["minSize"] = min.into();
        }
        if let Some(max) = max {
            validator["maxSize"] = max.into();
        }
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "tags", "isArray": is_array, "isOptional": false,
            "sizeValidator": validator
        })
    }

    #[test]
    fn size_validator_on_array_is_accepted() {
        assert!(Property::try_from(&collection_sized(true, Some(1), Some(10))).is_ok());
        assert!(Property::try_from(&collection_sized(true, Some(2), None)).is_ok());
        assert!(Property::try_from(&collection_sized(true, None, Some(5))).is_ok());
    }

    /// TS's `Property` constructor accepts a size validator on a non-array
    /// property; only `Property.validate` rejects it (property.ts), which
    /// `crate::validation`'s tests cover. (P2-08 review: this test used to
    /// assert that construction itself failed.)
    #[test]
    fn size_validator_on_non_array_is_accepted_at_construction() {
        let p = Property::try_from(&collection_sized(false, Some(1), Some(5)))
            .expect("construction accepts a size validator on a non-array property");
        assert!(p.size_validator().is_some());
    }

    #[test]
    fn size_validator_min_above_max_is_rejected() {
        let p = Property::try_from(&collection_sized(true, Some(10), Some(2)))
            .expect("construction accepts it");
        let err = p.check_bound_validators("test@1.0.0.Box", None);
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("minSize must be less than or equal to maxSize")
        );
    }

    /// accordproject/concerto-rust#219 (P5-05 stage-2 T2c): the fuzz-triage
    /// minimised repro `7b9fc1eca8208827732709eb` — a `sizeValidator` with
    /// `minSize: "aaaa…"` and `maxSize: [1]`. As
    /// [`string_length_min_above_max_by_raw_string_comparison_is_rejected`]'s
    /// doc comment explains for `lengthValidator`: `ToPrimitive([1])` is the
    /// string `"1"`, so TS's raw `this.minSize > this.maxSize` becomes a
    /// string comparison (`"aaaa…" > "1"` is `true`), not the always-false
    /// `NaN` comparison converting each side to a number first would give.
    #[test]
    fn size_validator_min_above_max_by_raw_string_comparison_is_rejected() {
        let raw = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "tags", "isArray": true, "isOptional": false,
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": "aaaaaaaaaaaaaaaaaaaaa",
                "maxSize": [1]
            }
        });
        let p = Property::try_from(&raw).expect("construction accepts it (a two-phase load)");
        let err = p
            .check_bound_validators("test@1.0.0.Box", Some(&raw))
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("minSize must be less than or equal to maxSize")
        );
    }

    /// The same fixture, but checked with no raw AST (as every call site but
    /// `ClassDeclaration::from_json` passes): without the raw comparison,
    /// each bound coerces to `NaN` and the order check never fires — the
    /// pre-existing, still-correct behaviour for already-validated data.
    #[test]
    fn size_validator_min_above_max_by_raw_string_comparison_is_accepted_without_raw_ast() {
        let raw = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "tags", "isArray": true, "isOptional": false,
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": "aaaaaaaaaaaaaaaaaaaaa",
                "maxSize": [1]
            }
        });
        let p = Property::try_from(&raw).expect("construction accepts it (a two-phase load)");
        assert!(p.check_bound_validators("test@1.0.0.Box", None).is_ok());
    }

    #[test]
    fn size_validator_negative_bounds_rejected() {
        let p = Property::try_from(&collection_sized(true, Some(-1), Some(5)))
            .expect("construction accepts it");
        let err = p.check_bound_validators("test@1.0.0.Box", None);
        assert!(err.unwrap_err().to_string().contains("positive integers"));
    }

    #[test]
    fn size_validator_on_object_property_without_array_is_allowed() {
        let json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty",
            "name": "contacts",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "PhoneBook" },
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": 1,
                "maxSize": 5
            }
        });
        assert!(Property::try_from(&json).is_ok());
    }

    #[test]
    fn size_validator_on_relationship_array_is_accepted() {
        let json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
            "name": "advisors",
            "isArray": true,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": 1,
                "maxSize": 3
            }
        });
        let p = Property::try_from(&json).unwrap();
        assert!(p.size_validator().is_some());
        assert_eq!(p.size_validator().unwrap().min_size, Some(1.0));
        assert_eq!(p.size_validator().unwrap().max_size, Some(3.0));
    }

    /// Construction accepts it (TS `Property` constructor); validation
    /// rejects it (`crate::validation`'s tests). P2-08 review: this test
    /// used to assert construction-time rejection.
    #[test]
    fn size_validator_on_non_array_relationship_is_accepted_at_construction() {
        let json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
            "name": "owner",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": 1
            }
        });
        let p = Property::try_from(&json)
            .expect("construction accepts a size validator on a non-array relationship");
        assert!(p.size_validator().is_some());
    }

    #[test]
    fn unknown_property_kind_is_reported_by_name() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryProperty",
            "name": "x"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Unrecognised model element \"concerto.metamodel@1.0.0.MysteryProperty\"."
        );
    }

    #[test]
    fn missing_class_is_reported_verbatim() {
        let err = Property::try_from(&serde_json::json!({ "name": "x" }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: property node is missing its $class"
        );
    }

    #[test]
    fn only_the_full_metamodel_property_classes_are_recognised() {
        // TS `ClassDeclaration.process` matches each property's `$class`
        // with `===` against the full metamodel classes; a short name,
        // another namespace's, or text that merely ends in a property
        // class's short name is "Unrecognised model element"
        // (accordproject/concerto-rust#285, BC-25).
        for class in [
            "StringProperty",
            "foo.StringProperty",
            "concerto.metamodel@1.0.0.StringPropertyconcerto.metamodel@1.0.0.StringProperty",
            "concerto.metamodel@1.0.0.EnumPropertyconcerto.metamodel@1.0.0.EnumProperty",
            "concerto.metamodel@1.0.0.RelationshipPropertyconcerto.metamodel@1.0.0.RelationshipProperty",
            "concerto.metamodel@2.0.0.StringProperty",
            "concerto.metamodel@1.0.0.Foo.StringProperty",
        ] {
            let err = Property::try_from(&serde_json::json!({
                "$class": class,
                "name": "email",
                "isArray": false,
                "isOptional": false,
                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "T" }
            }))
            .unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("Unrecognised model element \"{class}\"."),
                "{class}"
            );
            assert!(matches!(
                err.ported(),
                Some(c) if c.kind == ErrorKind::IllegalModel
            ));
        }
        for kind in PROPERTY_KINDS {
            assert_eq!(
                property_kind(&format!("concerto.metamodel@1.0.0.{kind}")),
                Some(kind)
            );
        }
    }

    #[test]
    fn an_unrecognised_property_class_is_reported_before_the_property_checks() {
        // TS: the `$class` match comes before `Property.process`, so it wins
        // over a null decorator (DV-018), a missing name (a plain `Error`)
        // and an invalid name.
        for extra in [
            serde_json::json!({ "name": "a", "decorators": [null] }),
            serde_json::json!({}),
            serde_json::json!({ "name": "1bad" }),
        ] {
            let mut ast = serde_json::json!({ "$class": "foo.StringProperty" });
            for (key, value) in extra.as_object().unwrap() {
                ast[key] = value.clone();
            }
            let err = Property::try_from(&ast).unwrap_err();
            assert_eq!(
                err.to_string(),
                "Unrecognised model element \"foo.StringProperty\".",
                "{ast}"
            );
        }
    }

    #[test]
    fn process_matches_the_full_property_class() {
        // TS `Property.process`'s `switch (this.ast.$class)` has no arm for
        // a class that only ends in a property class's short name, so
        // `this.type` is left unassigned, and a `RelationshipProperty`
        // short name with no `type` does not reach its unguarded arm.
        for class in ["StringProperty", "foo.StringProperty"] {
            let processed = process::<ContractError>(&serde_json::json!({
                "$class": class, "name": "s"
            }))
            .unwrap();
            assert_eq!(processed.property_type, None, "{class}");
            assert!(!processed.type_set, "{class}");
        }
        assert!(
            process::<ContractError>(&serde_json::json!({
                "$class": "RelationshipProperty", "name": "r"
            }))
            .is_ok()
        );
    }

    #[test]
    fn a_reserved_name_is_rejected_before_the_kind_is_checked() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryProperty",
            "name": "$identifier"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: Invalid field name '$identifier'"
        );
    }

    /// P2-04 (plan §1.2's "enum ... reserved values" gap; issue #48): an
    /// enum value may not take a reserved (system) property name either,
    /// the same check every other property kind gets above.
    ///
    /// Checked against the frozen TS 5.0.0 reference
    /// (`migration/oracle/reference`): `ModelManager.addCTOModel` on
    ///
    /// ```cto
    /// namespace org.acme.enumreserved@1.0.0
    /// enum Status {
    ///   o $identifier
    /// }
    /// ```
    ///
    /// raises `IllegalModelException: Invalid field name '$identifier'`,
    /// matching this test verbatim.
    #[test]
    fn a_reserved_name_is_rejected_on_an_enum_value() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.EnumProperty",
            "name": "$identifier"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: Invalid field name '$identifier'"
        );
    }

    #[test]
    fn a_malformed_property_is_reported_under_its_own_kind() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "s",
            "isArray": "yes"
        }));
        assert!(
            err.unwrap_err()
                .to_string()
                .starts_with("illegal model: invalid StringProperty: ")
        );
    }

    /// Ported from `test/introspect/property.js` #getSizeValidator "should
    /// return null when no size validator".
    #[test]
    fn size_validator_is_none_when_absent() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "tags", "isArray": true, "isOptional": false
        }));
        assert!(p.size_validator().is_none());
    }

    /// Ported from `test/introspect/field.js` #constructor "should not have a
    /// default value by default" and "should save the incoming default
    /// value". TS builds a `Field` over a stubbed `ClassDeclaration` parent
    /// for these two, but `process()` never calls it (`this.ast.defaultValue`
    /// only), so the stub is inert scaffolding, not white-box coupling
    /// (module doc on [`crate::model_manager::ModelManager::property_default_value`],
    /// which is the same raw-AST read for a `PropId` already in the arena);
    /// `Property::try_from` alone is the faithful port here, no `ModelManager`
    /// or parent needed.
    #[test]
    fn a_default_value_is_read_from_the_ast_when_present() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "field", "isArray": false, "isOptional": false,
            "defaultValue": "wowSuchDefault"
        }));
        match &p {
            Property::String(s) => {
                assert_eq!(s.default_value.as_deref(), Some("wowSuchDefault"));
            }
            _ => panic!("expected String"),
        }

        let without = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "field", "isArray": false, "isOptional": false
        }));
        match &without {
            Property::String(s) => assert_eq!(s.default_value, None),
            _ => panic!("expected String"),
        }
    }

    /// Ported from `test/introspect/field.js` #getDefaultValue "should return
    /// the default value for falsy defaults": a JSON `false` default is not
    /// itself nullish, so it is kept (`Util.isNull` in TS, `!v.is_null()` in
    /// [`crate::model_manager::ModelManager::property_default_value`]),
    /// unlike a JSON `null`.
    #[test]
    fn a_falsy_boolean_default_value_is_not_treated_as_absent() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.BooleanProperty",
            "name": "field", "isArray": false, "isOptional": false,
            "defaultValue": false
        }));
        match &p {
            Property::Boolean(b) => assert_eq!(b.default_value, Some(false)),
            _ => panic!("expected Boolean"),
        }
    }

    /// Ported from `test/introspect/field.js` #constructor "should not be
    /// optional by default" and "should detect if field is optional".
    #[test]
    fn optional_defaults_to_false_and_follows_the_ast() {
        let not_optional = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "field", "isArray": false
        }));
        assert!(!not_optional.is_optional());

        let optional = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "field", "isArray": false, "isOptional": true
        }));
        assert!(optional.is_optional());
    }

    /// accordproject/concerto-rust#217, cluster 1/3: a fuzz-mutated
    /// `sizeValidator.minSize`/`maxSize` that is not a JSON number (here, a
    /// non-numeric string) must not fail the whole property's parse the way
    /// `serde`'s strict struct decode otherwise would — TS's own
    /// `CollectionSizeValidator` constructor reads them with no type check
    /// at all, and coerces through `ToNumber` only when it later compares
    /// them (`ecma::to_number`'s own doc comment): `"NaN" < 0` is `false`
    /// (like every other comparison against a non-numeric coercion), so
    /// this loads with no bound enforced, not an error.
    #[test]
    fn size_validator_with_a_non_numeric_bound_loads_instead_of_failing_to_parse() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "items", "isArray": true, "isOptional": false,
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionDomainValidator",
                "minSize": "NaN", "maxSize": 5
            }
        }));
        assert_eq!(p.size_validator().unwrap().max_size, Some(5.0));
        assert!(p.size_validator().unwrap().min_size.unwrap().is_nan());
    }

    /// accordproject/concerto-rust#217: a `sizeValidator`/`lengthValidator`/
    /// `validator` sub-object's own `$class`, when it is not a JSON string
    /// (here, an array), must not fail the parse either — nothing in this
    /// crate ever reads that field back (`validators::size_validator_from_ast`'s
    /// own doc comment).
    #[test]
    fn size_validator_class_that_is_not_a_string_loads_instead_of_failing_to_parse() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "items", "isArray": true, "isOptional": false,
            "sizeValidator": {
                "$class": ["concerto.metamodel@1.0.0.CollectionDomainValidator"],
                "minSize": 1, "maxSize": 5
            }
        }));
        assert_eq!(p.size_validator().unwrap().min_size, Some(1.0));
    }

    /// accordproject/concerto-rust#217, cluster 4/44: a `lengthValidator`
    /// AST replaced wholesale by a wrongly-shaped, but still truthy, JSON
    /// value (here, an array) must load with no bound enforced, matching
    /// TS's own outcome: `this.ast.lengthValidator` is truthy, so
    /// `StringValidator`'s constructor runs, but `lengthValidator.minLength`/
    /// `.maxLength` on a non-object are both `undefined`, which never trips
    /// the "must be specified" check (that check is a strict `=== null`
    /// identity, not a truthiness test — `validators::length_bound_field`'s
    /// own doc comment).
    #[test]
    fn string_property_with_a_non_object_length_validator_loads() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "s", "isArray": false, "isOptional": false,
            "lengthValidator": [{
                "$class": "concerto.metamodel@1.0.0.StringLengthValidator",
                "minLength": null, "maxLength": 10
            }]
        }));
        match &p {
            Property::String(s) => assert!(s.length_validator.is_some()),
            _ => panic!("expected String"),
        }
    }

    /// The corner `string_property_with_a_non_object_length_validator_loads`
    /// must still catch: a *well-formed* `lengthValidator` whose bounds are
    /// both explicitly `null` is the one shape that does still trip the
    /// "must be specified" check, in both engines.
    #[test]
    fn string_property_with_both_length_bounds_explicitly_null_is_rejected() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "s", "isArray": false, "isOptional": false,
            "lengthValidator": {
                "$class": "concerto.metamodel@1.0.0.StringLengthValidator",
                "minLength": null, "maxLength": null
            }
        }))
        .expect("try_from itself does not build the validator")
        .check_bound_validators("ns.C", None)
        .unwrap_err();
        assert!(err.to_string().contains("must be specified"));
    }

    /// accordproject/concerto-rust#217, cluster 6: a `validator.pattern`
    /// that is not a JSON string (here, a number) must coerce through
    /// `ToString`, matching `new RegExp(validator.pattern, ...)`, not fail
    /// the parse.
    #[test]
    fn regex_validator_with_a_non_string_pattern_loads_instead_of_failing_to_parse() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "s", "isArray": false, "isOptional": false,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
                "pattern": 5, "flags": ""
            }
        }));
        match &p {
            Property::String(s) => assert_eq!(s.validator.as_ref().unwrap().pattern, "5"),
            _ => panic!("expected String"),
        }
    }
}
