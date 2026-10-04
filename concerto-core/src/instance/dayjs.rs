//! The `DateTime` values an instance holds in its `DateTime` fields and its
//! `$timestamp` (PORTING.md 3.3): what TS keeps as dayjs objects.
//!
//! D7 keeps the dayjs objects themselves in TS (template logic does date
//! arithmetic on them): on the WASM fast path Rust receives and returns
//! `(epoch ms, utcOffset minutes)`. Rust only needs the value behind one,
//! so [`Dayjs`] is a plain instant plus a UTC offset, or an explicit
//! invalid date, with the handful of operations the ported members call
//! (`JSONPopulator`, `JSONGenerator`, `Typed.assignFieldDefaults`,
//! `Factory.newResource`, the WASM wire codec and the oracle's `codec.js`
//! records). Every output is what dayjs 1.11.10 with its `utc` plugin
//! (`dayjs-setup.ts`) gives under `TZ=UTC` (3.3: every test runs with it,
//! and Rust never consults the system time zone).
//!
//! The instant is an ECMAScript time value (whole milliseconds since the
//! epoch, at most 8.64e15 either way). That range runs to the year
//! 275760, past chrono's (262142), so the instant is held as milliseconds
//! and chrono does the calendar work on the same day of a 400-year
//! Gregorian cycle (`Fields::of`).
//!
//! Parsing is *not* dayjs's: a `DateTime` string is accepted only in the
//! strict ISO 8601 / RFC 3339 form (the `strictQualifiedDateTimes`
//! regex, then chrono's calendar checks; BC-07, BC-42), on
//! every path that reads one: fields, map values and model defaults.

use std::sync::LazyLock;

use chrono::Datelike;

use crate::ecma;

/// Milliseconds in a minute (`MILLISECONDS_A_MINUTE`).
const MS_PER_MINUTE: f64 = 60_000.0;
const MS_PER_DAY: i64 = 86_400_000;
/// The largest ECMAScript time value, either way (`TimeClip`).
const MAX_TIME: f64 = 8.64e15;

/// A `DateTime` value: an instant plus the UTC offset it reads in, or an
/// invalid date. What a dayjs object (the `dayjs-setup.ts` build: dayjs
/// 1.11.10 plus `utc`) is to the ported members.
#[derive(Debug, Clone)]
pub struct Dayjs {
    /// The instant, as an ECMAScript time value (`valueOf()` of a dayjs in
    /// UTC or local time); `None` for an invalid date.
    instant: Option<i64>,
    /// The offset the date reads in.
    zone: Zone,
}

/// The offset a [`Dayjs`] reads in.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Zone {
    /// UTC (`isUTC()`): `utcOffset()` is 0.
    Utc,
    /// Local time, which under `TZ=UTC` reads as UTC: `utcOffset()` is
    /// `-0`, dayjs's `-Math.round(0 / 15) * 15`.
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    Local,
    /// A fixed offset in minutes, as `utcOffset(n)` set it (any number,
    /// fractional or beyond a day included, as dayjs keeps it).
    Offset(f64),
}

/// What `utcOffset(input)` is given: a number of minutes (or hours, when
/// `|n| <= 16`), or a `±HH:mm` string.
#[derive(Debug, Clone, PartialEq)]
pub enum UtcOffset {
    /// A number of minutes, or of hours when `|n| <= 16`.
    Number(f64),
    /// A `±HH:mm` offset string.
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    String(String),
}

impl PartialEq for Dayjs {
    /// Two dates are equal when both are valid, read in the same offset
    /// and show the same local time. An invalid date equals nothing, as
    /// the time value it wraps is `NaN`.
    fn eq(&self, other: &Self) -> bool {
        self.zone == other.zone
            && matches!((self.local_ms(), other.local_ms()), (Some(a), Some(b)) if a == b)
    }
}

impl Dayjs {
    /// A UTC date at the time value `instant` (invalid when it is `None`).
    fn utc_at(instant: Option<i64>) -> Self {
        Self {
            instant,
            zone: Zone::Utc,
        }
    }

    /// `dayjs.utc()` at the time value `now_ms` (TS reads the clock; the
    /// caller supplies it): `Date.now()`, a whole number of
    /// milliseconds.
    pub fn utc_now(now_ms: f64) -> Self {
        Self::utc_from_number(now_ms)
    }

    /// A `DateTime` string read with the strict rule (BC-07): the
    /// `strictQualifiedDateTimes` format, naming a real calendar instant
    /// (`strict_instant`). Anything else is an invalid date, where TS 5.0.0's
    /// `dayjs.utc(date)` parses leniently (DIVERGENCES.md DV-009).
    pub fn utc_parse(s: &str) -> Self {
        Self::utc_at(strict_instant(s))
    }

    /// `dayjs.utc(n)` for a number: `new Date(n)`.
    pub fn utc_from_number(n: f64) -> Self {
        Self::utc_at(time_clip(n))
    }

    /// `dayjs.utc(null)`: `parseDate` returns `new Date(NaN)` for `null`.
    pub fn utc_invalid() -> Self {
        Self::utc_at(None)
    }

