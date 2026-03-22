use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

/// Sum type for all import types in Concerto.
/// See [Concerto documentation on imports](https://concerto.accordproject.org/docs/design/specification/model-imports).
#[derive(Debug, Clone)]
pub enum ImportDecl {
    /// Corresponds to `import com.example.test.*`.
    /// This is only allowed in non-strict mode.
    All(mm::ImportAll),
    /// Corresponds to importing a single type from
    /// a namespace. `import com.example.test@1.0.0.Foo`
    Type(mm::ImportType),
    /// Corresponds to importing several types from
    /// a namespace. `import com.example.test@1.0.0.{Foo, Bar}`
    Types(mm::ImportTypes),
}

impl ImportDecl {
    pub fn namespace(&self) -> &str {
        match self {
            Self::All(i) => &i.namespace,
            Self::Type(i) => &i.namespace,
            Self::Types(i) => &i.namespace,
        }
    }

    pub fn uri(&self) -> Option<&str> {
        match self {
            Self::All(i) => i.uri.as_deref(),
            Self::Type(i) => i.uri.as_deref(),
            Self::Types(i) => i.uri.as_deref(),
        }
    }

    /// Returns the fully-qualified names introduced by this import.
    pub fn fully_qualified_names(&self) -> Vec<String> {
        match self {
            Self::All(_) => {
                // ImportAll doesn't enumerate specific types;
                // resolution requires the model manager.
                vec![]
            }
            Self::Type(i) => {
                vec![crate::model_util::fully_qualified_name(
                    &i.namespace,
                    &i.name,
                )]
            }
            Self::Types(i) => i
                .types
                .iter()
                .map(|t| crate::model_util::fully_qualified_name(&i.namespace, t))
                .collect(),
        }
    }

}

/// Construct an [`ImportDecl`] from AST JSON by inspecting `$class`.
impl TryFrom<serde_json::Value> for ImportDecl {
    type Error = crate::error::ConcertoError;

    fn try_from(value: serde_json::Value) -> crate::error::Result<Self> {
        let class = value
            .get("$class")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = crate::model_util::get_short_name(&class).to_string();
        let mk_err = |e: serde_json::Error, k: &str| crate::error::ConcertoError::IllegalModel {
            message: format!("Invalid {k}: {e}"),
            file_name: None,
            location: None,
        };

        match kind.as_str() {
            "ImportAll" => Ok(Self::All(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?)),
            "ImportType" => Ok(Self::Type(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?)),
            "ImportTypes" => Ok(Self::Types(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?)),
            _ => Err(crate::error::ConcertoError::IllegalModel {
                message: format!("Unknown import type: {class}"),
                file_name: None,
                location: None,
            }),
        }
    }
}
