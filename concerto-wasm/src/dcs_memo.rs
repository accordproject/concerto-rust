//! A [`ModelManagerHandle`](crate::ModelManagerHandle)'s per-epoch
//! DecoratorManager extract memo (P5-56; P5-77), in a module of its own
//! (P5-101, D-7, accordproject/concerto-rust#455). The memo is a `RefCell`
//! field of the handle, so the extract bindings keep taking `&self`; it never
//! moves the epoch, and is dropped whenever the epoch moves (the rule on
//! `ModelManagerHandle::epoch`).

use super::*;

// ---------------------------------------------------------------------------
// P5-56 (T2, F-A2, accordproject/concerto-rust#377): a per-epoch extract
// result memo on the source handle (the P5-42 report's Design 2, on #352).
//
// With `removeDecoratorsFromModel` false, an extract's result models are the
// handle's own models, resolved, whatever the action and locale; only the
// command sets and vocabularies depend on those. So while the epoch is
// unchanged, a repeated `dcsExtract*` call reuses the result manager, its
// encoded AST and its staged headers, and rebuilds only the command sets
// and vocabularies from the kept source models
// ([`dcs::encode_extract_source`]): no resolve, no result-manager build,
// validation or drop, no AST encode.
//
// - Filled on the second call at the same epoch (and system flag), so a
//   one-shot caller never pays for it; the first call only notes its key.
// - Dropped when the epoch moves ([`ModelManagerHandle::bump_epoch`]), on
//   [`ModelManagerHandle::drop_dcs_memo`] and with the handle.
// - Errors are never memoised: a call that throws leaves no memo, and the
//   next call runs in full. A memo exists only after a call whose result
//   models loaded and validated, which is the only error that
//   `dcs::extract` reports ahead of the transform's, so a repeated call
//   throws what a full one throws.
// - Nothing shared is returned: the JS result is parsed from new text on
//   every call. P5-77: each staged model file is shared with the kept
//   result manager (a model file never changes once built), not cloned.
//
// P5-77 (accordproject/concerto-rust#419): with `removeDecoratorsFromModel`
// true, the result models are the handle's own models, resolved, with the
// decorators the action strips removed: they depend on the action, but not
// on the locale, and the command sets and vocabularies are read before any
// decorator is stripped ([`dcs::encode_extract_source`]). So the same memo
// serves that case too, keyed by the action as well; everything above
// holds for it unchanged.
// ---------------------------------------------------------------------------

/// A [`ModelManagerHandle`]'s extract memo (P5-56): its key, and the kept
/// result once the second call at that key has filled it.
pub(crate) struct DcsExtractMemo {
    /// `(epoch, system models walked, stripping action)`: `ExtractAll` and
    /// `ExtractVocab` walk the system models too, `ExtractNonVocab` does not
    /// ([`dcs::extract`]); the stripping action is the action when
    /// `removeDecoratorsFromModel` is true (P5-77), and `None` when it is
    /// false, since then every action gives the same result models.
    pub(crate) key: (u64, bool, Option<dcs::extractor::Action>),
    /// `None` after the first call at `key`, `Some` from the second on.
    pub(crate) kept: Option<DcsExtractKept>,
}

/// The `{"$class", "models"}` envelope of a manager's model ASTs
/// (`model_manager_to_ast`'s), from each model's own AST text
/// ([`ModelManager::compact_model_asts`]): the same compact text, byte for
/// byte (P5-77).
fn models_envelope_text(texts: &[Arc<str>]) -> String {
    let mut out = String::with_capacity(64 + texts.iter().map(|t| t.len() + 1).sum::<usize>());
    out.push_str("{\"$class\":\"concerto.metamodel@1.0.0.Models\",\"models\":[");
    for (i, text) in texts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(text);
    }
    out.push_str("]}");
    out
}

/// P5-77: stages an extract result into `target` and returns its JS value,
/// for a result the caller drops once the call returns, through
/// [`DcsExtractKept`], so the files staged into `target` keep their ASTs as
/// text ([`DcsExtractKept::new`]). The same JS value, and the same stages,
/// as [`stage_result`] gives.
pub(crate) fn compacted_extract_js(
    target: &mut ModelManagerHandle,
    result: dcs::extractor::ExtractResult,
) -> Result<(JsValue, DcsExtractKept)> {
    let dcs::extractor::ExtractResult {
        model_manager,
        decorator_command_set,
        vocabularies,
        source_models,
    } = result;
    let kept = DcsExtractKept::new(source_models.unwrap_or_default(), model_manager)?;
    let staged = kept.stage(target);
    let js = kept.result_js(&decorator_command_set, &vocabularies, &staged)?;
    Ok((js, kept))
}

/// What a repeated extract at the same key reuses.
pub(crate) struct DcsExtractKept {
    /// The resolved source models the extractor walks.
    pub(crate) source: Vec<Value>,
    /// The result manager, staged from on every call.
    pub(crate) result: ModelManager,
    /// The JSON text of the result manager's AST ([`models_envelope_text`]).
    pub(crate) ast_text: String,
    /// The staged header of each of `result`'s model files, in order
    /// ([`staged_header_from_parts`]).
    pub(crate) headers: Vec<Option<StagedHeader<'static>>>,
}

