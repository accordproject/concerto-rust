//! A [`ModelManagerHandle`](crate::ModelManagerHandle)'s staging slot, in a
//! module of its own, with its one capacity policy:
//! past [`StagedModelFiles::CAPACITY`] entries the oldest one is evicted,
//! whichever binding stages, and an evicted stage costs only time (its file
//! falls back to sending its AST). Staging never changes the manager, so it
//! never moves the epoch (the rule on `ModelManagerHandle::epoch`).

use std::collections::BTreeMap;
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
    next: u32,
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

    /// The id the next [`Self::insert`] gives.
    pub(crate) fn next_id(&self) -> u32 {
        self.next
    }

    pub(crate) fn insert(&mut self, file: ModelFile) -> u32 {
        self.insert_shared(Arc::new(file))
    }

    /// [`Self::insert`] for a model file that may also be held
    /// elsewhere: the file is shared, not copied.
    pub(crate) fn insert_shared(&mut self, file: Arc<ModelFile>) -> u32 {
        while self.files.len() >= Self::CAPACITY {
            if let Some((evicted, _)) = self.files.pop_first() {
                self.proofs.remove(&evicted);
            }
        }
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        self.files.insert(id, file);
        id
    }
}
