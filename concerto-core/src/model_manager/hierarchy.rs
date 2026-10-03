//! Inheritance: class-like chains ([`ClassInfo`]), properties along them,
//! super types, subclasses and assignability.

use super::*;

/// The inheritance facts of one class-like or enum declaration, from its
/// declaration handle up to its root (P5-13): what `class_info`,
/// `getProperties`, `getProperty` and `getIdentifierFieldName` walk on every
/// call. It depends only on the registered files, so it is cached per
/// declaration until they change ([`ModelManager::invalidate_caches`]).
#[derive(Debug)]
pub(super) struct ClassInfo {
    /// The declaration itself, then each super type up to the root.
    pub(super) chain: Box<[DeclId]>,
    /// Every property along the chain, in `getProperties()` order.
    pub(super) properties: Box<[PropId]>,
}

js_compat_pub! {
    /// A borrowed view of every property of a class-like or enum
    /// declaration, own and inherited (TS `getProperties()`), each with the
    /// fully-qualified name of the declaration that declares it: the
    /// allocation-free form of [`ModelManager::properties`] (P5-13).
    #[derive(Clone)]
    pub struct ClassProperties<'a> {
        mm: &'a ModelManager,
        info: Arc<ClassInfo>,
    }
}

impl<'a> ClassProperties<'a> {
    /// Each property with its declaring type's fully-qualified name, in
    /// `getProperties()` order.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a Property)> + '_ {
        let mm = self.mm;
        self.info.properties.iter().map(move |id| {
            mm.property_with_owner(*id)
                .expect("a cached property handle is live")
        })
    }

    /// Each property's handle, in `getProperties()` order (P5-106).
    #[cfg(feature = "js-compat")]
    pub fn ids(&self) -> impl Iterator<Item = PropId> + '_ {
        self.info.properties.iter().copied()
    }

    /// The first property named `name` (TS `getProperty(name)`).
    pub fn find(&self, name: &str) -> Option<(&'a str, &'a Property)> {
        self.iter().find(|(_, p)| p.name() == name)
    }

    /// [`ClassProperties::find`], with the property's handle (A-14).
    pub(super) fn find_with_id(&self, name: &str) -> Option<(PropId, &'a Property)> {
        let mm = self.mm;
        self.info.properties.iter().find_map(|id| {
            let property = mm.property_by_id(*id)?;
            (property.name() == name).then_some((*id, property))
        })
    }
}

/// The class-like facts a `ClassDeclaration` or an `EnumDeclaration` carries,
/// unified for the members TS defines once on `ClassDeclaration` and
/// `EnumDeclaration` inherits unchanged (enumdeclaration.ts overrides only
/// `toString` and `declarationKind`, PORTING.md 1.1 rule 2). The manager's
/// inheritance-walking members (`class_info` and everything built on it)
/// read a declaration through this instead of `Declaration::as_class`, so
/// that an enum's implicit `Concept` super type, own properties and identity
/// are seen the same way a concept-like declaration's are.
#[derive(Clone, Copy)]
pub(super) enum ClassLike<'a> {
    Class(&'a ClassDeclaration),
    Enum(&'a EnumDeclaration),
}

impl<'a> ClassLike<'a> {
    pub(super) fn from_declaration(declaration: &'a Declaration) -> Option<Self> {
        match declaration {
            Declaration::Class(class) => Some(Self::Class(class)),
            Declaration::Enum(e) => Some(Self::Enum(e)),
            Declaration::Scalar(_) | Declaration::Map(_) => None,
        }
    }

    pub(super) fn own_properties(&self) -> &'a [Property] {
        match self {
            Self::Class(class) => class.own_properties(),
            Self::Enum(e) => e.own_properties(),
        }
    }

    pub(super) fn own_identifier_field_name(&self) -> Option<&'a str> {
        match self {
            Self::Class(class) => class.own_identifier_field_name(),
            Self::Enum(e) => e.own_identifier_field_name(),
        }
    }

    /// The direct super type this declaration's own AST names, or the
    /// implicit `Concept` — `None` only for the system model's own `Concept`
    /// declaration.
    pub(super) fn super_type(&self) -> Option<mm::TypeIdentifier> {
        match self {
            Self::Class(class) => class.super_type().cloned(),
            Self::Enum(e) => Some(e.implicit_super_type()),
        }
    }

    pub(super) fn location(&self) -> Option<&'a mm::Range> {
        match self {
            Self::Class(class) => class.location(),
            Self::Enum(e) => e.location(),
        }
    }
}

