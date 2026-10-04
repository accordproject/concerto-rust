//! Reading JS values and calling JS collaborators (PORTING.md 1.4).
//!
//! Split out of `lib.rs`; the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// JS values
// ---------------------------------------------------------------------------

/// A V8 `TypeError`, built through the catalogue.
pub(crate) fn type_error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::MalformedInput, code, params).into()
}

/// A catalogue `Error`, built the same way `type_error` builds a `TypeError`.
pub(crate) fn plain_error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::InvalidArgument, code, params).into()
}

/// JS `String(value)`.
pub(crate) fn js_string(value: &JsValue) -> Result<String> {
    if let Some(s) = value.as_string() {
        return Ok(s);
    }
    let string =
        Reflect::get(&js_sys::global(), &JsValue::from_str("String")).map_err(Error::Js)?;
    let string: Function = string.dyn_into().map_err(Error::Js)?;
    let text = string.call1(&JsValue::NULL, value).map_err(Error::Js)?;
    Ok(text.as_string().unwrap_or_default())
}

/// `value[name]`, with V8's error for a nullish `value`.
pub(crate) fn get(value: &JsValue, name: &str) -> Result<JsValue> {
    if value.is_undefined() || value.is_null() {
        let what = if value.is_undefined() {
            "undefined"
        } else {
            "null"
        };
        return Err(type_error(
            "engine-typeerror-readproperties",
            vec![("value", what.to_string()), ("property", name.to_string())],
        ));
    }
    if !value.is_object() && !value.is_function() {
        // A primitive receiver: none of the collaborator calls this backs reads
        // a property of one, so it reads as `undefined`.
        return Ok(JsValue::UNDEFINED);
    }
    Reflect::get(value, &JsValue::from_str(name)).map_err(Error::Js)
}

/// `value.name(...args)`; `expression` names the callee in a
/// "... is not a function" error.
pub(crate) fn call(
    value: &JsValue,
    name: &str,
    args: &[JsValue],
    expression: &str,
) -> Result<JsValue> {
    let method = get(value, name)?;
    let Some(method) = method.dyn_ref::<Function>() else {
        return Err(type_error(
            "engine-typeerror-notafunction",
            vec![("expression", expression.to_string())],
        ));
    };
    let list = Array::new();
    for arg in args {
        list.push(arg);
    }
    Reflect::apply(method, value, &list).map_err(Error::Js)
}

/// `value.name?.()`: `None` when the method is nullish. `value` itself is
/// read unguarded, matching every TS call site this backs (including
/// `MapValueType.validate`'s deliberately unguarded
/// `decl.isMapDeclaration?.()`): a nullish `value` throws the same
/// "Cannot read properties of null/undefined" `TypeError` TS's property
/// read would.
pub(crate) fn call_optional(value: &JsValue, name: &str) -> Result<Option<JsValue>> {
    let method = get(value, name)?;
    if method.is_undefined() || method.is_null() {
        return Ok(None);
    }
    call(value, name, &[], name).map(Some)
}

/// A JS value as JSON, `None` for `undefined`; `NaN`, `Infinity` and
/// functions are not modelled (no model AST holds one). An unpaired
/// surrogate escape, which `serde_json` rejects, becomes U+FFFD, so the
/// string fails its own check with that check's error.
pub(crate) fn to_json(value: &JsValue) -> Result<Option<Value>> {
    if value.is_undefined() {
        return Ok(None);
    }
    let text = JSON::stringify(value).map_err(Error::Js)?;
    let Some(text) = text.as_string() else {
        return Ok(None);
    };
    match serde_json::from_str(&text) {
        Ok(v) => Ok(Some(v)),
        Err(_) => {
            let sanitized = sanitize_lone_surrogate_escapes(&text);
            serde_json::from_str(&sanitized)
                .map(Some)
                .map_err(|e| internal(format!("to_json: {e}")))
        }
    }
}

