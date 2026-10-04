//! A [`ModelManagerHandle`](crate::ModelManagerHandle)'s staging slot, in a
//! module of its own, with its one capacity policy:
//! past [`StagedModelFiles::CAPACITY`] entries the oldest one is evicted,
//! whichever binding stages, and an evicted stage costs only time (its file
//! falls back to sending its AST). Staging never changes the manager, so it
//! never moves the epoch (the rule on `ModelManagerHandle::epoch`).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use concerto_core::ModelFile;
use concerto_core::model_manager::ValidityProof;

/// The staging slot of a [`crate::ModelManagerHandle`]: files loaded once by
/// [`crate::ModelManagerHandle::stage_model_file`], kept shared (`Arc`) until
/// the view registers, validates or drops them. Staging never moves the
/// epoch. Past [`StagedModelFiles::CAPACITY`] the oldest entry is evicted;
/// an evicted stage gets `undefined` and the view sends the AST again.
#[derive(Default)]
pub(crate) struct StagedModelFiles {
    pub(crate) files: BTreeMap<u32, Arc<ModelFile>>,
    /// The source manager's [`ValidityProof`] of a file staged shared by
    /// [`crate::ModelManagerHandle::model_file_filter_staged`], by stage id;
    /// registered with the file
    /// ([`crate::ModelManagerHandle::commit_staged_model_file`]).
    pub(crate) proofs: BTreeMap<u32, Arc<ValidityProof>>,
    /// The stage ids of the files staged shared by
    /// [`crate::ModelManagerHandle::model_file_filter_staged`]: each is read,
    /// once registered, in TS 5.0.0's filtered form
    /// (`ModelManager::read_in_filtered_form`), and its AST checked in that
    /// form.
    pub(crate) filtered: BTreeSet<u32>,
    pub(crate) next: u32,
}

/// [`crate::ModelManagerHandle::stage_model_file_bytes`]'s flag: BC-19's AST
/// shape check is folded into the load.
pub(crate) const STAGE_CHECKED: u32 = 1;
/// [`crate::ModelManagerHandle::stage_model_file_bytes`]'s flag: the bytes are the
/// AST in the compact binary layout, not its JSON text as UTF-8.
pub(crate) const STAGE_COMPACT: u32 = 2;

impl StagedModelFiles {
    /// The most staged files kept at once.
    pub(crate) const CAPACITY: usize = 256;

    pub(crate) fn insert(&mut self, file: ModelFile) -> u32 {
        self.insert_shared(Arc::new(file))
    }

    /// [`Self::insert`] for a model file that may also be held
    /// elsewhere: the file is shared, not copied.
    pub(crate) fn insert_shared(&mut self, file: Arc<ModelFile>) -> u32 {
        self.insert_shared_in_batch(file, 0)
    }

    /// [`Self::insert_shared`] for one file of a batch of `batch` files
    /// staged together (a DecoratorManager result): the oldest entry is
    /// evicted only past the larger of [`Self::CAPACITY`] and `batch`, so a
    /// batch larger than the slot never evicts its own earlier files. A
    /// later insert evicts back down to the capacity.
    pub(crate) fn insert_shared_in_batch(&mut self, file: Arc<ModelFile>, batch: usize) -> u32 {
        while self.files.len() >= Self::CAPACITY.max(batch) {
            let Some(evicted) = self.oldest() else {
                break;
            };
            self.files.remove(&evicted);
            self.forget(evicted);
        }
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        self.files.insert(id, file);
        id
    }

    /// Takes what is kept with the file staged under `stage` (it is being
    /// registered, or is gone): its source manager's proof, and whether it
    /// is read in TS's filtered form.
    pub(crate) fn take_extras(&mut self, stage: u32) -> (Option<Arc<ValidityProof>>, bool) {
        (self.proofs.remove(&stage), self.filtered.remove(&stage))
    }

    /// Drops what is kept with the file staged under `stage`
    /// ([`Self::take_extras`]).
    pub(crate) fn forget(&mut self, stage: u32) {
        self.take_extras(stage);
    }

    /// The AST of the file staged under `stage` as it is read once
    /// registered: TS 5.0.0's filtered form for a file staged by `filter`,
    /// else its own.
    pub(crate) fn read_ast<'a>(
        &self,
        stage: u32,
        file: &'a ModelFile,
    ) -> Cow<'a, concerto_core::json::Value> {
        match self.filtered.contains(&stage).then(|| file.filtered_ast()) {
            Some(Some(filtered)) => Cow::Owned(filtered),
            _ => Cow::Borrowed(file.ast()),
        }
    }

    /// The id staged longest ago. Ids are handed out in order but wrap
    /// around, so the oldest is the one furthest behind the next id, not
    /// the smallest.
    fn oldest(&self) -> Option<u32> {
        self.files
            .keys()
            .copied()
            .max_by_key(|id| self.next.wrapping_sub(*id))
    }
}
