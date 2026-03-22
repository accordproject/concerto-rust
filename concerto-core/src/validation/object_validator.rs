use serde_json::Value;

use crate::error::{ConcertoError, Result};
use crate::introspect::declarations::ClassDeclaration;
use crate::introspect::properties::PropertyDecl;
use crate::introspect::traits::HasProperties;
use crate::model_manager::ModelManager;
use crate::model_util;

/// Validates a JSON object (`serde_json::Value`) against a loaded Concerto
/// model, mirroring the JS `ObjectValidator`.
pub struct ObjectValidator<'a> {
    model_manager: &'a ModelManager,
}

impl<'a> ObjectValidator<'a> {
    pub fn new(model_manager: &'a ModelManager) -> Self {
        Self { model_manager }
    }

    /// Validate a JSON object. The object must have a `$class` field.
    pub fn validate(&self, obj: &Value) -> Result<()> {
        let class = obj
            .get("$class")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConcertoError::Validation {
                message: "Object missing '$class' field".into(),
                component: "$class".into(),
            })?;

        let decl = self.model_manager.get_type(class)?;
        let class_decl = decl.as_class().ok_or_else(|| ConcertoError::Validation {
            message: format!("Type '{class}' is not a class declaration"),
            component: class.to_string(),
        })?;

        // Cannot instantiate abstract types
        if class_decl.is_abstract() {
            return Err(ConcertoError::Validation {
                message: format!("Cannot validate instance of abstract type '{class}'"),
                component: class.to_string(),
            });
        }

        let obj_map = obj.as_object().ok_or_else(|| ConcertoError::Validation {
            message: "Expected a JSON object".into(),
            component: class.to_string(),
        })?;

        // Collect all properties including inherited ones
        let all_props = self.collect_all_properties(class_decl)?;

        // Check for undeclared fields
        for key in obj_map.keys() {
            if model_util::is_system_property(key) {
                continue;
            }
            if !all_props.iter().any(|p| p.name() == key.as_str()) {
                return Err(ConcertoError::Validation {
                    message: format!(
                        "Unexpected property '{key}' on instance of '{class}'"
                    ),
                    component: class.to_string(),
                });
            }
        }

        // Check all required fields are present and validate types
        for prop in &all_props {
            let value = obj_map.get(prop.name());

            if !prop.is_optional() && value.is_none() {
                return Err(ConcertoError::Validation {
                    message: format!(
                        "Missing required property '{}' on instance of '{class}'",
                        prop.name()
                    ),
                    component: class.to_string(),
                });
            }

            if let Some(val) = value {
                self.validate_property_value(prop, val, class)?;
            }
        }

