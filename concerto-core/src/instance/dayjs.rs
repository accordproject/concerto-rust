//! The `dayjs` values an instance holds in its `DateTime` fields and its
//! `$timestamp` (PORTING.md 3.3), modelled closely enough to reproduce what
//! `JSONPopulator`, `JSONGenerator`, `Typed.assignFieldDefaults` and
//! `Factory.newResource` do with them, and what the oracle records of them
//! (`codec.js`: `{valid, iso, offset, utc}`).
//!
//! D7 keeps the *construction* of dayjs objects in TS: on the WASM fast
//! path Rust receives and returns `(epoch ms, utcOffset minutes)`. The
//! native harness has no TS, so this module stands in for the handful of
//! dayjs 1.11.10 operations (with its `utc` plugin, `dayjs-setup.ts`) the
//! ported members call, under `TZ=UTC` (3.3: every test runs with it, and
//! Rust never consults the system time zone). Under `TZ=UTC` a "local"
//! dayjs and a UTC one read the same calendar fields, so only the state
//! the `utc` plugin keeps differs.
//!
//! The state is dayjs's own:
//!
//! - `$d`, the JS `Date` it wraps, as its time value in ms (`NaN` when
//!   invalid). The `utc` plugin's `utcOffset(n)` *shifts* `$d` by `n`
//!   minutes (`this.local().add(offset + localTimezoneOffset, 'minute')`)
//!   and records `$offset`, so that the calendar fields `format` reads are
//!   the offset's local time; its `valueOf` takes the shift back off, so
//!   `toDate()`, `toISOString()` and `.utc()` see the original instant.
//! - `$u` (`isUTC()`), `$offset` and `$x.$localOffset`.
//!
//! Parsing is *not* dayjs's (P5-24, accordproject/concerto-rust#328): a
//! `DateTime` string is accepted only in the strict ISO 8601 / RFC 3339
//! form (the `strictQualifiedDateTimes` regex, then chrono's calendar
//! checks, BC-07 and BC-42 in R1), on every path that reads one: fields,
//! map values and model defaults. The emulation of dayjs's lenient
//! `parseDate` and of V8's `Date.parse` is gone; what stays is the value
//! model above and the output formatting (`format_json`, `to_iso_string`,
//! `to_js_string`).

use std::sync::LazyLock;

use crate::ecma;

/// Milliseconds in a minute (`MILLISECONDS_A_MINUTE`).
const MS_PER_MINUTE: f64 = 60_000.0;
const MS_PER_DAY: f64 = 86_400_000.0;

/// A dayjs object (the `dayjs-setup.ts` build: dayjs 1.11.10 plus `utc`).
#[derive(Debug, Clone, PartialEq)]
pub struct Dayjs {
    /// `$d.getTime()`: `NaN` for an invalid date.
    time: f64,
    /// `$u`.
    utc: bool,
    /// `$offset`, in minutes, when the `utc` plugin set one.
    offset: Option<f64>,
    /// `$x.$localOffset`.
    local_offset: Option<f64>,
}

/// What `utcOffset(input)` is given: a number of minutes (or hours, when
/// `|n| <= 16`), or a `±HH:mm` string.
#[derive(Debug, Clone, PartialEq)]
pub enum UtcOffset {
    /// A number of minutes, or of hours when `|n| <= 16`.
    Number(f64),
    /// A `±HH:mm` offset string.
    String(String),
}

impl Dayjs {
    /// A dayjs over an arbitrary `$d` time value.
    fn with_time(time: f64, utc: bool) -> Self {
        Self {
            time,
            utc,
            offset: None,
            local_offset: None,
        }
    }

    /// `dayjs.utc()` at the time value `now_ms` (TS reads the clock; the
    /// caller supplies it, D7).
    pub fn utc_now(now_ms: f64) -> Self {
        Self::with_time(now_ms, true)
    }

    /// A `DateTime` string read with the strict rule (P5-24, BC-07,
    /// accordproject/concerto-rust#328): the `strictQualifiedDateTimes`
    /// format, naming a real calendar instant (`strict_instant`). Anything
    /// else is an invalid date, where TS's `dayjs.utc(date)` used to parse
    /// leniently (DIVERGENCES.md DV-009).
    pub fn utc_parse(s: &str) -> Self {
        Self::with_time(strict_instant(s), true)
    }

    /// `dayjs.utc(n)` for a number: `new Date(n)`.
    pub fn utc_from_number(n: f64) -> Self {
        Self::with_time(time_clip(n), true)
    }