    /// A date as the oracle recorded it (`codec.js` `encodeScalar`):
    /// rebuilt the way `codec.js`'s decoder does it (`dayjs.utc(iso)`, then
    /// `.utcOffset(offset)` when the offset is not 0; a non-UTC one through
    /// `dayjs(iso)`, the local, equal-offset case under `TZ=UTC`).
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    pub fn from_recorded(valid: bool, iso: Option<&str>, offset: f64, utc: bool) -> Self {
        let zone = if utc { Zone::Utc } else { Zone::Local };
        if !valid {
            // `dayjs('not a date')`: a local, invalid dayjs.
            return Self {
                instant: None,
                zone: Zone::Local,
            };
        }
        let d = Self {
            instant: iso.and_then(iso_string_instant),
            zone,
        };
        // `utcOffset()` is 0 (UTC) or -0 (local), both `== 0`.
        if offset == 0.0 {
            d
        } else {
            d.utc_offset_set(&UtcOffset::Number(offset))
        }
    }

    /// This date in the instance validator's value shape: a
    /// `DAYJS_TAG`-tagged object holding its ISO string (`null` when
    /// invalid).
    pub fn validator_value(&self) -> crate::json::Value {
        crate::json!({ super::validate::DAYJS_TAG: self.to_iso_string() })
    }

    /// `isValid()`.
    pub fn is_valid(&self) -> bool {
        self.instant.is_some()
    }

    /// `valueOf()` for the Serializer fast path, which crosses a date as (epoch
    /// ms, utcOffset minutes); `NaN` for an invalid date. With an offset, dayjs
    /// cuts the shifted local time to whole milliseconds, so an offset that is
    /// not a whole number of milliseconds gives a value a fraction away.
    pub fn epoch_ms(&self) -> f64 {
        match (self.instant, self.zone) {
            (None, _) => f64::NAN,
            (Some(_), Zone::Offset(minutes)) => {
                let shift = minutes * MS_PER_MINUTE;
                self.local_ms().map_or(f64::NAN, |local| local as f64 - shift)
            }
            (Some(ms), _) => ms as f64,
        }
    }

    /// `isUTC()`.
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    pub fn is_utc(&self) -> bool {
        self.zone == Zone::Utc
    }

    /// `utcOffset()`, in minutes.
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    pub fn utc_offset(&self) -> f64 {
        match self.zone {
            Zone::Utc => 0.0,
            Zone::Local => -0.0,
            Zone::Offset(minutes) => minutes,
        }
    }

    /// The local time the date reads as, as a time value: the instant
    /// shifted by the offset (`None` when invalid). A valid date's always
    /// is a time value (the setter keeps it so).
    fn local_ms(&self) -> Option<i64> {
        let ms = self.instant?;
        match self.zone {
            Zone::Offset(minutes) => time_clip(ms as f64 + minutes * MS_PER_MINUTE),
            Zone::Utc | Zone::Local => Some(ms),
        }
    }

    /// `.utc()`: the same instant in UTC.
    pub fn to_utc(&self) -> Self {
        Self::utc_at(time_clip(self.epoch_ms()))
    }

    /// `.utcOffset(input)` (the `utc` plugin's setter) on a date in UTC or
    /// local time, the only ones the callers set an offset on: a number
    /// `|n| <= 16` is hours, others minutes; a string is read for its first
    /// `±HH:mm`, and one without any leaves the date as it is; an offset of 0
    /// is `.utc()`. The date is invalid when the offset is not a number or
    /// its local time is not a time value.
    ///
    /// (dayjs, on a date that already has an offset, shifts the local time
    /// by the difference of the two offsets; this sets the offset on the
    /// date's instant.)
    pub fn utc_offset_set(&self, input: &UtcOffset) -> Self {
        let input = match input {
            UtcOffset::Number(n) => *n,
            UtcOffset::String(s) => match offset_from_string(s) {
                Some(n) => n,
                // `if (input === null) return this`
                None => return self.clone(),
            },
        };
        let minutes = if input.abs() <= 16.0 {
            input * 60.0
        } else {
            input
        };
        // `input !== 0` (`-0 !== 0` is false; `NaN !== 0` is true).
        if input == 0.0 {
            return self.to_utc();
        }
        let set = Self {
            instant: time_clip(self.epoch_ms()),
            zone: Zone::Offset(minutes),
        };
        Self {
            instant: set.local_ms().and(set.instant),
            zone: set.zone,
        }
    }