/// TS: `ClassDeclaration._resolveSuperType`/`getProperty`/… all reached
/// through a receiver whose prototype chain includes `ClassDeclaration`;
/// reaching one of these members on a scalar or map declaration is not a TS
/// shape at all (neither extends `ClassDeclaration`), so no fixture or TS
/// class corresponds to it (PORTING.md 2.3), the same as [`unknown`] and
/// [`not_a_function`] above.
pub(super) fn not_a_class_like(fqn: &str) -> Error {
    Error::illegal_model(
        format!("{fqn} is not a concept-like or enum declaration"),
        None,
        None,
    )
}

impl ModelManager {
    /// The direct super type of the concept-like or enum type `fqn`, with its
    /// fully-qualified name, or `None` when it has none (only the system
    /// `Concept`).
    ///
    /// TS: `ClassDeclaration.getSuperTypeDeclaration`.
    pub fn super_type(&self, fqn: &str) -> Result<Option<(String, &Declaration)>> {
        let Some(super_fqn) = self.super_type_name(fqn)? else {
            return Ok(None);
        };
        let declaration = self.get_declaration(&super_fqn)?;
        Ok(Some((super_fqn, declaration)))
    }

    /// Every super type of `fqn`, from its direct super type up to the root,
    /// with their fully-qualified names. A cyclic chain is an
    /// `IllegalModel` error (BC-11).
    ///
    /// TS: `ClassDeclaration.getAllSuperTypeDeclarations`.
    pub fn super_types(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>> {
        self.with_declarations(self.super_type_names(fqn)?)
    }

    /// The declarations that directly extend `fqn`, with their
    /// fully-qualified names, in load order.
    ///
    /// TS: `ClassDeclaration.getDirectSubclasses`.
    pub fn subclasses(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>> {
        self.with_declarations(self.direct_subclass_names(fqn)?)
    }

    /// `fqn` itself and every declaration that transitively extends it, with
    /// their fully-qualified names: what a value of type `fqn` can be.
    ///
    /// TS: `ClassDeclaration.getAssignableClassDeclarations`.
    pub fn assignable_types(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>> {
        self.with_declarations(self.assignable_type_names(fqn)?)
    }

    /// Each name, with its declaration.
    pub(super) fn with_declarations(
        &self,
        names: Vec<String>,
    ) -> Result<Vec<(String, &Declaration)>> {
        names
            .into_iter()
            .map(|name| {
                let declaration = self.get_declaration(&name)?;
                Ok((name, declaration))
            })
            .collect()
    }

    /// The properties declared directly on the concept-like or enum type
    /// `fqn` (for an enum, its values), not those it inherits.
    ///
    /// TS: `ClassDeclaration.getOwnProperties`.
    pub fn own_properties(&self, fqn: &str) -> Result<&[Property]> {
        let class = ClassLike::from_declaration(self.get_declaration(fqn)?)
            .ok_or_else(|| not_a_class_like(fqn))?;
        Ok(class.own_properties())
    }

    /// Every property of `fqn`, own and inherited, with the fully-qualified
    /// name of the declaration that declares each: the type's own first,
    /// then each super type's up to the root. It is an error when `fqn` is
    /// not a concept-like or enum type, a super type cannot be resolved, or
    /// the chain is cyclic (`IllegalModel`, BC-11).
    ///
    /// TS: `ClassDeclaration.getProperties`, with `Property.getParent()`.
    pub fn properties(&self, fqn: &str) -> Result<Vec<(String, &Property)>> {
        Ok(self
            .class_properties(fqn)?
            .iter()
            .map(|(owner, property)| (owner.to_string(), property))
            .collect())
    }

    /// [`ModelManager::properties`], borrowed from the model (P5-13): the
    /// same properties, owners, order and errors, with nothing copied.
    pub(crate) fn class_properties(&self, fqn: &str) -> Result<ClassProperties<'_>> {
        Ok(ClassProperties {
            mm: self,
            info: self.class_info(fqn)?,
        })
    }

    js_compat_pub! {
        /// [`ModelManager::properties`], borrowed from the model, for a
        /// declaration handle (P5-13).
        pub fn class_properties_of(&self, id: DeclId) -> Result<ClassProperties<'_>> {
            Ok(ClassProperties {
                mm: self,
                info: self.class_info_of(id)?,
            })
        }
    }

    /// A property and the fully-qualified name of its declaration.
    pub(super) fn property_with_owner(&self, id: PropId) -> Option<(&str, &Property)> {
        let slot = self.properties.get(id.slot())?;
        let owner = self.declarations.get(slot.declaration.slot())?;
        Some((&*owner.fqn, self.property_by_id(id)?))
    }

    /// The property called `name`, own or inherited, with the
    /// fully-qualified name of the declaration that declares it, or `None`.
    ///
    /// TS: `ClassDeclaration.getProperty`.
    pub fn property(&self, fqn: &str, name: &str) -> Result<Option<(String, &Property)>> {
        Ok(self
            .class_properties(fqn)?
            .find(name)
            .map(|(owner, property)| (owner.to_string(), property)))
    }

    /// The property at a dotted `path` (`a.b.c`), following the declared type
    /// of each property but the last, with the fully-qualified name of the
    /// declaration that declares it.
    ///
    /// TS: `ClassDeclaration.getNestedProperty`.
    pub fn property_path(&self, fqn: &str, path: &str) -> Result<(String, &Property)> {
        let id = self.nested_property_id(fqn, path)?;
        let (owner, property) = self
            .property_with_owner(id)
            .ok_or_else(|| unknown(Node::Property(id)))?;
        Ok((owner.to_string(), property))
    }

    /// The name of the field that identifies instances of `fqn`: its own
    /// (an `identified by` field, or `$identifier` for `identified`), or its
    /// nearest super type's. `None` when nothing in the chain declares one.
    ///
    /// TS: `ClassDeclaration.getIdentifierFieldName`.
    pub fn identifier_field(&self, fqn: &str) -> Result<Option<&str>> {
        let info = self.class_info(fqn)?;
        Ok(self.chain_identifier_field(&info))
    }

    js_compat_pub! {
        /// [`ModelManager::identifier_field`] for a declaration handle (P5-13).
        pub fn identifier_field_of(&self, id: DeclId) -> Result<Option<&str>> {
            let info = self.class_info_of(id)?;
            Ok(self.chain_identifier_field(&info))
        }
    }

    /// The nearest identifying field along a cached chain.
    pub(super) fn chain_identifier_field(&self, info: &ClassInfo) -> Option<&str> {
        info.chain.iter().find_map(|id| {
            self.declaration(*id)
                .and_then(ClassLike::from_declaration)
                .and_then(|class| class.own_identifier_field_name())
        })
    }

    /// [`ModelManager::identifier_field_name`], as a boolean.
    ///
    /// TS: `ClassDeclaration.isIdentified` (src/introspect/classdeclaration.ts):
    /// `!!this.getIdentifierFieldName()`, inherited unchanged by `EnumDeclaration`.
    pub fn is_identified(&self, fqn: &str) -> Result<bool> {
        Ok(self.identifier_field(fqn)?.is_some())
    }

    /// [`ModelManager::identifier_field_name`], `true` only for the system
    /// `$identifier`.
    ///
    /// TS: `ClassDeclaration.isSystemIdentified`: `this.getIdentifierFieldName()
    /// === '$identifier'`, inherited unchanged by `EnumDeclaration`.
    pub fn is_system_identified(&self, fqn: &str) -> Result<bool> {
        Ok(self.identifier_field(fqn)? == Some("$identifier"))
    }

    /// Every property of a type, own and inherited: the type's own first,
    /// then each super type's up to the root ([`ModelManager::properties`]
    /// gives each with the name of the declaration that declares it). It is
    /// an error when `fqn` is not a concept-like or enum type, a super type
    /// cannot be resolved, or the chain is cyclic (`RecursionLimit`).
    ///
    /// TS: `ClassDeclaration.getProperties`.
    pub fn get_all_properties(&self, fqn: &str) -> Result<Vec<&Property>> {
        Ok(self
            .class_properties(fqn)?
            .iter()
            .map(|(_, property)| property)
            .collect())
    }

    /// The handle of the property at a dotted `path` (`a.b.c`), following
    /// the declared types of each element but the last: the walk
    /// [`ModelManager::property_path`] and the deprecated
    /// [`ModelManager::get_nested_property`] both map from (A-14).
    ///
    /// TS: `ClassDeclaration.getNestedProperty` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    pub(super) fn nested_property_id(&self, fqn: &str, property_path: &str) -> Result<PropId> {
        let names: Vec<&str> = property_path.split('.').collect();
        let mut search_root = fqn.to_string();
        let mut result = None;
        for (n, name) in names.iter().enumerate() {
            let Some((id, property)) = self.class_properties(&search_root)?.find_with_id(name)
            else {
                return Err(ContractError::new(
                    ErrorKind::IllegalModel,
                    "classdeclaration-getnestedproperty-doesnotexist",
                    vec![("propertyName", (*name).to_string()), ("fqn", search_root)],
                )
                .into());
            };
            let is_last = n == names.len() - 1;
            if !is_last {
                // TS: `Property.isTypeEnum` (src/introspect/property.ts):
                // `this.isPrimitive() ? false : this.getParent().getModelFile()
                // .getType(this.getType()).isEnum()`. Reached here only for an
                // object/relationship field (the walk's own `getProperty`
                // already ruled out a missing property, and an intermediate
                // step is never itself an enum *value* — the field whose
                // declared type is an enum trips this same check one level
                // higher, before the walk ever reaches the value).
                let is_enum = !property.is_primitive()
                    && model_util::is_enum(self, &Node::Property(id))?.unwrap_or(false);
                if property.is_primitive() || is_enum {
                    return Err(ContractError::new(
                        ErrorKind::InvalidArgument,
                        "classdeclaration-getnestedproperty-primitiveorenum",
                        vec![
                            ("propertyName", (*name).to_string()),
                            ("propertyPath", property_path.to_string()),
                        ],
                    )
                    .into());
                }
                search_root = self.get_fully_qualified_type_name(&Node::Property(id))?;
            }
            result = Some(id);
        }
        Ok(result.expect("propertyPath.split('.') always yields at least one name"))
    }