    /// `dayjs.utc(null)`: `parseDate` returns `new Date(NaN)` for `null`.
    pub fn utc_invalid() -> Self {
        Self::with_time(f64::NAN, true)
    }

    /// A dayjs as the oracle recorded it (`codec.js` `encodeScalar`):
    /// rebuilt the way `codec.js`'s decoder does it (`dayjs.utc(iso)`, then
    /// `.utcOffset(offset)` when the offset is not 0; a non-UTC one through
    /// `dayjs(iso)`, the local, equal-offset case under `TZ=UTC`).
    pub fn from_recorded(valid: bool, iso: Option<&str>, offset: f64, utc: bool) -> Self {
        if !valid {
            // `dayjs('not a date')`: a local, invalid dayjs.
            return Self::with_time(f64::NAN, false);
        }
        let time = iso.map_or(f64::NAN, iso_string_instant);
        if utc {
            let d = Self::with_time(time, true);
            if offset == 0.0 {
                d
            } else {
                d.utc_offset_set(&UtcOffset::Number(offset))
            }
        } else {
            let local = Self::with_time(time, false);
            if local.utc_offset() == offset {
                local
            } else {
                local.utc_offset_set(&UtcOffset::Number(offset))
            }
        }
    }

    /// This date in the instance validator's value shape: a
    /// `DAYJS_TAG`-tagged object holding its ISO string (`null` when
    /// invalid).
    pub fn validator_value(&self) -> serde_json::Value {
        serde_json::json!({ super::validate::DAYJS_TAG: self.to_iso_string() })
    }

    /// `isValid()`: `!(this.$d.toString() === 'Invalid Date')`.
    pub fn is_valid(&self) -> bool {
        !self.time.is_nan()
    }

    /// `valueOf()`, exposed for the Serializer fast path across the WASM
    /// boundary (PORTING.md 3.3: "On the fast path Rust receives and
    /// returns (epoch ms, utcOffset minutes)"). `NaN` for an invalid date.
    pub fn epoch_ms(&self) -> f64 {
        self.value_of()
    }

    /// `isUTC()`.
    pub fn is_utc(&self) -> bool {
        self.utc
    }

    /// `utcOffset()`: 0 when UTC, else `$offset`, else the local zone's
    /// offset, which under `TZ=UTC` dayjs computes as
    /// `-Math.round(0 / 15) * 15`, that is `-0`.
    pub fn utc_offset(&self) -> f64 {
        if self.utc {
            0.0
        } else {
            self.offset.unwrap_or(-0.0)
        }
    }

    /// `valueOf()` (the `utc` plugin's): `$d` less the offset's shift,
    /// `$offset + ($x.$localOffset || $d.getTimezoneOffset())` minutes, the
    /// time zone offset being 0 under `TZ=UTC`. `toDate()` is `new
    /// Date(this.valueOf())`.
    fn value_of(&self) -> f64 {
        match self.offset {
            Some(offset) => {
                let local = self
                    .local_offset
                    .filter(|l| *l != 0.0 && !l.is_nan())
                    .unwrap_or(0.0);
                self.time - (offset + local) * MS_PER_MINUTE
            }
            None => self.time,
        }
    }

    /// `.utc()`: `dayjs(this.toDate(), { utc: true })`.
    pub fn to_utc(&self) -> Self {
        Self::with_time(time_clip(self.value_of()), true)
    }

    /// `.local()`: `dayjs(this.toDate(), { utc: false })`.
    fn to_local(&self) -> Self {
        Self::with_time(time_clip(self.value_of()), false)
    }

    /// `.utcOffset(input)` (the `utc` plugin's setter).
    pub fn utc_offset_set(&self, input: &UtcOffset) -> Self {
        let input = match input {
            UtcOffset::Number(n) => *n,
            UtcOffset::String(s) => match offset_from_string(s) {
                Some(n) => n,
                // `if (input === null) return this`
                None => return self.clone(),
            },
        };
        let offset = if input.abs() <= 16.0 {
            input * 60.0
        } else {
            input
        };
        // `input !== 0` (`-0 !== 0` is false; `NaN !== 0` is true).
        if input != 0.0 {
            // `this.$u ? this.toDate().getTimezoneOffset() : -1 * this.utcOffset()`,
            // with `getTimezoneOffset()` 0 under TZ=UTC.
            let local_timezone_offset = if self.utc { 0.0 } else { -self.utc_offset() };
            let mut ins = self.to_local();
            // `.add(offset + localTimezoneOffset, 'minute')`
            ins.time = time_clip(ins.time + (offset + local_timezone_offset) * MS_PER_MINUTE);
            ins.offset = Some(offset);
            ins.local_offset = Some(local_timezone_offset);
            ins
        } else {
            self.to_utc()
        }
    }