        Ok(())
    }

    fn collect_all_properties(
        &self,
        cd: &'a ClassDeclaration,
    ) -> Result<Vec<&'a PropertyDecl>> {
        let mut props: Vec<&'a PropertyDecl> = Vec::new();

        // Collect from super-type chain first (inherited properties)
        if let Some(super_type_id) = cd.super_type() {
            let super_fqn = if let Some(ref ns) = super_type_id.namespace {
                model_util::fully_qualified_name(ns, &super_type_id.name)
            } else if let Some(ref resolved) = super_type_id.resolved_name {
                resolved.clone()
            } else {
                super_type_id.name.clone()
            };

            if let Ok(super_decl) = self.model_manager.get_type(&super_fqn) {
                if let Some(super_cd) = super_decl.as_class() {
                    let inherited = self.collect_all_properties(super_cd)?;
                    props.extend(inherited);
                }
            }
        }

        // Add own properties
        props.extend(cd.own_properties().iter());

        Ok(props)
    }

    fn validate_property_value(
        &self,
        prop: &PropertyDecl,
        value: &Value,
        parent_class: &str,
    ) -> Result<()> {
        if prop.is_array() {
            let arr = value.as_array().ok_or_else(|| ConcertoError::Validation {
                message: format!(
                    "Property '{}' should be an array on '{parent_class}'",
                    prop.name()
                ),
                component: parent_class.to_string(),
            })?;
            for item in arr {
                self.validate_single_value(prop, item, parent_class)?;
            }
        } else {
            self.validate_single_value(prop, value, parent_class)?;
        }
        Ok(())
    }

    fn validate_single_value(
        &self,
        prop: &PropertyDecl,
        value: &Value,
        parent_class: &str,
    ) -> Result<()> {
        match prop {
            PropertyDecl::Boolean(_) => {
                if !value.is_boolean() {
                    return Err(ConcertoError::Validation {
                        message: format!(
                            "Property '{}' should be Boolean on '{parent_class}'",
                            prop.name()
                        ),
                        component: parent_class.to_string(),
                    });
                }
            }
            PropertyDecl::String(_) => {
                if !value.is_string() {
                    return Err(ConcertoError::Validation {
                        message: format!(
                            "Property '{}' should be String on '{parent_class}'",
                            prop.name()
                        ),
                        component: parent_class.to_string(),
                    });
                }
            }
            PropertyDecl::Integer(_) | PropertyDecl::Long(_) => {
                if !value.is_i64() && !value.is_u64() {
                    return Err(ConcertoError::Validation {
                        message: format!(
                            "Property '{}' should be an integer on '{parent_class}'",
                            prop.name()
                        ),
                        component: parent_class.to_string(),
                    });
                }
            }
            PropertyDecl::Double(_) => {
                if !value.is_number() {
                    return Err(ConcertoError::Validation {
                        message: format!(
                            "Property '{}' should be a number on '{parent_class}'",
                            prop.name()
                        ),
                        component: parent_class.to_string(),
                    });
                }
            }
            PropertyDecl::DateTime(_) => {
                // DateTime is serialized as an ISO string
                if !value.is_string() {
                    return Err(ConcertoError::Validation {
                        message: format!(
                            "Property '{}' should be a DateTime string on '{parent_class}'",
                            prop.name()
                        ),
                        component: parent_class.to_string(),
                    });
                }
            }
            PropertyDecl::Object(_) => {
                // For complex object properties, recursively validate
                if value.is_object() {
                    self.validate(value)?;
                } else if !value.is_null() {
                    return Err(ConcertoError::Validation {
                        message: format!(
                            "Property '{}' should be an object on '{parent_class}'",
                            prop.name()
                        ),
                        component: parent_class.to_string(),
                    });
                }
            }
            PropertyDecl::Relationship(_) => {
                // Relationships are serialized as URI strings
                if !value.is_string() {
                    return Err(ConcertoError::Validation {
                        message: format!(
                            "Relationship '{}' should be a string URI on '{parent_class}'",
                            prop.name()
                        ),
                        component: parent_class.to_string(),
                    });
                }
            }
            PropertyDecl::Enum(_) => {
                // Enum values in an object context would be strings
                if !value.is_string() {
                    return Err(ConcertoError::Validation {
                        message: format!(
                            "Enum property '{}' should be a string on '{parent_class}'",
                            prop.name()
                        ),
                        component: parent_class.to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_manager::{ModelManager, ModelManagerOptions};

    fn make_manager_with_person() -> ModelManager {
        let mut mgr = ModelManager::new(ModelManagerOptions::default()).unwrap();
        let model_json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example@1.0.0",
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Person",
                    "isAbstract": false,
                    "properties": [
                        {
                            "$class": "concerto.metamodel@1.0.0.StringProperty",
                            "name": "name",
                            "isArray": false,
                            "isOptional": false
                        },
                        {
                            "$class": "concerto.metamodel@1.0.0.IntegerProperty",
                            "name": "age",
                            "isArray": false,
                            "isOptional": true
                        }
                    ]
                }
            ]
        });
        mgr.add_model(&model_json, None, false).unwrap();
        mgr
    }

    #[test]
    fn test_validate_valid_object() {
        let mgr = make_manager_with_person();
        let validator = ObjectValidator::new(&mgr);

        let obj = serde_json::json!({
            "$class": "org.example@1.0.0.Person",
            "name": "Alice",
            "age": 30
        });

        assert!(validator.validate(&obj).is_ok());
    }

    #[test]
    fn test_validate_missing_required_field() {
        let mgr = make_manager_with_person();
        let validator = ObjectValidator::new(&mgr);

        let obj = serde_json::json!({
            "$class": "org.example@1.0.0.Person",
            "age": 30
        });

        let err = validator.validate(&obj).unwrap_err();
        assert!(matches!(err, ConcertoError::Validation { .. }));
    }

    #[test]
    fn test_validate_undeclared_field() {
        let mgr = make_manager_with_person();
        let validator = ObjectValidator::new(&mgr);

        let obj = serde_json::json!({
            "$class": "org.example@1.0.0.Person",
            "name": "Alice",
            "unknownField": true
        });

        let err = validator.validate(&obj).unwrap_err();
        assert!(matches!(err, ConcertoError::Validation { .. }));
    }

    #[test]
    fn test_validate_wrong_type() {
        let mgr = make_manager_with_person();
        let validator = ObjectValidator::new(&mgr);

        let obj = serde_json::json!({
            "$class": "org.example@1.0.0.Person",
            "name": 123,
            "age": 30
        });

        let err = validator.validate(&obj).unwrap_err();
        assert!(matches!(err, ConcertoError::Validation { .. }));
    }

    #[test]
    fn test_validate_missing_class() {
        let mgr = make_manager_with_person();
        let validator = ObjectValidator::new(&mgr);

        let obj = serde_json::json!({
            "name": "Alice"
        });

        let err = validator.validate(&obj).unwrap_err();
        assert!(matches!(err, ConcertoError::Validation { .. }));
    }

    #[test]
    fn test_validate_abstract_rejected() {
        let mgr = ModelManager::new(ModelManagerOptions::default()).unwrap();
        let validator = ObjectValidator::new(&mgr);

        let obj = serde_json::json!({
            "$class": "concerto@1.0.0.Concept"
        });

        let err = validator.validate(&obj).unwrap_err();
        assert!(matches!(err, ConcertoError::Validation { .. }));
    }
}
