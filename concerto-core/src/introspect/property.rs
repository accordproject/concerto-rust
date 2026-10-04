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
use crate::introspect::decorator::{Decorated, Decorator, WithDecorators};
use crate::introspect::model_file::unreadable_ast;
use crate::introspect::validators;
use crate::introspect::{FullyQualified, METAMODEL_NAMESPACE, Named, Typed};
use crate::model_util::{is_system_property, is_valid_identifier};

/// What `Property.process` computes, after `super.process()`.
///
/// TS: `Property.process` (src/introspect/property.ts). `type_set` says
/// whether TS assigns `this.type` at all: the `EnumProperty` arm leaves it
/// `undefined`, which the WASM view tells apart from the `null` of an
/// `ObjectProperty` with no `type`.
#[cfg(feature = "js-compat")]
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

/// `Property.process`'s fields from the AST, in TS order: the identifier
/// check, the name, the `$class` switch for `type`, `array`, `optional`.
/// The WASM view builds `sizeValidator`. `ID_REGEX.test` applies
/// `ToString` to a non-string `name` (DV-002), so `name: true` is `"true"`.
///
/// TS: `Property.process` (src/introspect/property.ts)
#[cfg(feature = "js-compat")]
pub fn process<E: From<ContractError>>(ast: &Value) -> std::result::Result<ProcessedProperty, E> {
    // TS interpolates the raw `this.ast.name` into a template literal and
    // into `ID_REGEX.test`, both of which apply JS `ToString` to any value
    // (an absent key is `"undefined"`).
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
        // this.ast.location)`: the WASM binding supplies the JS model file;
        // the location comes from this AST node.
        err.location = ast.get("location").cloned();
        err.model_file = Some(None);
        return Err(err.into());
    }
    // TS: `this.name = this.ast.name; if (!this.name) { throw new Error('No
    // name for type ' + JSON.stringify(this.ast)); }`: a second check, on
    // the raw value's truthiness, since `ID_REGEX.test` accepts `false`
    // (stringified to `"false"`).
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
    // (`===`); anything else takes no arm.
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
            // with an `IllegalModelException` instead.
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

/// DV-017: a `RelationshipProperty` whose `type` is missing or `null`, where
/// TS reads `this.ast.type.name` unguarded and V8 throws a `TypeError`.
/// Rust raises `property-process-relationshipnotype` (an
/// `IllegalModelException`) at the property's `location`. `None` for any
/// other node; `name` is the validated property name.
#[cfg(feature = "js-compat")]
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

    /// This property's own AST `location`, if the node carried one, as TS
    /// `Property.validate`/`Decorated.validate` throw with `this.ast.location`.
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
/// one of the nine full metamodel property classes. TS
/// `ClassDeclaration.process` compares `$class` by `===` and throws
/// "Unrecognised model element" for anything else, such as a bare short
/// name or another namespace's (BC-25).
pub(crate) fn property_kind(class: &str) -> Option<&str> {
    let kind = class.strip_prefix(METAMODEL_NAMESPACE)?.strip_prefix('.')?;
    PROPERTY_KINDS.contains(&kind).then_some(kind)
}

impl TryFrom<&serde_json::Value> for Property {
    type Error = Error;

    /// Reads one property node, outside any declaration: the typed read (a
    /// strict one, `crate::introspect::typed_ast`), then the name checks
    /// `ClassDeclaration.process`'s loop and `Property.process` make — a
    /// reserved system name, then a name that is not a valid identifier.
    /// The validators are checked once the owning class is known
    /// (`Property::check_bound_validators`).
    fn try_from(value: &serde_json::Value) -> Result<Self> {
        let property = crate::introspect::typed_ast::property_from_value(value)
            .map_err(|e| unreadable_ast(&e, None))?;
        let name = property.name();
        if is_system_property(name) {
            return Err(Error::illegal_model(
                format!("Invalid field name '{name}'"),
                None,
                None,
            ));
        }
        if !is_valid_identifier(name) {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "property-process-invalidname",
                vec![("name", name.to_string())],
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
/// [`Property::try_from`] (and so the typed read of a class's properties)
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

impl Property {
    /// Checks a collection size validator's own bounds, as TS's
    /// `CollectionSizeValidator` constructor does during `process`. Whether the
    /// property may carry one at all is `Property.validate`'s check, once types
    /// resolve (`check_property_type`), so such a file still constructs.
    fn check_size_validator(
        fqn: &str,
        name: &str,
        validator: Option<&mm::CollectionSizeValidator>,
    ) -> Result<()> {
        let Some(v) = validator else { return Ok(()) };
        let element = BoundElement {
            fqn,
            name,
            default_value: None,
        };
        validators::CollectionSizeValidator::new(&element, v, None)?;
        Ok(())
    }

    /// Whether [`Property::check_bound_validators`] has a validator to
    /// rebuild: a size validator, or a String, Integer, Long or Double
    /// domain (or length) validator.
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
        /// Rebuilds and discards this property's validators to surface the
        /// `IllegalModelException` their constructors raise (BC-39). Called by the
        /// class declaration's loader once the fully qualified name is known, never
        /// from `try_from`, so a property with a bad validator still parses, as in
        /// TS's two-phase load. The size validator comes first, then the domain or
        /// length-and-regex validator, as in `Property.process`/`Field.process`.
        pub fn check_bound_validators(&self, class_fqn: &str) -> Result<()> {
            // A property with no validator to rebuild (most) returns before
            // its fully-qualified name is built; every arm below is then a
            // no-op.
            if !self.has_bound_validators() {
                return Ok(());
            }
            let name = self.name().to_string();
            // TS: `Validator.getFieldOrScalarDeclaration().getFullyQualifiedName()`
            // — a property's own, `<namespace>.<Class>.<property>` (property.ts
            // `getFullyQualifiedName`), not its owning class's.
            let fqn = format!("{class_fqn}.{name}");
            let fqn = fqn.as_str();
            Self::check_size_validator(fqn, &name, self.size_validator())?;
            let element = |default_value: Option<Value>| BoundElement {
                fqn,
                name: &name,
                default_value,
            };
            // The typed bounds, as the strict read gave them.
            let number = |bounds: Option<(Option<f64>, Option<f64>)>, default_value: Option<f64>| {
                if let Some((lower, upper)) = bounds {
                    validators::NumberValidator::from_bounds(
                        &element(default_value.map(Value::from)),
                        lower,
                        upper,
                    )?;
                }
                Ok(())
            };
            match self {
                Self::String(p) if p.validator.is_some() || p.length_validator.is_some() => {
                    let default_value = p.default_value.clone().map(Value::String);
                    validators::StringValidator::new(
                        &element(default_value),
                        p.validator.as_ref(),
                        p.length_validator.as_ref(),
                        None,
                    )?;
                    Ok(())
                }
                Self::Integer(p) => number(p.validator.as_ref().map(|v| (v.lower, v.upper)), p.default_value),
                Self::Long(p) => number(p.validator.as_ref().map(|v| (v.lower, v.upper)), p.default_value),
                Self::Double(p) => number(p.validator.as_ref().map(|v| (v.lower, v.upper)), p.default_value),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests;
