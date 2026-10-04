//! The manager's per-declaration answer cache ([`DeclCache`]) and its
//! validity marks and proofs ([`ValidityProof`]).

use super::{
    Arc, ClassInfo, DeclId, EXCLUDE_NS, ManagerOptions, ModelFile, ModelFileId, ModelManager,
    Mutex, Result, SeededHashSet,
};

js_compat_pub! {
    /// Why a model file shared into another manager
    /// ([`ModelManager::add_shared_model_file_with_proof`]) is valid there
    /// without validating it again: it passed validation with these options,
    /// and these are every namespace it reaches with the file the source held.
    /// Validity reads nothing else, which [`ModelManager::validate_models`]
    /// checks.
    #[derive(Debug)]
    pub struct ValidityProof {
        options: ManagerOptions,
        closure: Box<[(Box<str>, Arc<ModelFile>)]>,
    }
}

/// Locks `mutex`, recovering it when a thread panicked while holding it:
/// every cache under a lock only ever holds whole answers, so a poisoned
/// one is still consistent.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One declaration's cached answers, each computed on first use.
#[derive(Debug, Clone, Default)]
pub(super) struct DeclFacts {
    /// Its inheritance chain and properties; only a successful answer.
    pub(super) class: Option<Arc<ClassInfo>>,
    /// Its validation plan ([`crate::instance::plan`]); `Some(None)`
    /// records a declaration with no plan.
    pub(super) plan: Option<Option<Arc<crate::instance::plan::ClassPlan>>>,
    /// Its converted field defaults
    /// ([`crate::instance::from_json::assign_field_defaults_of`]); only a
    /// successful answer.
    pub(super) field_defaults: Option<Arc<crate::instance::from_json::FieldDefaults>>,
    /// The declarations that directly extend it (TS `getDirectSubclasses`),
    /// in load order, filled for every declaration at once.
    pub(super) direct_subclasses: Option<Arc<[DeclId]>>,
    /// For a map declaration, what the populators resolve for its entries
    /// ([`crate::instance::model::map_entries`]).
    pub(super) map_entries: Option<Arc<crate::instance::model::MapEntries>>,
}

/// The per-declaration cache of a [`ModelManager`]: one slot of
/// [`DeclFacts`] per declaration handle, under one lock (a `Mutex` rather
/// than a `RefCell`, so the manager stays `Sync`).
#[derive(Debug, Default)]
pub(super) struct DeclCache(Mutex<Vec<DeclFacts>>);

impl DeclCache {
    /// The answer `field` selects for declaration `id`, or `compute`'s,
    /// which is kept when it succeeds. The lock is not held while
    /// computing, so `compute` may read this cache again.
    pub(super) fn get_or_try_insert_with<T: Clone, E>(
        &self,
        id: DeclId,
        field: fn(&mut DeclFacts) -> &mut Option<T>,
        compute: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        if let Some(answer) = lock(&self.0)
            .get_mut(id.slot())
            .and_then(|facts| field(facts).clone())
        {
            return Ok(answer);
        }
        let answer = compute()?;
        let mut cache = lock(&self.0);
        if cache.len() <= id.slot() {
            cache.resize_with(id.slot() + 1, DeclFacts::default);
        }
        *field(&mut cache[id.slot()]) = Some(answer.clone());
        Ok(answer)
    }

    /// A copy of every answer, for a fork ([`ModelManager::fork`]).
    #[cfg(feature = "js-compat")]
    pub(super) fn snapshot(&self) -> Self {
        Self(Mutex::new(lock(&self.0).clone()))
    }