    /// `toISOString()`: `this.toDate().toISOString()`, `None` where JS
    /// throws `RangeError: Invalid time value`.
    pub fn to_iso_string(&self) -> Option<String> {
        let f = Fields::of(time_clip(self.epoch_ms())?);
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
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    pub fn to_js_string(&self) -> String {
        let Some(time) = time_clip(self.epoch_ms()) else {
            return "Invalid Date".to_string();
        };
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
    /// `JSONGenerator.convertToJSON` uses (TS: `inZ ? '[Z]' : 'Z'`), of the
    /// local time. An invalid date formats as `"Invalid Date"`
    /// (`C.INVALID_DATE_STRING`).
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    pub fn format_json(&self) -> String {
        let Some(local) = self.local_ms() else {
            return "Invalid Date".to_string();
        };
        let f = Fields::of(local);
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
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
fn pad_start(s: &str, length: usize) -> String {
    let n = s.encode_utf16().count();
    if n >= length {
        s.to_string()
    } else {
        format!("{}{s}", "0".repeat(length - n))
    }
}

/// dayjs `Utils.z` (`padZoneStr`): `±HH:mm` from `utcOffset()`.
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
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
/// accepted (BC-07): `YYYY-MM-DDTHH:mm:ss`, an optional fraction of any
/// length, then `Z` or `±HH:mm`. TS:
/// `/^((?:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2}(?:\.\d+)?))(Z|[+-]\d{2}:\d{2}))$/`.
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
/// `None` when `s` is not one: it must have the strict format, and name a
/// real calendar instant (no `2024-02-30`; no `T24:00:00`; no leap second
/// `:60`; offsets up to `±23:59`; BC-42). The fraction is truncated to
/// milliseconds, as `Date` does. chrono's RFC 3339 parser does the
/// calendar checks; the regex keeps out the forms RFC 3339 allows and the
/// strict format does not (a lower-case `t`/`z`, a space separator).
fn strict_instant(s: &str) -> Option<i64> {
    if !is_strict_date_time_format(s) {
        return None;
    }
    match chrono::DateTime::parse_from_rfc3339(s) {
        // chrono reads `:60` as a leap second (a nanosecond field of 1e9
        // or more); a `DateTime` has none.
        Ok(dt) if dt.timestamp_subsec_nanos() < 1_000_000_000 => {
            time_clip(dt.timestamp_millis() as f64)
        }
        _ => None,
    }
}

/// `toISOString()` output (`YYYY-MM-DDTHH:mm:ss.sssZ`, or an expanded
/// `±YYYYYY` year) read back as a time value, `None` when `s` is not one:
/// the inverse of [`Dayjs::to_iso_string`], for values this engine (or the
/// oracle's recorder) formatted itself. Not a parser for user input.
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
fn iso_string_instant(s: &str) -> Option<i64> {
    static ISO_STRING: LazyLock<regress::Regex> = LazyLock::new(|| {
        regress::Regex::new(r"^([+-]\d{6}|\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})\.(\d{3})Z$")
            .expect("static pattern")
    });
    let m = ISO_STRING.find(s)?;
    let num = |i: usize| -> i64 {
        m.group(i)
            .and_then(|r| s[r].trim_start_matches('+').parse().ok())
            .unwrap_or(-1)
    };
    let (year, month, day) = (num(1), num(2), num(3));
    let (hour, minute, second, ms) = (num(4), num(5), num(6), num(7));
    let dt = i32::try_from(year)
        .ok()
        .and_then(|y| chrono::NaiveDate::from_ymd_opt(y, month as u32, day as u32))?
        .and_hms_milli_opt(hour as u32, minute as u32, second as u32, ms as u32)?;
    time_clip(dt.and_utc().timestamp_millis() as f64)
}

/// ECMAScript `TimeClip`: `None` for `NaN`, an infinity or a value beyond
/// 8.64e15 either way, else the value truncated to whole milliseconds.
fn time_clip(time: f64) -> Option<i64> {
    if !time.is_finite() || time.abs() > MAX_TIME {
        return None;
    }
    Some(time.trunc() as i64)
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
    /// 0 (Sunday) to 6.
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    weekday: i64,
}

impl Fields {
    /// chrono's calendar fields of the same day in the 400-year Gregorian
    /// cycle from 1970 (146097 days, a whole number of weeks), with the
    /// cycles added back to the year: the ECMAScript time value range runs
    /// past chrono's.
    fn of(time: i64) -> Self {
        const DAYS_PER_400_YEARS: i64 = 146_097;
        let days = time.div_euclid(MS_PER_DAY);
        let ms_of_day = time.rem_euclid(MS_PER_DAY);
        let cycles = days.div_euclid(DAYS_PER_400_YEARS);
        let date = chrono::DateTime::UNIX_EPOCH
            .date_naive()
            .checked_add_days(chrono::Days::new(
                days.rem_euclid(DAYS_PER_400_YEARS).unsigned_abs(),
            ))
            .expect("1970 to 2369 is in chrono's range");
        Self {
            year: i64::from(date.year()) + cycles * 400,
            month: i64::from(date.month0()),
            day: i64::from(date.day()),
            hour: ms_of_day / 3_600_000,
            minute: ms_of_day / 60_000 % 60,
            second: ms_of_day / 1000 % 60,
            ms: ms_of_day % 1000,
            weekday: i64::from(date.weekday().num_days_from_sunday()),
        }
    }
}

#[cfg(test)]
mod tests;

/// Every output of every date the callers can build, against a digest of
/// the same transcript from the dayjs state emulation this value replaced
/// (compared line for line with it, 1,292,044 lines, before it was
/// deleted), and a table of cases.
#[cfg(test)]
mod golden;
