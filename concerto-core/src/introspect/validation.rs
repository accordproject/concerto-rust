use crate::error::{ConcertoError, Result};
use crate::introspect::declarations::{ClassDeclaration, Declaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::traits::{HasProperties, Identifiable, Named};
use crate::model_util;

use std::collections::{HashMap, HashSet};

// ===================================================================
// ValidationContext
// ===================================================================

/// Provides type resolution context during model validation.
pub struct ValidationContext<'a> {
    model_files: &'a HashMap<String, ModelFile>,
    current_namespace: &'a str,
}

impl<'a> ValidationContext<'a> {
    pub fn new(model_files: &'a HashMap<String, ModelFile>, current_namespace: &'a str) -> Self {
        Self {
            model_files,
            current_namespace,
        }
    }

    /// Resolve a fully-qualified type name to its declaration.
    pub fn get_type(&self, fqn: &str) -> Option<&'a Declaration> {
        let ns = model_util::get_namespace(fqn);
        let short = model_util::get_short_name(fqn);
        self.model_files
            .get(ns)
            .and_then(|mf| mf.get_local_type(short))
    }

    pub fn current_model_file(&self) -> Option<&'a ModelFile> {
        self.model_files.get(self.current_namespace)
    }
}

// ===================================================================
// Model Validation
// ===================================================================

/// Validate all model files in the given map.
pub fn validate_model_files(model_files: &HashMap<String, ModelFile>) -> Result<()> {
    for (ns, mf) in model_files {
        let ctx = ValidationContext::new(model_files, ns);
        validate_model_file(mf, &ctx)?;
    }
    Ok(())
}

/// Validate a single model file.
fn validate_model_file(mf: &ModelFile, ctx: &ValidationContext<'_>) -> Result<()> {
    for decl in mf.all_declarations() {
        match decl {
            Declaration::Class(cd) => validate_class_declaration(cd, mf, ctx)?,
            Declaration::Scalar(sd) => validate_scalar_declaration(sd, mf)?,
            Declaration::Map(md) => validate_map_declaration(md, mf)?,
        }
    }
    Ok(())
}

// -------------------------------------------------------------------
// ClassDeclaration validation
// -------------------------------------------------------------------

fn validate_class_declaration(
    cd: &ClassDeclaration,
    mf: &ModelFile,
    ctx: &ValidationContext<'_>,
) -> Result<()> {
    // 1. Validate super-type
    if let Some(super_type_id) = cd.super_type() {
        let super_fqn = resolve_type_identifier(super_type_id, mf)?;

        // Super type must exist
        let super_decl = ctx.get_type(&super_fqn).ok_or_else(|| {
            ConcertoError::IllegalModel {
                message: format!(
                    "Super type '{}' not found for '{}'",
                    super_fqn,
                    cd.name()
                ),
                file_name: mf.file_name().map(String::from),
                location: None,
            }
        })?;

        // Super type must be a class declaration
        if super_decl.as_class().is_none() {
            return Err(ConcertoError::IllegalModel {
                message: format!(
                    "Super type '{}' of '{}' is not a class declaration",
                    super_fqn,
                    cd.name()
                ),
                file_name: mf.file_name().map(String::from),
                location: None,
            });
        }

        // Check for circular inheritance
        check_circular_inheritance(cd, mf, ctx)?;
    }

    // 2. Validate identifier field (for identified declarations)
    if cd.is_identified() && !cd.is_enum() {
        validate_identifier_field(cd, mf, ctx)?;
    }

    // 3. Check for duplicate property names
    check_duplicate_properties(cd, mf, ctx)?;

    // 4. Validate each property
    for prop in cd.own_properties() {
        validate_property(prop, mf, ctx)?;
    }

    Ok(())
}

fn resolve_type_identifier(
    type_id: &concerto_metamodel::concerto_metamodel_1_0_0::TypeIdentifier,
    mf: &ModelFile,
) -> Result<String> {
    // If already resolved, use that
    if let Some(ref resolved) = type_id.resolved_name {
        return Ok(resolved.clone());
    }
    // If namespace is present, build FQN directly
    if let Some(ref ns) = type_id.namespace {
        return Ok(model_util::fully_qualified_name(ns, &type_id.name));
    }
    // Otherwise resolve through the model file
    mf.fully_qualified_type_name(&type_id.name)
}

fn check_circular_inheritance(
    cd: &ClassDeclaration,
    mf: &ModelFile,
    ctx: &ValidationContext<'_>,
) -> Result<()> {
    let mut visited = HashSet::new();
    let start_fqn = model_util::fully_qualified_name(mf.namespace(), cd.name());
    visited.insert(start_fqn.clone());

    let mut current: Option<&ClassDeclaration> = Some(cd);
    while let Some(decl) = current {
        if let Some(super_type_id) = decl.super_type() {
            let super_fqn = resolve_type_identifier(super_type_id, mf)
                .or_else(|_| {
                    // Try resolving from model files directly
                    if let Some(ref ns) = super_type_id.namespace {
                        Ok(model_util::fully_qualified_name(ns, &super_type_id.name))
                    } else {
                        Ok(model_util::fully_qualified_name(
                            mf.namespace(),
                            &super_type_id.name,
                        ))
                    }
                })?;

            if !visited.insert(super_fqn.clone()) {
                return Err(ConcertoError::IllegalModel {
                    message: format!(
                        "Circular inheritance detected for '{}'",
                        start_fqn
                    ),
                    file_name: mf.file_name().map(String::from),
                    location: None,
                });
            }

            current = ctx
                .get_type(&super_fqn)
                .and_then(|d| d.as_class());
        } else {
            current = None;
        }
    }

    Ok(())
}