    /// The [`PropId`] of the property named `name`, declared directly on
    /// `declaring_fqn` (not inherited): a test helper.
    #[cfg(test)]
    pub(super) fn find_property_id(
        &self,
        declaring_fqn: &str,
        name: &str,
    ) -> Result<Option<PropId>> {
        let Some(owner) = self.declaration_id(declaring_fqn) else {
            return Ok(None);
        };
        Ok(self
            .property_ids(owner)
            .find(|id| self.property_by_id(*id).is_some_and(|p| p.name() == name)))
    }

    /// The body of the deprecated [`ModelManager::get_super_type`], whose
    /// docs say what it returns (A-11).
    pub(super) fn super_type_name(&self, fqn: &str) -> Result<Option<String>> {
        let class = ClassLike::from_declaration(self.get_declaration(fqn)?)
            .ok_or_else(|| not_a_class_like(fqn))?;
        self.super_type_fqn(&class, namespace_of(fqn))
    }

    /// The body of the deprecated [`ModelManager::get_all_super_type_names`],
    /// whose docs say what it returns (A-11).
    pub(super) fn super_type_names(&self, fqn: &str) -> Result<Vec<String>> {
        // The chain starts with the type itself.
        let info = self.class_info(fqn)?;
        info.chain[1..]
            .iter()
            .map(|id| self.declaration_fqn(*id))
            .collect()
    }