/// Replaces each `\uXXXX` escape of an unpaired UTF-16 surrogate inside a
/// JSON string literal with the `�` escape, leaving pairs and other
/// escapes untouched. `JSON.stringify` output has no `\u` outside strings.
pub(crate) fn sanitize_lone_surrogate_escapes(text: &str) -> String {
    /// Reads a `\uXXXX` escape's 4 hex digits starting at `chars[at]`,
    /// returning the unit and its source characters, or `None` if `at` is
    /// out of range or the 4 characters there are not all hex digits.
    pub(crate) fn hex_unit(chars: &[char], at: usize) -> Option<(u32, &[char])> {
        let digits = chars.get(at..at + 4)?;
        let s: String = digits.iter().collect();
        Some((u32::from_str_radix(&s, 16).ok()?, digits))
    }

    /// Whether `chars[at..at + 2]` is a `\u` escape opener.
    pub(crate) fn is_u_escape(chars: &[char], at: usize) -> bool {
        chars.get(at..at + 2) == Some(['\\', 'u'].as_slice())
    }

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if !in_string {
            out.push(c);
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = false;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '\\'
            && is_u_escape(&chars, i)
            && let Some((unit, digits)) = hex_unit(&chars, i + 2)
        {
            if (0xD800..=0xDBFF).contains(&unit) {
                // High surrogate: valid only if immediately followed by a
                // low-surrogate escape.
                let low = is_u_escape(&chars, i + 6)
                    .then(|| hex_unit(&chars, i + 8))
                    .flatten();
                if let Some((l, _)) = low
                    && (0xDC00..=0xDFFF).contains(&l)
                {
                    out.push_str("\\u");
                    out.extend(digits);
                    i += 6;
                    continue;
                }
                out.push_str("\\uFFFD");
                i += 6;
                continue;
            }
            if (0xDC00..=0xDFFF).contains(&unit) {
                // A low surrogate reaching here was not just consumed as
                // the second half of a pair above, so it is unpaired on
                // its own.
                out.push_str("\\uFFFD");
                i += 6;
                continue;
            }
        }
        if let Some(&next) = (c == '\\').then(|| chars.get(i + 1)).flatten() {
            // Any other escape (`\\`, `\"`, `\n`, a non-surrogate `\uXXXX`,
            // …): copy the backslash and its one following character
            // through unchanged; the loop picks back up correctly whether
            // that was a simple escape or the first half of `\uXXXX`.
            out.push(c);
            out.push(next);
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

pub(crate) fn nullish(value: &JsValue) -> bool {
    value.is_undefined() || value.is_null()
}

/// `Option<bool>` as JS: `None` is `undefined`.
pub(crate) fn js_opt_bool(value: Option<bool>) -> JsValue {
    value.map_or(JsValue::UNDEFINED, JsValue::from_bool)
}

/// A string argument the TS member calls `method` on: a nullish value is
/// V8's property-read error, any other non-string a "not a function" error.
pub(crate) fn receiver(value: &JsValue, expression: &str, method: &str) -> Result<String> {
    if let Some(s) = value.as_string() {
        return Ok(s);
    }
    get(value, method)?;
    Err(type_error(
        "engine-typeerror-notafunction",
        vec![("expression", format!("{expression}.{method}"))],
    ))
}

// ---------------------------------------------------------------------------
// JS collaborator calls (PORTING.md 1.4)
// ---------------------------------------------------------------------------
//
// The ModelUtil predicates, scalar and decorator validation and the subclass
// queries answer from the manager's arena (BC-52; "Arena answers", below).
// These two helpers serve `MapKeyType.validate` and `MapValueType.validate`,
// which read the declaration `this.modelFile.getType(...)` returns.

/// TS `decl?.isScalarDeclaration?.()` and `decl?.isMapDeclaration?.()`:
/// `None` when the method is missing.
pub(crate) fn js_declaration_is(declaration: &JsValue, method: &str) -> Result<Option<bool>> {
    Ok(call_optional(declaration, method)?.map(|v| v.is_truthy()))
}

/// TS `ModelUtil.isValidMapKeyScalar(decl)` over the JS declaration
/// `MapKeyType.validate` resolved: `decl?.isScalarDeclaration?.() &&
/// decl?.ast.$class === <StringScalar> || <the same for DateTimeScalar>`,
/// keeping JS's `&&`/`||` result (`None`: JS `undefined`). The arena
/// answers the same question in [`mu::is_valid_map_key_scalar`].
pub(crate) fn js_is_valid_map_key_scalar(decl: Option<&JsValue>) -> Result<Option<bool>> {
    let side = |scalar: &str| -> Result<Option<bool>> {
        let Some(decl) = decl else {
            return Ok(None);
        };
        match js_declaration_is(decl, "isScalarDeclaration")? {
            Some(true) => {
                let class = get(&get(decl, "ast")?, "$class")?.as_string();
                Ok(Some(
                    class.as_deref() == Some(format!("concerto.metamodel@1.0.0.{scalar}").as_str()),
                ))
            }
            falsy => Ok(falsy),
        }
    };
    match side("StringScalar")? {
        Some(true) => Ok(Some(true)),
        _ => side("DateTimeScalar"),
    }
}
