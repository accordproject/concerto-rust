//! The dayjs state emulation this module replaced (P5-66), kept only to
//! compare the two, output for output, before it is deleted.
#![allow(dead_code)]

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
        serde_json::json!({ crate::instance::validate::DAYJS_TAG: self.to_iso_string() })
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