    /// `toISOString()`: `this.toDate().toISOString()`, `None` where JS
    /// throws `RangeError: Invalid time value`.
    pub fn to_iso_string(&self) -> Option<String> {
        let time = time_clip(self.value_of());
        if time.is_nan() {
            return None;
        }
        let f = Fields::of(time);
        let year = if (0..=9999).contains(&f.year) {
            format!("{:04}", f.year)
        } else if f.year < 0 {
            format!("-{:06}", -f.year)
        } else {
            format!("+{:06}", f.year)
        };
        Some(format!(
            "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            f.month + 1,
            f.day,
            f.hour,
            f.minute,
            f.second,
            f.ms
        ))
    }

    /// `toString()`: `this.toDate().toUTCString()` (`"Invalid Date"` when
    /// invalid).
    pub fn to_js_string(&self) -> String {
        let time = time_clip(self.value_of());
        if time.is_nan() {
            return "Invalid Date".to_string();
        }
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let f = Fields::of(time);
        let year = if f.year >= 0 {
            format!("{:04}", f.year)
        } else {
            format!("-{:06}", -f.year)
        };
        format!(
            "{}, {:02} {} {year} {:02}:{:02}:{:02} GMT",
            DAYS[f.weekday as usize], f.day, MONTHS[f.month as usize], f.hour, f.minute, f.second
        )
    }

    /// `format('YYYY-MM-DDTHH:mm:ss.SSS[Z]')` when `utcOffset()` is 0, and
    /// `format('YYYY-MM-DDTHH:mm:ss.SSSZ')` otherwise: the two formats
    /// `JSONGenerator.convertToJSON` uses (TS: `inZ ? '[Z]' : 'Z'`). An
    /// invalid date formats as `"Invalid Date"` (`C.INVALID_DATE_STRING`).
    pub fn format_json(&self) -> String {
        if !self.is_valid() {
            return "Invalid Date".to_string();
        }
        let f = Fields::of(self.time);
        let zone = if self.utc_offset() == 0.0 {
            "Z".to_string()
        } else {
            pad_zone_str(self.utc_offset())
        };
        // Field by field, as dayjs's REGEX_FORMAT replacement does.
        format!(
            "{}-{}-{}T{}:{}:{}.{}{zone}",
            pad_start(&f.year.to_string(), 4),
            pad_start(&(f.month + 1).to_string(), 2),
            pad_start(&f.day.to_string(), 2),
            pad_start(&f.hour.to_string(), 2),
            pad_start(&f.minute.to_string(), 2),
            pad_start(&f.second.to_string(), 2),
            pad_start(&f.ms.to_string(), 3),
        )
    }
}

/// dayjs `Utils.s` (`padStart`): `String(n).padStart(length, '0')`.
fn pad_start(s: &str, length: usize) -> String {
    let n = s.encode_utf16().count();
    if n >= length {
        s.to_string()
    } else {
        format!("{}{s}", "0".repeat(length - n))
    }
}

/// dayjs `Utils.z` (`padZoneStr`): `±HH:mm` from `utcOffset()`.
fn pad_zone_str(utc_offset: f64) -> String {
    let neg_minutes = -utc_offset;
    let minutes = neg_minutes.abs();
    let hour_offset = (minutes / 60.0).floor();
    let minute_offset = minutes % 60.0;
    format!(
        "{}{}:{}",
        if neg_minutes <= 0.0 { '+' } else { '-' },
        pad_start(&ecma::number_to_string(hour_offset), 2),
        pad_start(&ecma::number_to_string(minute_offset), 2),
    )
}