    /// Every class-like or enum declaration loaded, across every model file
    /// whose namespace is not in [`EXCLUDE_NS`], in registration order — the
    /// population `getAssignableClassDeclarations` and `getDirectSubclasses`
    /// search (TS: `new Introspector(modelManager).getClassDeclarations()`,
    /// src/introspect/introspector.ts, which reads
    /// `modelManager.getModelFiles()` with no argument, so the system and
    /// decorator models are left out by their namespace string, not by
    /// `ModelFile.isSystemModelFile`; then every declaration that is not a
    /// map or a scalar, which leaves the class-like kinds and
    /// `EnumDeclaration`).
    ///
    /// Each comes with its handle and its fully-qualified name, borrowed
    /// from the arena (A-8).
    pub(super) fn all_class_like(&self) -> impl Iterator<Item = (DeclId, &str, ClassLike<'_>)> {
        self.declarations_in(self.user_file_slots())
            .filter_map(|(id, fqn, declaration)| {
                Some((id, fqn, ClassLike::from_declaration(declaration)?))
            })
    }

    /// `fqn` itself, plus every declaration that (transitively) extends it,
    /// in TS's pre-order: each declaration before its own direct
    /// subclasses, which come in load order.
    ///
    /// A declaration met again below itself is a cyclic inheritance chain:
    /// the BC-11 `IllegalModelException` naming the cycle (R1; TS 5.0.0
    /// recursed until V8's stack overflowed, DV-013). Only a model loaded
    /// with validation disabled has one.
    ///
    /// TS: `ClassDeclaration.getAssignableClassDeclarations`, inherited
    /// unchanged by `EnumDeclaration`.
    pub(super) fn assignable_type_names(&self, fqn: &str) -> Result<Vec<String>> {
        Ok(match self.declaration_id(fqn) {
            Some(id) => self
                .assignable_ids(id)?
                .into_iter()
                .map(|id| self.declaration_fqn(id))
                .collect::<Result<_>>()?,
            None => {
                // No declaration has this name, so none extends it; the
                // population is still resolved, for its errors.
                self.direct_subclass_ids(None)?;
                vec![fqn.to_string()]
            }
        })
    }