    /// Every answer, for the caller to edit in place.
    pub(super) fn facts_mut(&mut self) -> &mut Vec<DeclFacts> {
        self.0
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The answer `field` selects for declaration `id`, if one is cached.
    pub(super) fn get<T: Clone>(
        &self,
        id: DeclId,
        field: fn(&DeclFacts) -> &Option<T>,
    ) -> Option<T> {
        lock(&self.0)
            .get(id.slot())
            .and_then(|facts| field(facts).clone())
    }

    /// Stores `answers[i]` as the answer `field` selects for the declaration
    /// of slot `i`, for every slot `answers` covers.
    pub(super) fn fill<T>(&self, field: fn(&mut DeclFacts) -> &mut Option<T>, answers: Vec<T>) {
        let mut cache = lock(&self.0);
        if cache.len() < answers.len() {
            cache.resize_with(answers.len(), DeclFacts::default);
        }
        for (facts, answer) in cache.iter_mut().zip(answers) {
            *field(facts) = Some(answer);
        }
    }

    /// Reads every answer under the lock.
    #[cfg(any(test, feature = "validation-plan-testing"))]
    pub(super) fn read<R>(&self, read: impl FnOnce(&[DeclFacts]) -> R) -> R {
        read(&lock(&self.0))
    }
}

impl ModelManager {
    js_compat_pub! {
        /// Why the model file registered under `namespace` is valid in any
        /// manager holding it with the same options and files
        /// ([`ValidityProof`]), or `None` when it has not been validated here
        /// or reaches a namespace this manager does not hold.
        pub fn validity_proof(&self, namespace: &str) -> Option<Arc<ValidityProof>> {
            let id = *self.namespaces.get(namespace)?;
            if !self.known_valid(id) {
                return None;
            }
            // The namespaces the file reaches: its own, the system models
            // (every file's implicit import) and the transitive closure of
            // its imports, each with the file held under it.
            let mut closure: Vec<(Box<str>, Arc<ModelFile>)> = Vec::new();
            let mut seen: SeededHashSet<&str> = SeededHashSet::default();
            let mut pending: Vec<&str> = vec![namespace];
            pending.extend(EXCLUDE_NS.iter().copied().filter(|ns| self.namespaces.contains_key(*ns)));
            while let Some(ns) = pending.pop() {
                if !seen.insert(ns) {
                    continue;
                }
                let slot = &self.files[self.namespaces.get(ns)?.slot()];
                for import in slot.model_file.imports() {
                    pending.push(import.namespace());
                }
                closure.push((Box::from(ns), Arc::clone(&slot.model_file)));
            }
            Some(Arc::new(ValidityProof {
                options: self.options.clone(),
                closure: closure.into_boxed_slice(),
            }))
        }
    }

    /// Whether the file `id` may be taken as valid without validating it:
    /// it passed validation in this manager, or it carries a
    /// [`ValidityProof`] that holds here (then it is marked validated).
    pub(crate) fn known_valid(&self, id: ModelFileId) -> bool {
        use std::sync::atomic::Ordering;
        let Some(slot) = self.files.get(id.slot()) else {
            return false;
        };
        if slot.validated.load(Ordering::Relaxed) {
            return true;
        }
        let Some(proof) = &slot.proof else {
            return false;
        };
        let holds = self.proof_holds(proof);
        if holds {
            slot.validated.store(true, Ordering::Relaxed);
        }
        holds
    }

    /// Whether `proof` holds in this manager: the same options, and the
    /// very same file under each namespace it names.
    pub(super) fn proof_holds(&self, proof: &ValidityProof) -> bool {
        proof.options == self.options
            && proof.closure.iter().all(|(ns, file)| {
                self.namespaces
                    .get(&**ns)
                    .and_then(|id| self.files.get(id.slot()))
                    .is_some_and(|slot| Arc::ptr_eq(&slot.model_file, file))
            })
    }

