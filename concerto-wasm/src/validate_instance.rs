//! `validateInstance` (accordproject/concerto#1239).
//!
//! Split out of `lib.rs`; the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// accordproject/concerto#1239: validateInstance
// ---------------------------------------------------------------------------

/// The options [`diagnose`] reads from a `fromJSON` call's merged options,
/// as `Serializer.fromJSON` reads them: `utcOffset || 0`, the strict
/// boolean flags, the truthy `DeserializeOptions` flags, and the
/// validator's defaults as `ValidatedResource.validate` has them.
pub(crate) fn native_from_json_options(options: &Value) -> FromJsonOptions {
    let get = |key: &str| options.get(key);
    let utc_offset = match get("utcOffset") {
        v if !json_truthy(v) => UtcOffset::Number(0.0),
        Some(Value::String(s)) => UtcOffset::String(s.clone()),
        Some(Value::Number(n)) => UtcOffset::Number(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::Bool(_)) => UtcOffset::Number(1.0),
        _ => UtcOffset::Number(f64::NAN),
    };
    FromJsonOptions {
        validate: json_truthy(get("validate")),
        utc_offset,
        strict_qualified_date_times: get("strictQualifiedDateTimes") == Some(&Value::Bool(true)),
        accept_resources_for_relationships: get("acceptResourcesForRelationships")
            == Some(&Value::Bool(true)),
        reject_unknown_keys: json_truthy(get("rejectUnknownKeys")),
        reject_required_null: json_truthy(get("rejectRequiredNull")),
        validator: ValidateOptions::default(),
    }
}

/// Diagnostics as the plain objects the TS layer hands its callers:
/// `{code, path, expected?, severity, message}`.
pub(crate) fn diagnostics_json(diagnostics: &[Diagnostic]) -> Value {
    Value::Array(
        diagnostics
            .iter()
            .map(|d| {
                let mut out = serde_json::Map::new();
                out.insert("code".into(), json!(d.code.as_str()));
                out.insert("path".into(), json!(d.pointer));
                if let Some(expected) = &d.expected {
                    out.insert("expected".into(), json!(expected));
                }
                let severity = if d.severity == Severity::Warning {
                    "warning"
                } else {
                    "error"
                };
                out.insert("severity".into(), json!(severity));
                out.insert("message".into(), json!(d.message));
                Value::Object(out)
            })
            .collect(),
    )
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// accordproject/concerto#1239 `validateInstance`: validates `json_text`
    /// (`fromJSON`'s wire encoding) as `Serializer.fromJSON` with `validate:
    /// true` and `options_text` would, as `fqn` when given. Plain JSON takes
    /// the native walk ([`diagnose`]); a tagged document takes
    /// `serializerFromJsonCompact`'s verdict, with the walk's report where it
    /// raises the same error ([`diagnose_read`]). `mode` 0 throws with the
    /// diagnostics as `details` (`""` when valid); 1 returns
    /// `{"diagnostics": [...]}` for the first error, 2 for every violation.
    #[wasm_bindgen(js_name = validateInstance)]
    pub fn validate_instance(
        &self,
        json_text: &str,
        options_text: &str,
        fqn: Option<String>,
        mode: u32,
    ) -> JsResult<String> {
        run(|| {
            let wire = serde_json::from_str::<Value>(json_text).map_err(json_syntax)?;
            // The options are read once per options text, and the serializer built
            // from them reused, as `serializerFromJsonCompact` reuses them
            // ([`with_serializer_options`]).
            with_serializer_options(options_text, |entry| {
                validate_wire(
                    &self.manager,
                    &wire,
                    &entry.serializer,
                    &entry.from_json,
                    &entry.native,
                    fqn.as_deref(),
                    mode,
                )
            })?
        })
    }
}

/// [`ModelManagerHandle::validate_instance`]'s check of the wire document
/// `wire` on `manager`, with `serializer` and the merged options as
/// `from_json` (`from_json`) and the walk (`options`) read them; shared
/// with `validateMetaModelInstance`.
pub(crate) fn validate_wire(
    manager: &ModelManager,
    wire: &Value,
    serializer: &Serializer,
    from_json: &FromJsonOptions,
    options: &FromJsonOptions,
    fqn: Option<&str>,
    mode: u32,
) -> Result<String> {
    let diagnosis = if has_wire_tag(wire) {
        // Not plain JSON: read by `Serializer.fromJSON`'s own engine, from
        // the same decoded document, for the verdict and the error; the
        // walk reads its validator form.
        let object = decode_wire(wire)?;
        let readings = validator_readings(&object);
        diagnose_read(manager, fqn, &readings, options, mode == 2, || {
            serializer
                .from_json_prepared(
                    manager,
                    &with_class(object, fqn),
                    from_json,
                    &mut ValidationEnv,
                )
                .map(|_| ())
        })
    } else {
        diagnose(manager, fqn, wire, options, mode == 2)
    };
    let diagnostics = diagnostics_json(diagnosis.report.diagnostics());
    if mode == 0 {
        return match diagnosis.error {
            Some(err) => Err(Error::Instance(Box::new(err.into_contract()), diagnostics)),
            None => Ok(String::new()),
        };
    }
    snapshot(&json!({ "diagnostics": diagnostics }))
}

/// Calls back the view's `env.newId()`/`env.nowMs()` (the identifier and
/// the clock stay with the caller, `InstanceEnv`'s doc). Both trait
/// methods are infallible, so a callback that throws or returns the wrong
/// type is reported as best it can be (an empty id, or `0`) rather than
/// propagated: a real `Factory.newId`/clock never does either.
pub(crate) struct JsInstanceEnv {
    pub(crate) env: JsValue,
}

impl InstanceEnv for JsInstanceEnv {
    fn new_id(&mut self) -> String {
        call(&self.env, "newId", &[], "env.newId")
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default()
    }

    fn now_ms(&mut self) -> f64 {
        call(&self.env, "nowMs", &[], "env.nowMs")
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    }
}