    js_compat_pub! {
        /// The handles of the declaration `id` and of every declaration that
        /// (transitively) extends it: [`ModelManager::assignable_types`] by
        /// handle (P5-106, BC-52), answered from the cached direct
        /// subclasses ([`ModelManager::direct_subclass_ids`]).
        ///
        /// TS: `ClassDeclaration.getAssignableClassDeclarations`.
        pub fn assignable_ids(&self, id: DeclId) -> Result<Vec<DeclId>> {
            /// `path` holds the declarations from `id` down to `children`'
            /// super type.
            fn walk(
                mm: &ModelManager,
                children: &[DeclId],
                seen: &mut FxHashSet<DeclId>,
                results: &mut Vec<DeclId>,
                path: &mut Vec<DeclId>,
            ) -> Result<()> {
                for &child in children {
                    if let Some(start) = path.iter().position(|seen| *seen == child) {
                        // `path` runs from super type to subclass; the
                        // chain runs the other way, from `child` up to
                        // `child` again.
                        let cycle = std::iter::once(child)
                            .chain(path[start + 1..].iter().rev().copied())
                            .collect::<Vec<_>>();
                        return Err(mm.circular_inheritance(&cycle, child));
                    }
                    if seen.insert(child) {
                        results.push(child);
                    }
                    let grandchildren = mm
                        .direct_subclass_ids(Some(child))?
                        .unwrap_or_else(|| Arc::from([]));
                    if !grandchildren.is_empty() {
                        path.push(child);
                        let walked = walk(mm, &grandchildren, seen, results, path);
                        path.pop();
                        walked?;
                    }
                }
                Ok(())
            }
            if self.declaration(id).is_none() {
                return Err(unknown(Node::Declaration(id)));
            }
            let mut results = Vec::new();
            walk(self, &[id], &mut FxHashSet::default(), &mut results, &mut Vec::new())?;
            Ok(results)
        }
    }

    /// The handles of the declarations that directly extend the
    /// declaration `id`, in load order: [`ModelManager::subclasses`] by
    /// handle (P5-106, BC-52).
    ///
    /// TS: `ClassDeclaration.getDirectSubclasses`.
    #[cfg(feature = "js-compat")]
    pub fn direct_subclasses_of(&self, id: DeclId) -> Result<Arc<[DeclId]>> {
        if self.declaration(id).is_none() {
            return Err(unknown(Node::Declaration(id)));
        }
        Ok(self
            .direct_subclass_ids(Some(id))?
            .unwrap_or_else(|| Arc::from([])))
    }