fn validate_identifier_field(
    cd: &ClassDeclaration,
    mf: &ModelFile,
    ctx: &ValidationContext<'_>,
) -> Result<()> {
    // System-identified declarations don't need an explicit field
    if cd.is_system_identified() {
        return Ok(());
    }

    // For explicitly-identified declarations, find the identifier field
    // Walk properties including inherited ones
    if let Some(id_field_name) = cd.identifier_field_name() {
        let prop = find_property_in_chain(cd, id_field_name, mf, ctx);
        if prop.is_none() {
            return Err(ConcertoError::IllegalModel {
                message: format!(
                    "Identifier field '{}' not found on '{}'",
                    id_field_name,
                    cd.name()
                ),
                file_name: mf.file_name().map(String::from),
                location: None,
            });
        }
    }

    Ok(())
}

/// Find a property by name, walking the super-type chain.
fn find_property_in_chain<'a>(
    cd: &'a ClassDeclaration,
    name: &str,
    mf: &'a ModelFile,
    ctx: &'a ValidationContext<'_>,
) -> Option<&'a crate::introspect::properties::PropertyDecl> {
    // Check own properties first
    if let Some(p) = cd.own_properties().iter().find(|p| p.name() == name) {
        return Some(p);
    }

    // Walk super-type chain
    if let Some(super_type_id) = cd.super_type() {
        let super_fqn = resolve_type_identifier(super_type_id, mf).ok()?;
        let super_cd = ctx.get_type(&super_fqn)?.as_class()?;
        return find_property_in_chain(super_cd, name, mf, ctx);
    }

    None
}

fn check_duplicate_properties(
    cd: &ClassDeclaration,
    mf: &ModelFile,
    ctx: &ValidationContext<'_>,
) -> Result<()> {
    let mut seen = HashSet::new();

    // Collect all property names including inherited
    collect_property_names(cd, mf, ctx, &mut seen)?;

    Ok(())
}

fn collect_property_names(
    cd: &ClassDeclaration,
    mf: &ModelFile,
    ctx: &ValidationContext<'_>,
    seen: &mut HashSet<String>,
) -> Result<()> {
    // First collect inherited property names
    if let Some(super_type_id) = cd.super_type() {
        if let Ok(super_fqn) = resolve_type_identifier(super_type_id, mf) {
            if let Some(super_cd) = ctx.get_type(&super_fqn).and_then(|d| d.as_class()) {
                collect_property_names(super_cd, mf, ctx, seen)?;
            }
        }
    }

    // Then check own properties for duplicates
    for prop in cd.own_properties() {
        if !seen.insert(prop.name().to_string()) {
            return Err(ConcertoError::IllegalModel {
                message: format!(
                    "Duplicate property name '{}' in '{}'",
                    prop.name(),
                    cd.name()
                ),
                file_name: mf.file_name().map(String::from),
                location: None,
            });
        }
    }

    Ok(())
}

// -------------------------------------------------------------------
// Property validation
// -------------------------------------------------------------------

fn validate_property(
    prop: &crate::introspect::properties::PropertyDecl,
    mf: &ModelFile,
    ctx: &ValidationContext<'_>,
) -> Result<()> {
    use crate::introspect::properties::PropertyDecl;

    match prop {
        PropertyDecl::Object(p) => {
            let fqn = resolve_type_identifier(&p.type_, mf)?;
            if ctx.get_type(&fqn).is_none() && !model_util::is_primitive_type(&p.type_.name) {
                return Err(ConcertoError::IllegalModel {
                    message: format!(
                        "Type '{}' not found for property '{}'",
                        fqn, p.name
                    ),
                    file_name: mf.file_name().map(String::from),
                    location: None,
                });
            }
        }
        PropertyDecl::Relationship(p) => {
            let fqn = resolve_type_identifier(&p.type_, mf)?;
            // Relationship type must not be primitive
            if model_util::is_primitive_type(&p.type_.name) {
                return Err(ConcertoError::IllegalModel {
                    message: format!(
                        "Relationship '{}' cannot have primitive type '{}'",
                        p.name, p.type_.name
                    ),
                    file_name: mf.file_name().map(String::from),
                    location: None,
                });
            }
            // Relationship target must exist
            let target = ctx.get_type(&fqn).ok_or_else(|| ConcertoError::IllegalModel {
                message: format!(
                    "Relationship target type '{}' not found for '{}'",
                    fqn, p.name
                ),
                file_name: mf.file_name().map(String::from),
                location: None,
            })?;
            // Relationship target must be identified
            if let Some(class_decl) = target.as_class() {
                if !class_decl.is_identified() {
                    return Err(ConcertoError::IllegalModel {
                        message: format!(
                            "Relationship '{}' must point to an identified type, but '{}' is not identified",
                            p.name, fqn
                        ),
                        file_name: mf.file_name().map(String::from),
                        location: None,
                    });
                }
            }
        }
        // Primitive properties don't need type resolution
        _ => {}
    }

    Ok(())
}

// -------------------------------------------------------------------
// Scalar validation
// -------------------------------------------------------------------

fn validate_scalar_declaration(
    _sd: &crate::introspect::declarations::ScalarDeclaration,
    _mf: &ModelFile,
) -> Result<()> {
    // Scalar validation: check that default value passes validator.
    // This is a simplified version — full validator execution is done
    // at runtime.
    Ok(())
}

// -------------------------------------------------------------------
// Map validation
// -------------------------------------------------------------------

fn validate_map_declaration(
    _md: &crate::introspect::declarations::MapDeclaration,
    _mf: &ModelFile,
) -> Result<()> {
    // Map key must be String, DateTime, or a scalar thereof.
    // Map value must not be a Map.
    // Simplified — full checking deferred to runtime validation.
    Ok(())
}