/// The `utc` plugin's `offsetFromString`: the first `[+-]\d\d(?::?\d\d)?`
/// in the string, in minutes, or `None` (JS `null`) when there is none.
fn offset_from_string(value: &str) -> Option<f64> {
    static OFFSET: LazyLock<regress::Regex> =
        LazyLock::new(|| regress::Regex::new(r"[+-]\d\d(?::?\d\d)?").expect("static pattern"));
    static PARTS: LazyLock<regress::Regex> =
        LazyLock::new(|| regress::Regex::new(r"([+-]|\d\d)").expect("static pattern"));
    let m = OFFSET.find(value)?;
    let offset = &value[m.range];
    // `("" + offset[0]).match(/([+-]|\d\d)/g) || ['-', 0, 0]`
    let parts: Vec<&str> = PARTS.find_iter(offset).map(|m| &offset[m.range]).collect();
    let indicator = parts.first().copied().unwrap_or("-");
    let hours = parts.get(1).map_or(0.0, |h| ecma::string_to_number(h));
    // `+minutesOffset` with `minutesOffset` possibly `undefined`: NaN.
    let minutes = parts.get(2).map_or(f64::NAN, |m| ecma::string_to_number(m));
    let total = hours * 60.0 + minutes;
    if total == 0.0 {
        return Some(0.0);
    }
    Some(if indicator == "+" { total } else { -total })
}


/// The `strictQualifiedDateTimes` format, the only `DateTime` string form
/// accepted (P5-24, BC-07, accordproject/concerto-rust#328):
/// `YYYY-MM-DDTHH:mm:ss`, an optional fraction of any length, then `Z` or
/// `±HH:mm`. TS: `/^((?:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2}(?:\.\d+)?))(Z|[+-]\d{2}:\d{2}))$/`.
static STRICT_DATE_TIME: LazyLock<regress::Regex> = LazyLock::new(|| {
    regress::Regex::new(
        r"^((?:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2}(?:\.\d+)?))(Z|[+-]\d{2}:\d{2}))$",
    )
    .expect("static pattern")
});

/// Whether `s` has the strict `DateTime` format ([`STRICT_DATE_TIME`]). It
/// says nothing about whether the fields name a real instant: see
/// [`strict_instant`].
pub(crate) fn is_strict_date_time_format(s: &str) -> bool {
    STRICT_DATE_TIME.find(s).is_some()
}

/// The time value (ms since the epoch) of a strict `DateTime` string, or
/// `NaN` when `s` is not one: it must have the strict format, and name a
/// real calendar instant (no `2024-02-30`, no `T24:00:00`, no leap second
/// `:60`, offsets up to `±23:59`; BC-42). The fraction is truncated to
/// milliseconds, as `Date` does. chrono's RFC 3339 parser does the
/// calendar checks; the regex keeps out the forms RFC 3339 allows and the
/// strict format does not (a lower-case `t`/`z`, a space separator).
fn strict_instant(s: &str) -> f64 {
    if !is_strict_date_time_format(s) {
        return f64::NAN;
    }
    match chrono::DateTime::parse_from_rfc3339(s) {
        // chrono reads `:60` as a leap second (a nanosecond field of 1e9
        // or more); a `DateTime` has none.
        Ok(dt) if dt.timestamp_subsec_nanos() < 1_000_000_000 => {
            time_clip(dt.timestamp_millis() as f64)
        }
        _ => f64::NAN,
    }
}

/// `toISOString()` output (`YYYY-MM-DDTHH:mm:ss.sssZ`, or an expanded
/// `±YYYYYY` year) read back as a time value, `NaN` when `s` is not one:
/// the inverse of [`Dayjs::to_iso_string`], for values this engine (or the
/// oracle's recorder) formatted itself. Not a parser for user input.
fn iso_string_instant(s: &str) -> f64 {
    static ISO_STRING: LazyLock<regress::Regex> = LazyLock::new(|| {
        regress::Regex::new(r"^([+-]\d{6}|\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})\.(\d{3})Z$")
            .expect("static pattern")
    });
    let Some(m) = ISO_STRING.find(s) else {
        return f64::NAN;
    };
    let num = |i: usize| -> i64 {
        m.group(i)
            .and_then(|r| s[r].trim_start_matches('+').parse().ok())
            .unwrap_or(-1)
    };
    let (year, month, day) = (num(1), num(2), num(3));
    let (hour, minute, second, ms) = (num(4), num(5), num(6), num(7));
    let date = i32::try_from(year)
        .ok()
        .and_then(|y| chrono::NaiveDate::from_ymd_opt(y, month as u32, day as u32));
    match date.and_then(|d| d.and_hms_milli_opt(hour as u32, minute as u32, second as u32, ms as u32)) {
        Some(dt) => time_clip(dt.and_utc().timestamp_millis() as f64),
        None => f64::NAN,
    }
}

/// ECMAScript `ToIntegerOrInfinity`.
fn to_integer_or_infinity(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n.trunc() }
}