    /// The direct subclasses of `id` (`None`: of no declaration, only
    /// resolving the population), from the cache, or from one pass that
    /// builds TS's `subclassMap` and caches every declaration's bucket at
    /// once: every loaded class-like declaration ([`Self::all_class_like`])
    /// under the declaration its super type resolves to, in the order they
    /// are met. A super type that does not resolve fails the pass, as it
    /// fails TS's `getSuperType()`, and nothing is cached.
    pub(super) fn direct_subclass_ids(&self, id: Option<DeclId>) -> Result<Option<Arc<[DeclId]>>> {
        if let Some(id) = id
            && let Some(found) = self.decl_cache.get(id, |facts| &facts.direct_subclasses)
        {
            return Ok(Some(found));
        }
        let mut buckets: Vec<Vec<DeclId>> = vec![Vec::new(); self.declarations.len()];
        for (child, child_fqn, class) in self.all_class_like() {
            // A cached chain already holds the declaration the direct super
            // type resolved to (A-8): its second entry is exactly what
            // resolving the name again finds, and it was cached only once
            // that resolution succeeded.
            let parent = match self.decl_cache.get(child, |facts| &facts.class) {
                Some(info) => info.chain.get(1).copied(),
                None => self
                    .super_type_fqn(&class, namespace_of(child_fqn))?
                    .and_then(|super_fqn| self.declaration_id(&super_fqn)),
            };
            if let Some(parent) = parent {
                buckets[parent.slot()].push(child);
            }
        }
        let empty: Arc<[DeclId]> = Arc::from([]);
        let answers: Vec<Arc<[DeclId]>> = buckets
            .into_iter()
            .map(|bucket| {
                if bucket.is_empty() {
                    empty.clone()
                } else {
                    Arc::from(bucket)
                }
            })
            .collect();
        let found = id.and_then(|id| answers.get(id.slot()).cloned());
        self.decl_cache
            .fill(|facts| &mut facts.direct_subclasses, answers);
        Ok(found)
    }

    /// The body of the deprecated [`ModelManager::get_direct_subclasses`],
    /// whose docs say what it returns (A-11).
    pub(super) fn direct_subclass_names(&self, fqn: &str) -> Result<Vec<String>> {
        let id = self.declaration_id(fqn);
        let Some(children) = self.direct_subclass_ids(id)? else {
            return Ok(Vec::new());
        };
        children
            .iter()
            .map(|child| self.declaration_fqn(*child))
            .collect()
    }

    /// Returns `true` if a value of `sub_fqn` is also a valid `super_fqn`: the
    /// two are the same type, or `sub_fqn` transitively extends `super_fqn`.
    ///
    /// On a cyclic inheritance chain this walks `class_info`, so it returns
    /// the same `IllegalModelException` naming the cycle as
    /// `getProperties`/`getProperty`/`getIdentifierFieldName` (BC-11, R1;
    /// TS 5.0.0 returns `true` when the target is in the cycle and otherwise
    /// loops until it runs out of memory, DV-013).
    pub fn is_assignable_to(&self, sub_fqn: &str, super_fqn: &str) -> Result<bool> {
        if sub_fqn == super_fqn {
            return Ok(true);
        }
        // Every class-like declaration walks its chain, an enum included:
        // TS `EnumDeclaration extends ClassDeclaration`, so its implicit
        // `Concept` super type is on `getSuperTypeDeclaration()`'s chain too
        // (P5-98, A-1). Only a scalar or map declaration has no chain.
        match ClassLike::from_declaration(self.get_declaration(sub_fqn)?) {
            None => Ok(false),
            Some(_) => {
                let info = self.class_info(sub_fqn)?;
                Ok(info
                    .chain
                    .iter()
                    .any(|id| self.decl_fqn(*id).is_ok_and(|fqn| fqn == super_fqn)))
            }
        }
    }

    /// The cached [`ClassInfo`] of the declaration `fqn` names, resolved
    /// the way `getType` does ([`ModelManager::type_declaration`]).
    pub(super) fn class_info(&self, fqn: &str) -> Result<Arc<ClassInfo>> {
        self.class_info_of(self.type_declaration_impl(fqn)?)
    }