    /// Records that the file `id` passed validation in this manager.
    pub(crate) fn mark_validated(&self, id: ModelFileId) {
        if let Some(slot) = self.files.get(id.slot()) {
            slot.validated
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Every file's validated mark, for a batch to restore when it rolls
    /// back ([`ModelManager::restore_validated`]).
    pub(super) fn validated_marks(&self) -> Vec<bool> {
        self.files
            .iter()
            .map(|slot| slot.validated.load(std::sync::atomic::Ordering::Relaxed))
            .collect()
    }

    /// Puts back the marks [`ModelManager::validated_marks`] took (a file
    /// validated while a rolled-back batch was registered may have reached
    /// one of its files).
    pub(super) fn restore_validated(&mut self, marks: &[bool]) {
        for (slot, mark) in self.files.iter_mut().zip(marks) {
            *slot.validated.get_mut() = *mark;
        }
    }

    /// Forgets every validated mark (an option changed).
    #[cfg(feature = "js-compat")]
    pub(super) fn clear_validated(&mut self) {
        for slot in &mut self.files {
            *slot.validated.get_mut() = false;
        }
    }

    /// Declaration `id`'s converted field defaults
    /// ([`crate::instance::from_json::assign_field_defaults_of`]):
    /// `compute`'s answer, computed on first use and then cached until the
    /// registered files change, as the inheritance facts are. An error is
    /// returned, and not cached.
    pub(crate) fn cached_field_defaults(
        &self,
        id: DeclId,
        compute: impl FnOnce() -> Result<crate::instance::from_json::FieldDefaults>,
    ) -> Result<Arc<crate::instance::from_json::FieldDefaults>> {
        self.decl_cache.get_or_try_insert_with(
            id,
            |facts| &mut facts.field_defaults,
            || compute().map(Arc::new),
        )
    }

    /// Map declaration `id`'s resolved entries
    /// ([`crate::instance::model::map_entries`]): `compute`'s answer,
    /// computed on first use and then cached until the registered files
    /// change.
    pub(crate) fn cached_map_entries(
        &self,
        id: DeclId,
        compute: impl FnOnce() -> crate::instance::model::MapEntries,
    ) -> Arc<crate::instance::model::MapEntries> {
        let cached: std::result::Result<_, std::convert::Infallible> =
            self.decl_cache.get_or_try_insert_with(
                id,
                |facts| &mut facts.map_entries,
                || Ok(Arc::new(compute())),
            );
        match cached {
            Ok(entries) => entries,
        }
    }

    /// The caches an append ([`ModelManager::insert_shared`]) leaves valid: an
    /// append changes no registered file, so only answers built from
    /// successful resolutions are kept (inheritance chains, field defaults,
    /// settled validation plans and map entries); direct subclasses are
    /// dropped. So a fork of
    /// a warmed manager stays warm. Any other change drops everything
    /// ([`ModelManager::invalidate_caches`]).
    pub(super) fn keep_caches_for_append(&mut self) {
        let declarations = &self.declarations;
        let files = &self.files;
        for (slot, facts) in self.decl_cache.facts_mut().iter_mut().enumerate() {
            if !matches!(&facts.plan, Some(Some(plan)) if plan.is_settled()) {
                facts.plan = None;
            }
            facts.direct_subclasses = None;
            if let Some(entries) = &facts.map_entries {
                let settled = declarations
                    .get(slot)
                    .and_then(|d| {
                        files
                            .get(d.model_file.slot())?
                            .model_file
                            .declarations()
                            .get(d.index)
                    })
                    .is_some_and(|decl| entries.is_settled(decl));
                if !settled {
                    facts.map_entries = None;
                }
            }
        }
    }

    /// Drops every answer cached from the registered files;
    /// called by every change to them but an append
    /// ([`ModelManager::keep_caches_for_append`]).
    pub(super) fn invalidate_caches(&mut self) {
        self.decl_cache.facts_mut().clear();
    }

    /// Declaration `id`'s validation plan ([`crate::instance::plan`]), built
    /// by `build` on first use and cached until the registered files
    /// change. The lock is not held while building.
    pub(crate) fn cached_plan(
        &self,
        id: DeclId,
        build: impl FnOnce() -> Option<crate::instance::plan::ClassPlan>,
    ) -> Option<Arc<crate::instance::plan::ClassPlan>> {
        let built: std::result::Result<_, std::convert::Infallible> = self
            .decl_cache
            .get_or_try_insert_with(id, |facts| &mut facts.plan, || Ok(build().map(Arc::new)));
        match built {
            Ok(plan) => plan,
        }
    }

    /// The number of validation plans cached, and of their properties: a
    /// test- and dev-only measure (`validation-plan-testing`).
    #[cfg(any(test, feature = "validation-plan-testing"))]
    pub(crate) fn plan_cache_stats(&self) -> (usize, usize) {
        self.decl_cache.read(|facts| {
            facts
                .iter()
                .filter_map(|f| f.plan.as_ref())
                .flatten()
                .fold((0, 0), |(n, p), plan| (n + 1, p + plan.props.len()))
        })
    }

    /// The number of cached inheritance chains, instance facts and
    /// validation plans (a declaration recorded as having none included): a
    /// test-only measure.
    #[cfg(test)]
    pub(crate) fn cache_counts(&self) -> (usize, usize, usize) {
        self.decl_cache.read(|facts| {
            (
                facts.iter().filter(|f| f.class.is_some()).count(),
                facts.iter().filter(|f| f.field_defaults.is_some()).count(),
                facts.iter().filter(|f| f.plan.is_some()).count(),
            )
        })
    }
}