/// ECMAScript `TimeClip`.
fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > 8.64e15 {
        return f64::NAN;
    }
    // `ToIntegerOrInfinity`, which also turns -0 into +0.
    to_integer_or_infinity(time) + 0.0
}

/// The UTC calendar fields of a time value.
struct Fields {
    year: i64,
    /// 0-11.
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    ms: i64,
    weekday: i64,
}

impl Fields {
    fn of(time: f64) -> Self {
        let t = time as i64;
        let days = t.div_euclid(MS_PER_DAY as i64);
        let ms_of_day = t.rem_euclid(MS_PER_DAY as i64);
        // civil_from_days
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if m <= 2 { y + 1 } else { y };
        Self {
            year,
            month: m - 1,
            day: d,
            hour: ms_of_day / 3_600_000,
            minute: ms_of_day / 60_000 % 60,
            second: ms_of_day / 1000 % 60,
            ms: ms_of_day % 1000,
            weekday: (days + 4).rem_euclid(7),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn iso(s: &str) -> Option<String> {
        Dayjs::utc_parse(s).to_iso_string()
    }

    /// `epoch_ms()` round-trips through `utc_from_number`/`utc_offset_set`,
    /// the pair the Serializer fast path's wire codec (P4-10) crosses the
    /// WASM boundary with, and is `NaN` for an invalid date.
    #[test]
    fn epoch_ms_round_trips_with_an_offset() {
        let utc = Dayjs::utc_parse("2021-01-01T10:00:00Z");
        assert_eq!(
            utc.epoch_ms(),
            utc.utc_offset_set(&UtcOffset::Number(0.0)).epoch_ms()
        );
        let shifted = utc.utc_offset_set(&UtcOffset::Number(60.0));
        assert_eq!(shifted.epoch_ms(), utc.epoch_ms());
        let rebuilt =
            Dayjs::utc_from_number(shifted.epoch_ms()).utc_offset_set(&UtcOffset::Number(60.0));
        assert_eq!(rebuilt, shifted);
        assert!(Dayjs::utc_invalid().epoch_ms().is_nan());
    }

    /// Strict strings read as the instant `Date` gives them: any number of
    /// fraction digits, truncated to milliseconds, and `±HH:mm` offsets.
    #[test]
    fn strict_strings_parse_to_their_instant() {
        assert_eq!(
            iso("2021-01-01T10:20:30.123456Z").as_deref(),
            Some("2021-01-01T10:20:30.123Z")
        );
        assert_eq!(
            iso("2021-01-01T10:20:30.1234567891Z").as_deref(),
            Some("2021-01-01T10:20:30.123Z")
        );
        assert_eq!(
            iso("2021-01-01T10:00:00+05:00").as_deref(),
            Some("2021-01-01T05:00:00.000Z")
        );
        assert_eq!(
            iso("2022-11-28T01:02:03.98765-08:00").as_deref(),
            Some("2022-11-28T09:02:03.987Z")
        );
        assert_eq!(
            iso("1969-12-31T23:59:59.9999Z").as_deref(),
            Some("1969-12-31T23:59:59.999Z")
        );
        assert_eq!(
            iso("0000-01-01T00:00:00Z").as_deref(),
            Some("0000-01-01T00:00:00.000Z")
        );
        assert_eq!(
            iso("9999-12-31T23:59:59.999Z").as_deref(),
            Some("9999-12-31T23:59:59.999Z")
        );
        assert!(Dayjs::utc_parse("2021-01-01T00:00:00Z").is_utc());
    }

    /// BC-07 (R1): every lenient form dayjs and V8 used to accept is
    /// invalid now.
    #[test]
    fn lenient_forms_are_invalid() {
        for s in [
            "2021-01-01",
            "2021-01-01T00:00:00",
            "2021-01-01T00:00",
            "2022-11-28 01:02:03.987Z",
            "2022-11-28t01:02:03.987Z",
            "2022-11-28T01:02:03.987z",
            "2022",
            "2022-11",
            "+002022-11-28",
            "--11-28",
            "11-28",
            "11/28/2022",
            "20240102",
            "May 1, 2020",
            "1",
            "2022-11-28T01:02:03.987-08",
            "2022-11-28T01:02:03+0100",
            "1970-01-01T00:00:00.000Z\u{0}",
            " 2021-01-01T00:00:00Z",
            "not a date",
            "",
        ] {
            assert!(!Dayjs::utc_parse(s).is_valid(), "{s:?} should be invalid");
        }
    }

    /// BC-42 (R1): the fields must name a real calendar instant. No
    /// roll-over of impossible days or `24:00`, no leap seconds, no
    /// out-of-range months, hours, minutes or offsets.
    #[test]
    fn impossible_instants_are_invalid() {
        for s in [
            "2024-02-30T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2024-04-31T00:00:00Z",
            "2024-01-02T24:00:00Z",
            "2016-12-31T23:59:60Z",
            "2021-13-01T00:00:00Z",
            "2021-00-01T00:00:00Z",
            "2021-01-00T00:00:00Z",
            "2021-01-01T00:60:00Z",
            "2021-01-01T00:00:00+24:00",
            "2021-01-01T00:00:00+01:60",
        ] {
            assert!(!Dayjs::utc_parse(s).is_valid(), "{s:?} should be invalid");
        }
        // A leap day in a leap year is a real date.
        assert_eq!(
            iso("2024-02-29T00:00:00Z").as_deref(),
            Some("2024-02-29T00:00:00.000Z")
        );
        assert_eq!(
            iso("2021-01-01T00:00:00+23:59").as_deref(),
            Some("2020-12-31T00:01:00.000Z")
        );
    }

    /// `from_recorded` reads `toISOString()` output back, expanded years
    /// included, and nothing else.
    #[test]
    fn iso_strings_read_back() {
        let d = Dayjs::from_recorded(true, Some("2021-01-01T00:00:00.000Z"), 0.0, true);
        assert_eq!(d.to_iso_string().as_deref(), Some("2021-01-01T00:00:00.000Z"));
        let d = Dayjs::from_recorded(true, Some("-000001-12-31T23:00:00.000Z"), 60.0, true);
        assert_eq!(d.to_iso_string().as_deref(), Some("-000001-12-31T23:00:00.000Z"));
        assert_eq!(d.format_json(), "0000-01-01T00:00:00.000+01:00");
        let d = Dayjs::from_recorded(true, Some("+010000-01-01T00:00:00.000Z"), 0.0, true);
        assert_eq!(d.to_iso_string().as_deref(), Some("+010000-01-01T00:00:00.000Z"));
        assert!(!Dayjs::from_recorded(true, Some("2021-01-01"), 0.0, true).is_valid());
        assert!(!Dayjs::from_recorded(false, None, 0.0, true).is_valid());
    }

    #[test]
    fn utc_offset_shifts_the_wrapped_date() {
        let d = Dayjs::utc_parse("2021-01-01T00:00:00Z").utc_offset_set(&UtcOffset::Number(60.0));
        assert_eq!(d.utc_offset(), 60.0);
        assert!(!d.is_utc());
        // `toISOString()` is the original instant; `format` the local time.
        assert_eq!(
            d.to_iso_string().as_deref(),
            Some("2021-01-01T00:00:00.000Z")
        );
        assert_eq!(d.format_json(), "2021-01-01T01:00:00.000+01:00");
        assert_eq!(d.to_utc().format_json(), "2021-01-01T00:00:00.000Z");
        // |n| <= 16 is hours.
        let h = Dayjs::utc_parse("2021-01-01T00:00:00Z").utc_offset_set(&UtcOffset::Number(-5.0));
        assert_eq!(h.utc_offset(), -300.0);
        assert_eq!(h.format_json(), "2020-12-31T19:00:00.000-05:00");
        let s = Dayjs::utc_parse("2021-01-01T00:00:00Z")
            .utc_offset_set(&UtcOffset::String("-05:00".into()));
        assert_eq!(s.utc_offset(), -300.0);
        // `utcOffset('Z')` matches no offset and returns the same dayjs.
        let z =
            Dayjs::utc_parse("2021-01-01T00:00:00Z").utc_offset_set(&UtcOffset::String("Z".into()));
        assert!(z.is_utc());
        // `utcOffset(0)` and `utcOffset(-0)` are `.utc()`.
        assert!(h.utc_offset_set(&UtcOffset::Number(-0.0)).is_utc());
        assert_eq!(
            Dayjs::utc_parse("2021-01-01T00:00:00Z").format_json(),
            "2021-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn to_js_string_is_utc_string() {
        assert_eq!(
            Dayjs::utc_parse("2021-01-01T00:00:00Z").to_js_string(),
            "Fri, 01 Jan 2021 00:00:00 GMT"
        );
        assert_eq!(Dayjs::utc_invalid().to_js_string(), "Invalid Date");
    }
}