    /// BC-11 (R1): the `IllegalModelException` for a cyclic inheritance
    /// chain. `cycle` is the loop from `repeated` round to the declaration
    /// whose super type is `repeated` again; the error carries `repeated`'s
    /// model file (TS 5.0.0 overflowed V8's stack instead, DV-013).
    pub(super) fn circular_inheritance(&self, cycle: &[DeclId], repeated: DeclId) -> Error {
        let name = |id: DeclId| self.decl_fqn(id).unwrap_or_default().to_string();
        let path = cycle
            .iter()
            .chain(std::iter::once(&repeated))
            .map(|id| name(*id))
            .collect::<Vec<_>>()
            .join(" -> ");
        let mut err = ContractError::new(
            ErrorKind::IllegalModel,
            "classdeclaration-circularinheritance",
            vec![("type", name(repeated)), ("cycle", path)],
        );
        err.model_file = Some(
            self.model_file_of(repeated)
                .and_then(|file| self.file(file))
                .and_then(ModelFile::file_name)
                .map(str::to_string),
        );
        err.into()
    }

    /// The cached [`ClassInfo`] of a declaration, computed on first use.
    ///
    /// TS walks the chain by recursion (`ClassDeclaration.getProperties`,
    /// `getProperty`, `getIdentifierFieldName`), with no cycle check, so in
    /// TS 5.0.0 a cyclic chain overflowed V8's stack (DV-013). This walk is
    /// a loop with a visited set (PORTING.md 2.5 rule 1) and, when it meets
    /// a declaration again, returns an `IllegalModelException` naming the
    /// cycle (BC-11, R1), after the same earlier checks: a missing or
    /// non-class super type still fails first.
    pub(super) fn class_info_of(&self, id: DeclId) -> Result<Arc<ClassInfo>> {
        self.decl_cache.get_or_try_insert_with(
            id,
            |facts| &mut facts.class,
            || self.compute_class_info(id),
        )
    }

    /// [`Self::class_info_of`]'s walk, uncached.
    pub(super) fn compute_class_info(&self, id: DeclId) -> Result<Arc<ClassInfo>> {
        let mut chain = Vec::new();
        let mut properties = Vec::new();
        let mut current = id;
        loop {
            if let Some(start) = chain.iter().position(|seen| *seen == current) {
                return Err(self.circular_inheritance(&chain[start..], current));
            }
            let current_fqn = self.decl_fqn(current)?;
            let declaration = self
                .declaration(current)
                .ok_or_else(|| unknown(Node::Declaration(current)))?;
            let class = ClassLike::from_declaration(declaration)
                .ok_or_else(|| not_a_class_like(current_fqn))?;

            let next = self.super_type_fqn(&class, namespace_of(current_fqn))?;
            chain.push(current);
            properties.extend(self.property_ids(current));
            match next {
                // TS resolves each step of the chain the same way `getType`
                // does (`this.modelManager.getType(this.superType)`,
                // `ClassDeclaration.getSuperTypeDeclaration`), so an
                // unregistered super-type namespace raises `getType`'s own
                // `TypeNotFoundException` ("Namespace is not defined for type
                // ...").
                Some(parent) => current = self.type_declaration_impl(&parent)?,
                None => break,
            }
        }
        Ok(Arc::new(ClassInfo {
            chain: chain.into_boxed_slice(),
            properties: properties.into_boxed_slice(),
        }))
    }

    /// For the validation plan: a class-like declaration's chain and its
    /// properties (own, then inherited), from the inheritance cache.
    pub(crate) fn class_chain_and_properties(
        &self,
        id: DeclId,
    ) -> Result<(Vec<DeclId>, Vec<PropId>)> {
        let info = self.class_info_of(id)?;
        Ok((info.chain.to_vec(), info.properties.to_vec()))
    }

    /// For the validation plan: the identifier field a class-like
    /// declaration itself declares, if any.
    pub(crate) fn own_identifier_field_name_of(&self, id: DeclId) -> Option<&str> {
        self.declaration(id)
            .and_then(ClassLike::from_declaration)
            .and_then(|class| class.own_identifier_field_name())
    }

    /// For the validation plan: the declaration that declares a property.
    pub(crate) fn property_owner_of(&self, id: PropId) -> Option<DeclId> {
        self.properties.get(id.slot()).map(|slot| slot.declaration)
    }