impl DcsExtractKept {
    /// P5-77 (accordproject/concerto-rust#419): the staged headers are read
    /// from the result's parsed ASTs first, then the ASTs are compacted
    /// ([`ModelManager::compact_model_asts`]): each result model file keeps
    /// its AST as compact JSON text, and [`Self::ast_text`] is spliced
    /// from those texts, byte for byte the text of `model_manager_to_ast`'s
    /// value. So the files staged from it (shared, see
    /// [`Self::stage`]) hold text, not a parsed tree, for as long as the
    /// result ModelManager lives. Compaction serialises values serde built,
    /// which cannot fail; should it, the error is [`internal`] (P5-104,
    /// D-11: no intermediate-`Value` fallback).
    pub(crate) fn new(source: Vec<Value>, mut result: ModelManager) -> Result<Self> {
        let headers = result
            .model_files()
            .map(|mf| {
                staged_header_from_parts(mf.namespace(), mf.ast().get("imports"))
                    .map(StagedHeader::into_owned)
            })
            .collect();
        let ast_text = models_envelope_text(&result.compact_model_asts().map_err(internal)?);
        Ok(Self {
            source,
            result,
            ast_text,
            headers,
        })
    }

    /// [`stage_shared`] from the kept result manager, with its kept headers.
    /// P5-77: each file is staged shared with the kept manager, which never
    /// changes, so a repeated extract copies no model file.
    pub(crate) fn stage(&self, target: &mut ModelManagerHandle) -> Vec<Value> {
        stage_shared(
            target,
            self.result
                .shared_model_files()
                .zip(self.headers.iter().map(Option::as_ref)),
        )
    }

    /// The JSON text of the extract result (`{modelManager,
    /// decoratorCommandSet, vocabularies, staged, validated}`) for the kept
    /// result and this call's command sets and vocabularies, with the AST
    /// spliced in from [`Self::ast_text`].
    pub(crate) fn result_text(
        &self,
        command_sets: &str,
        vocabularies: &[String],
        staged: &[Value],
    ) -> serde_json::Result<String> {
        let mut out = Vec::new();
        out.extend_from_slice(b"{\"modelManager\":");
        out.extend_from_slice(self.ast_text.as_bytes());
        out.extend_from_slice(b",\"decoratorCommandSet\":");
        out.extend_from_slice(command_sets.as_bytes());
        out.extend_from_slice(b",\"vocabularies\":");
        serde_json::to_writer(&mut out, vocabularies)?;
        out.extend_from_slice(b",\"staged\":");
        serde_json::to_writer(&mut out, staged)?;
        out.extend_from_slice(b",\"validated\":true}");
        String::from_utf8(out).map_err(serde::ser::Error::custom)
    }

    /// The JS value of the extract result: [`Self::result_text`], parsed.
    /// The text is written from values serde built, so neither step fails
    /// (P5-104, D-11: the intermediate-`Value` fallback is gone).
    fn result_js(
        &self,
        command_sets: &str,
        vocabularies: &[String],
        staged: &[Value],
    ) -> Result<JsValue> {
        let text = self
            .result_text(command_sets, vocabularies, staged)
            .map_err(internal)?;
        JSON::parse(&text).map_err(Error::Js)
    }
}

impl ModelManagerHandle {
    /// [`staged_extract`] on this handle's own manager, through the
    /// per-epoch memo (P5-56; P5-77 for `removeDecoratorsFromModel` true).
    pub(crate) fn memo_extract(
        &self,
        target: &mut ModelManagerHandle,
        options: &JsValue,
        action: dcs::extractor::Action,
    ) -> Result<JsValue> {
        let options_json = to_json(options)?.unwrap_or_else(|| json!({}));
        let opts = extract_options_from_js(&options_json);
        let key = (
            self.epoch,
            action != dcs::extractor::Action::ExtractNonVocab,
            opts.remove_decorators_from_model.then_some(action),
        );
        let mut memo = self.dcs_memo.borrow_mut();
        match memo.as_mut() {
            Some(DcsExtractMemo {
                key: memo_key,
                kept: Some(kept),
            }) if *memo_key == key => {
                let (command_sets, vocabularies) =
                    dcs::encode_extract_source(&kept.source, &opts, action)?;
                let staged = kept.stage(target);
                kept.result_js(&command_sets, &vocabularies, &staged)
            }
            Some(DcsExtractMemo {
                key: memo_key,
                kept: kept @ None,
            }) if *memo_key == key => {
                let result = dcs::extract(&self.manager, &opts, action, true)?;
                let (js, filled) = compacted_extract_js(target, result)?;
                *kept = Some(filled);
                Ok(js)
            }
            _ => {
                *memo = Some(DcsExtractMemo { key, kept: None });
                drop(memo);
                let result = dcs::extract(&self.manager, &opts, action, false)?;
                Ok(compacted_extract_js(target, result)?.0)
            }
        }
    }
}
