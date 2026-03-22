use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

// ===================================================================
// MapKeyTypeDecl
// ===================================================================

#[derive(Debug, Clone)]
pub enum MapKeyTypeDecl {
    String(mm::StringMapKeyType),
    DateTime(mm::DateTimeMapKeyType),
    Object(mm::ObjectMapKeyType),
}

impl MapKeyTypeDecl {
    pub fn decorators(&self) -> &[mm::Decorator] {
        match self {
            Self::String(k) => k.decorators.as_deref().unwrap_or(&[]),
            Self::DateTime(k) => k.decorators.as_deref().unwrap_or(&[]),
            Self::Object(k) => k.decorators.as_deref().unwrap_or(&[]),
        }
    }

    pub fn type_name(&self) -> &str {
        match self {
            Self::String(_) => "String",
            Self::DateTime(_) => "DateTime",
            Self::Object(k) => &k.type_.name,
        }
    }
}

// ===================================================================
// MapValueTypeDecl
// ===================================================================

#[derive(Debug, Clone)]
pub enum MapValueTypeDecl {
    Boolean(mm::BooleanMapValueType),
    DateTime(mm::DateTimeMapValueType),
    String(mm::StringMapValueType),
    Integer(mm::IntegerMapValueType),
    Long(mm::LongMapValueType),
    Double(mm::DoubleMapValueType),
    Object(mm::ObjectMapValueType),
    Relationship(mm::RelationshipMapValueType),
}

impl MapValueTypeDecl {
    pub fn decorators(&self) -> &[mm::Decorator] {
        match self {
            Self::Boolean(v) => v.decorators.as_deref().unwrap_or(&[]),
            Self::DateTime(v) => v.decorators.as_deref().unwrap_or(&[]),
            Self::String(v) => v.decorators.as_deref().unwrap_or(&[]),
            Self::Integer(v) => v.decorators.as_deref().unwrap_or(&[]),
            Self::Long(v) => v.decorators.as_deref().unwrap_or(&[]),
            Self::Double(v) => v.decorators.as_deref().unwrap_or(&[]),
            Self::Object(v) => v.decorators.as_deref().unwrap_or(&[]),
            Self::Relationship(v) => v.decorators.as_deref().unwrap_or(&[]),
        }
    }

    pub fn type_name(&self) -> &str {
        match self {
            Self::Boolean(_) => "Boolean",
            Self::DateTime(_) => "DateTime",
            Self::String(_) => "String",
            Self::Integer(_) => "Integer",
            Self::Long(_) => "Long",
            Self::Double(_) => "Double",
            Self::Object(v) => &v.type_.name,
            Self::Relationship(v) => &v.type_.name,
        }
    }
}