    /// For the validation plan: [`Self::property_with_owner`].
    pub(crate) fn property_with_owner_of(&self, id: PropId) -> Option<(&str, &Property)> {
        self.property_with_owner(id)
    }

    /// Works out the full name of a class's direct super type, resolved in the
    /// namespace where the class is declared.
    ///
    /// TS: `this.superType = this.ast.superType.name` (`ClassDeclaration.process`)
    /// keeps only the AST `TypeIdentifier`'s `name`, discarding `namespace`
    /// and `resolvedName` — an aliased import's `TypeIdentifier` carries the
    /// *target* declaration's namespace in `namespace` (with the alias, not
    /// the target's own name, in `name`), which `resolveImport`'s alias
    /// lookup needs the whole import list to untangle correctly
    /// (`this.getModelFile().isImportedType(this.superType)` /
    /// `resolveImport`, `_resolveSuperType`); qualifying `name` directly with
    /// `namespace` would build the alias's name in the target namespace,
    /// which does not exist there. So this always resolves through
    /// [`ModelManager::resolve_type_name`] (`ModelFile.getType`'s own path,
    /// PORTING.md 6.2), over `ti.name` alone, the same as the implicit
    /// `Concept`/`Asset`/… super type already does.
    pub(super) fn super_type_fqn(
        &self,
        class: &ClassLike<'_>,
        in_namespace: &str,
    ) -> Result<Option<String>> {
        let Some(ti) = class.super_type() else {
            return Ok(None);
        };
        // TS: ClassDeclaration._resolveSuperType passes `this.ast.location`
        // to every error it raises (src/introspect/classdeclaration.ts); the
        // class whose super type is being resolved is the AST node in scope
        // here, so its `location` is passed on, re-serialised from the typed
        // `mm::Range` by `location_value` (PORTING.md 2.1).
        // P5-48: the location is re-serialised only on an error path, not
        // on every (successful) step of a super-type walk.
        let location = || class.location().and_then(crate::error::location_value);
        match self.resolve_type_name_lazy(in_namespace, &ti.name, location) {
            Ok(fqn) => Ok(Some(fqn)),
            // TS: `_resolveSuperType`'s own hardcoded `IllegalModelException`
            // (src/introspect/classdeclaration.ts) — `resolve_type_name`'s
            // own failure is `TypeNotFound` (`ModelManager.getType`'s shape,
            // a different TS throw site), so it is remapped here, the same
            // way `validation.rs`'s `check_super_type` already raises this
            // exact message for the same TS call.
            Err(err) if err.is_pre_port_type_not_found() => Err(ContractError::pre_port(
                ErrorKind::IllegalModel,
                format!("Could not find super type {}", ti.name),
                location(),
            )
            .into()),
            Err(other) => Err(other),
        }
    }

    /// TS `BaseModelManager.derivesFrom(fqt1, fqt2)` (basemodelmanager.ts):
    /// `fqt1` must resolve (`this.getType(fqt1)`, propagated verbatim —
    /// `ModelManager::get_type_declaration` is the same lookup `getType`
    /// dispatches to); then true when `fqt1` and `fqt2` are the same type, or
    /// `fqt1` transitively extends it. [`ModelManager::is_assignable_to`]
    /// already walks exactly this chain (including the implicit `Concept`
    /// super type, P2-03), so this reuses it once `fqt1`'s own resolution is
    /// confirmed with `getType`'s error surface — `is_assignable_to`'s own
    /// lookup raises a different one (the pre-port `TypeNotFound` of `Error::type_not_found`, not
    /// the catalogued `IllegalModelException` `getType` raises).
    ///
    /// For a map declaration `fqt1` this is `true` against itself and
    /// otherwise `false`, where TS 5.0.0 throws a `TypeError` (its
    /// `MapDeclaration` has no `getSuperTypeDeclaration`): DV-022,
    /// maintainer-accepted. A scalar matches TS (its
    /// `getSuperTypeDeclaration()` is `null`).
    pub fn derives_from(&self, fqt1: &str, fqt2: &str) -> Result<bool> {
        self.get_type_declaration(fqt1)?;
        self.is_assignable_to(fqt1, fqt2)
    }
}
