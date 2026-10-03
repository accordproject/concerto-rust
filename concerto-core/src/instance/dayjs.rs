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
//! and Rust never consults the system time zone). P5-66
//! (accordproject/concerto-rust#403) replaced the emulation of dayjs's
//! internal state (`$d` shifting, `$u`, `$offset`, `$x.$localOffset`)
//! with this value, with no change in behaviour.
//!
//! The instant is an ECMAScript time value (whole milliseconds since the
//! epoch, at most 8.64e15 either way). That range runs to the year
//! 275760, past chrono's (262142), so the instant is held as milliseconds
//! and chrono does the calendar work on the same day of a 400-year
//! Gregorian cycle ([`Fields::of`]).
//!
//! Parsing is *not* dayjs's (P5-24, accordproject/concerto-rust#328): a
//! `DateTime` string is accepted only in the strict ISO 8601 / RFC 3339
//! form (the `strictQualifiedDateTimes` regex, then chrono's calendar
//! checks, BC-07 and BC-42 in R1), on every path that reads one: fields,
//! map values and model defaults.

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
    /// caller supplies it, D7): `Date.now()`, a whole number of
    /// milliseconds.
    pub fn utc_now(now_ms: f64) -> Self {
        Self::utc_from_number(now_ms)
    }

    /// A `DateTime` string read with the strict rule (P5-24, BC-07,
    /// accordproject/concerto-rust#328): the `strictQualifiedDateTimes`
    /// format, naming a real calendar instant (`strict_instant`). Anything
    /// else is an invalid date, where TS's `dayjs.utc(date)` used to parse
    /// leniently (DIVERGENCES.md DV-009).
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
    pub fn validator_value(&self) -> serde_json::Value {
        serde_json::json!({ super::validate::DAYJS_TAG: self.to_iso_string() })
    }

    /// `isValid()`.
    pub fn is_valid(&self) -> bool {
        self.instant.is_some()
    }

    /// `valueOf()`, exposed for the Serializer fast path across the WASM
    /// boundary (PORTING.md 3.3: "On the fast path Rust receives and
    /// returns (epoch ms, utcOffset minutes)"). `NaN` for an invalid date.
    ///
    /// With an offset, dayjs keeps the local time (the instant shifted by
    /// the offset, cut to whole milliseconds) and takes the shift back off,
    /// so an offset that is not a whole number of milliseconds gives a
    /// value a fraction away from the instant.
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
    /// `|n| <= 16` is hours, others minutes (until BC-44); a string is
    /// read for its first `±HH:mm`, and one without any leaves the date as
    /// it is; an offset of 0 is `.utc()`. The date is invalid when the
    /// offset is not a number or its local time is not a time value.
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
/// `None` when `s` is not one: it must have the strict format, and name a
/// real calendar instant (no `2024-02-30`, no `T24:00:00`, no leap second
/// `:60`, offsets up to `±23:59`; BC-42). The fraction is truncated to
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
    fn utc_offset_shifts_the_local_time() {
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

/// P5-66 (accordproject/concerto-rust#403): every output of every date
/// the callers can build, against a digest of the same transcript from the
/// dayjs state emulation this value replaced (compared line for line with
/// it, 1,292,044 lines, before it was deleted), and a table of cases.
#[cfg(test)]
mod golden {
    use sha2::{Digest, Sha256};

    use super::{Dayjs, UtcOffset};

    /// A fixed-seed SplitMix64, so every run checks the same cases.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn range(&mut self, lo: i64, hi: i64) -> i64 {
            lo + self.below((hi - lo + 1) as u64) as i64
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// `0000-01-01T00:00:00.000Z` and `9999-12-31T23:59:59.999Z`.
    const YEAR_0: i64 = -62_167_219_200_000;
    const YEAR_9999_END: i64 = 253_402_300_799_999;

    /// Everything a caller can read of a date. Numbers are compared by
    /// their bits (`-0` is not `0` on the wire), `NaN` with `NaN`.
    #[derive(Debug, PartialEq)]
    struct Seen {
        valid: bool,
        epoch_ms: u64,
        utc: bool,
        utc_offset: u64,
        format_json: String,
        iso: Option<String>,
        js_string: String,
        validator: serde_json::Value,
    }

    fn bits(n: f64) -> u64 {
        if n.is_nan() { f64::NAN.to_bits() } else { n.to_bits() }
    }

    fn seen(d: &Dayjs) -> Seen {
        Seen {
            valid: d.is_valid(),
            epoch_ms: bits(d.epoch_ms()),
            utc: d.is_utc(),
            utc_offset: bits(d.utc_offset()),
            format_json: d.format_json(),
            iso: d.to_iso_string(),
            js_string: d.to_js_string(),
            validator: d.validator_value(),
        }
    }

    /// The WASM wire codec's decode of an encoded date: `{valid: false}`,
    /// or `utc_from_number(ms)` then `utcOffset(offset)` unless it is 0.
    fn wire(d: &Dayjs) -> Dayjs {
        if !d.is_valid() {
            return Dayjs::utc_invalid();
        }
        let built = Dayjs::utc_from_number(d.epoch_ms());
        if d.utc_offset() == 0.0 {
            built
        } else {
            built.utc_offset_set(&UtcOffset::Number(d.utc_offset()))
        }
    }

    /// The offsets: the hours rule's edges (`|n| <= 16`), whole and
    /// fractional minutes, offsets past a day, non-numbers, and strings.
    fn offsets(rng: &mut Rng) -> Vec<UtcOffset> {
        let mut numbers = vec![
            0.0,
            -0.0,
            0.5,
            -0.25,
            1e-7,
            0.1 + 0.2,
            15.99,
            -15.99,
            16.0,
            -16.0,
            16.0000001,
            -16.0000001,
            16.5,
            -16.5,
            17.0,
            -17.0,
            17.3,
            17.00001,
            -17.00001,
            30.0,
            45.0,
            60.0,
            -60.0,
            90.0,
            -90.0,
            330.0,
            345.0,
            -570.0,
            720.0,
            840.0,
            -840.0,
            1439.0,
            1440.0,
            -1440.0,
            1500.0,
            -2000.0,
            5000.0,
            1e6,
            1e11,
            -1e11,
            1.5e11,
            1e300,
            f64::MIN_POSITIVE,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        numbers.extend((-20..=20).map(f64::from));
        for _ in 0..40 {
            numbers.push(rng.range(-3000, 3000) as f64);
            numbers.push((rng.unit() - 0.5) * 6000.0);
            numbers.push((rng.unit() - 0.5) * 40.0);
        }
        let strings = [
            "+05:30",
            "-08:00",
            "-0800",
            "+0530",
            "+05",
            "-05",
            "+00",
            "Z",
            "",
            "+00:00",
            "-00:00",
            "+0000",
            "UTC+01:00",
            "x-12:34y",
            "+99:99",
            "+16:00",
            "-16:01",
            "+24:00",
            "+00:15",
            "-00:30",
            "2021-01-01T00:00:00+05:30",
        ];
        numbers
            .into_iter()
            .map(UtcOffset::Number)
            .chain(strings.iter().map(|s| UtcOffset::String((*s).to_string())))
            .collect()
    }

    /// Strict-format strings over years 0000-9999, some of them naming no
    /// real instant (day 31 of a short month, hour 24, second 60, offset
    /// hour 24 or minute 60), with fractions of up to ten digits.
    fn date_string(rng: &mut Rng) -> String {
        let fraction = match rng.below(4) {
            0 => String::new(),
            _ => {
                let digits = rng.range(1, 10) as usize;
                let n: String = (0..digits)
                    .map(|_| char::from(b'0' + rng.below(10) as u8))
                    .collect();
                format!(".{n}")
            }
        };
        let zone = match rng.below(3) {
            0 => "Z".to_string(),
            _ => format!(
                "{}{:02}:{:02}",
                if rng.below(2) == 0 { '+' } else { '-' },
                rng.range(0, 24),
                if rng.below(8) == 0 { 60 } else { rng.range(0, 59) }
            ),
        };
        // Mostly in range, at times one past it.
        let mut field = |lo: i64, hi: i64, past: i64, one_in: u64| {
            let hi = if rng.below(one_in) == 0 { past } else { hi };
            rng.range(lo, hi)
        };
        let year = field(0, 9999, 9999, 1);
        let month = field(1, 12, 13, 20);
        let day = field(1, 28, 31, 4);
        let hour = field(0, 23, 24, 20);
        let minute = field(0, 59, 60, 40);
        let second = field(0, 59, 60, 20);
        let month = if month == 13 { 0 } else { month };
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{fraction}{zone}")
    }

    /// A time value: mostly in years 0000-9999, else anywhere in (and
    /// just beyond) the ECMAScript range, fractional, or not a number.
    fn time_value(rng: &mut Rng) -> f64 {
        match rng.below(10) {
            0 => (rng.unit() - 0.5) * 2.0 * 8.64e15,
            1 => rng.range(YEAR_0, YEAR_9999_END) as f64 + rng.unit(),
            2 => [
                0.0,
                -0.0,
                -1.0,
                1.0,
                8.64e15,
                -8.64e15,
                8.64e15 + 1.0,
                -8.64e15 - 1.0,
                8.64e15 - 0.5,
                -0.5,
                0.5,
                YEAR_0 as f64,
                YEAR_0 as f64 - 1.0,
                YEAR_9999_END as f64,
                YEAR_9999_END as f64 + 1.0,
                f64::NAN,
                f64::INFINITY,
                f64::NEG_INFINITY,
            ][rng.below(18) as usize],
            _ => rng.range(YEAR_0, YEAR_9999_END) as f64,
        }
    }

    /// Every way the callers build a date.
    fn bases(rng: &mut Rng) -> Vec<(String, Dayjs)> {
        let mut out = vec![("utc_invalid".to_string(), Dayjs::utc_invalid())];
        for s in [
            "2021-01-01T00:00:00Z",
            "0000-01-01T00:00:00Z",
            "0000-01-01T00:00:00+23:59",
            "9999-12-31T23:59:59.999Z",
            "9999-12-31T23:59:59.999-23:59",
            "1970-01-01T00:00:00.000Z",
            "1969-12-31T23:59:59.9999Z",
            "2024-02-29T12:00:00+05:30",
            "2021-01-01",
            "not a date",
            "",
        ] {
            out.push((format!("utc_parse({s:?})"), Dayjs::utc_parse(s)));
        }
        for _ in 0..400 {
            let s = date_string(rng);
            out.push((format!("utc_parse({s:?})"), Dayjs::utc_parse(&s)));
        }
        for _ in 0..400 {
            let n = time_value(rng);
            out.push((format!("utc_from_number({n:?})"), Dayjs::utc_from_number(n)));
        }
        for _ in 0..20 {
            // `Date.now()`: a whole number of milliseconds.
            let n = rng.range(YEAR_0, YEAR_9999_END) as f64;
            out.push((format!("utc_now({n:?})"), Dayjs::utc_now(n)));
        }
        let recorded_offsets = [
            0.0,
            -0.0,
            1.0,
            -5.0,
            16.0,
            17.0,
            60.0,
            -300.0,
            330.0,
            1500.0,
            0.5,
            30.5,
            f64::NAN,
        ];
        for _ in 0..300 {
            let n = time_value(rng);
            let iso = match rng.below(12) {
                0 => None,
                1 => Some("2021-01-01".to_string()),
                2 => Some("+275760-09-13T00:00:00.000Z".to_string()),
                3 => Some("+262143-01-01T00:00:00.000Z".to_string()),
                _ => Dayjs::utc_from_number(n).to_iso_string(),
            };
            let valid = rng.below(10) != 0;
            let utc = rng.below(2) == 0;
            let offset = recorded_offsets[rng.below(recorded_offsets.len() as u64) as usize];
            out.push((
                format!("from_recorded({valid}, {iso:?}, {offset:?}, {utc})"),
                Dayjs::from_recorded(valid, iso.as_deref(), offset, utc),
            ));
        }
        out
    }

    /// One line per output of every date the callers can build: as
    /// constructed; with each offset set (populator, `fromJSON`, the wire
    /// decode, on a date in UTC or local time, the only ones the callers
    /// set an offset on); through `.utc().utcOffset(o)` then `format`
    /// (generator), also of a date already in an offset; across a wire
    /// round trip; and on equality.
    fn transcript() -> Vec<String> {
        let mut rng = Rng(0x5066_D4A7_0000_0403);
        let offsets = offsets(&mut rng);
        let bases = bases(&mut rng);
        let mut out = Vec::new();
        for (name, base) in &bases {
            out.push(format!("{name}: {:?}", seen(base)));
            out.push(format!("{name}.utc(): {:?}", seen(&base.to_utc())));
            out.push(format!("{name} wire: {:?}", seen(&wire(base))));
            // An invalid date equals nothing, itself included.
            #[allow(clippy::eq_op)]
            let itself = base == base;
            out.push(format!("{name} == itself: {itself}"));
            let has_offset = !base.is_utc() && base.utc_offset() != 0.0;
            for input in &offsets {
                let case = format!("{name}.utcOffset({input:?})");
                if !has_offset {
                    let set = base.utc_offset_set(input);
                    out.push(format!("{case}: {:?}", seen(&set)));
                    out.push(format!("{case} wire: {:?}", seen(&wire(&set))));
                    out.push(format!("{case} == base: {}", set == *base));
                    out.push(format!("{case} wire == itself: {}", wire(&set) == set));
                    let second = &offsets[rng.below(offsets.len() as u64) as usize];
                    out.push(format!(
                        "{case}.utc().utcOffset({second:?}): {:?}",
                        seen(&set.to_utc().utc_offset_set(second))
                    ));
                }
                out.push(format!(
                    "{name}.utc().utcOffset({input:?}).format(): {}",
                    base.to_utc().utc_offset_set(input).format_json()
                ));
            }
        }
        out
    }

    fn digest(lines: &[String]) -> String {
        let mut hash = Sha256::new();
        for line in lines {
            hash.update(line.as_bytes());
            hash.update(b"\n");
        }
        format!("{:x}", hash.finalize())
    }

    /// The SHA-256 of the transcript as the dayjs state emulation wrote it.
    const EMULATION_DIGEST: &str =
        "baf61b9ef99053beb841e3feae667dc650f1e83f135018fac9f94f30738b7b0e";

    #[test]
    fn outputs_match_the_dayjs_emulation() {
        let lines = transcript();
        assert_eq!(lines.len(), 1_292_044);
        assert_eq!(digest(&lines), EMULATION_DIGEST);
    }

    /// A date with an offset set, as the populator and the wire decode
    /// set one: `(date, offset, format_json, to_iso_string, to_js_string,
    /// epoch_ms, utc_offset, is_utc)`, as the dayjs state emulation gave
    /// them. The hours rule (`|n| <= 16`), fractional and past-a-day
    /// offsets, strings, and the years around 0000 and 9999.
    #[test]
    fn offsets_set_as_the_dayjs_emulation_did() {
        use UtcOffset::{Number, String as Str};
        // (date, offset, format_json, to_iso_string, to_js_string, epoch_ms,
        // utc_offset, is_utc)
        type Row = (&'static str, UtcOffset, &'static str, Option<&'static str>, &'static str, f64, f64, bool);
        let rows: [Row; 22] = [
            ("2021-06-15T12:34:56.789Z", Number(0.0), "2021-06-15T12:34:56.789Z", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 0.0, true),
            ("2021-06-15T12:34:56.789Z", Number(-0.0), "2021-06-15T12:34:56.789Z", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 0.0, true),
            ("2021-06-15T12:34:56.789Z", Number(5.0), "2021-06-15T17:34:56.789+05:00", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 300.0, false),
            ("2021-06-15T12:34:56.789Z", Number(-1.0), "2021-06-15T11:34:56.789-01:00", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, -60.0, false),
            ("2021-06-15T12:34:56.789Z", Number(16.0), "2021-06-16T04:34:56.789+16:00", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 960.0, false),
            ("2021-06-15T12:34:56.789Z", Number(16.5), "2021-06-15T12:51:26.789+00:16.5", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 16.5, false),
            ("2021-06-15T12:34:56.789Z", Number(17.0), "2021-06-15T12:51:56.789+00:17", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 17.0, false),
            ("2021-06-15T12:34:56.789Z", Number(-90.0), "2021-06-15T11:04:56.789-01:30", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, -90.0, false),
            ("2021-06-15T12:34:56.789Z", Number(1500.0), "2021-06-16T13:34:56.789+25:00", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 1500.0, false),
            ("2021-06-15T12:34:56.789Z", Number(17.00001), "2021-06-15T12:51:56.789+00:17.00001", Some("2021-06-15T12:34:56.788Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496788.4, 17.00001, false),
            ("2021-06-15T12:34:56.789Z", Number(f64::NAN), "Invalid Date", None, "Invalid Date", f64::NAN, f64::NAN, false),
            ("2021-06-15T12:34:56.789Z", Str("+05:30".into()), "2021-06-15T18:04:56.789+05:30", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 330.0, false),
            ("2021-06-15T12:34:56.789Z", Str("-0800".into()), "2021-06-15T04:34:56.789-08:00", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, -480.0, false),
            ("2021-06-15T12:34:56.789Z", Str("+05".into()), "Invalid Date", None, "Invalid Date", f64::NAN, f64::NAN, false),
            ("2021-06-15T12:34:56.789Z", Str("Z".into()), "2021-06-15T12:34:56.789Z", Some("2021-06-15T12:34:56.789Z"), "Tue, 15 Jun 2021 12:34:56 GMT", 1623760496789.0, 0.0, true),
            ("0000-01-01T00:00:00Z", Number(5.0), "0000-01-01T05:00:00.000+05:00", Some("0000-01-01T00:00:00.000Z"), "Sat, 01 Jan 0000 00:00:00 GMT", -62167219200000.0, 300.0, false),
            ("0000-01-01T00:00:00Z", Number(-1.0), "00-1-12-31T23:00:00.000-01:00", Some("0000-01-01T00:00:00.000Z"), "Sat, 01 Jan 0000 00:00:00 GMT", -62167219200000.0, -60.0, false),
            ("0000-01-01T00:00:00Z", Str("-0800".into()), "00-1-12-31T16:00:00.000-08:00", Some("0000-01-01T00:00:00.000Z"), "Sat, 01 Jan 0000 00:00:00 GMT", -62167219200000.0, -480.0, false),
            ("0000-01-01T00:00:00Z", Number(17.00001), "0000-01-01T00:17:00.001+00:17.00001", Some("0000-01-01T00:00:00.001Z"), "Sat, 01 Jan 0000 00:00:00 GMT", -62167219199999.6, 17.00001, false),
            ("9999-12-31T23:59:59.999Z", Number(5.0), "10000-01-01T04:59:59.999+05:00", Some("9999-12-31T23:59:59.999Z"), "Fri, 31 Dec 9999 23:59:59 GMT", 253402300799999.0, 300.0, false),
            ("9999-12-31T23:59:59.999Z", Number(-90.0), "9999-12-31T22:29:59.999-01:30", Some("9999-12-31T23:59:59.999Z"), "Fri, 31 Dec 9999 23:59:59 GMT", 253402300799999.0, -90.0, false),
            ("9999-12-31T23:59:59.999Z", Number(17.00001), "10000-01-01T00:16:59.999+00:17.00001", Some("9999-12-31T23:59:59.998Z"), "Fri, 31 Dec 9999 23:59:59 GMT", 253402300799998.4, 17.00001, false),
        ];
        for (date, offset, format_json, iso, js_string, epoch_ms, utc_offset, utc) in rows {
            let d = Dayjs::utc_parse(date).utc_offset_set(&offset);
            let case = format!("{date}.utcOffset({offset:?})");
            assert_eq!(d.format_json(), format_json, "{case}");
            assert_eq!(d.to_iso_string().as_deref(), iso, "{case}");
            assert_eq!(d.to_js_string(), js_string, "{case}");
            assert_eq!(bits(d.epoch_ms()), bits(epoch_ms), "{case}");
            assert_eq!(bits(d.utc_offset()), bits(utc_offset), "{case}");
            assert_eq!(d.is_utc(), utc, "{case}");
        }
    }

    /// Invalid dates, and the ends of the ECMAScript range.
    #[test]
    fn edge_dates_as_the_dayjs_emulation_did() {
        let rows = [
            ("utc_invalid", Dayjs::utc_invalid(), "Invalid Date", None, "Invalid Date", f64::NAN, 0.0, true),
            ("from_recorded(false)", Dayjs::from_recorded(false, None, 0.0, true), "Invalid Date", None, "Invalid Date", f64::NAN, -0.0, false),
            ("8.64e15 at +01:00", Dayjs::utc_from_number(8.64e15).utc_offset_set(&UtcOffset::Number(60.0)), "Invalid Date", None, "Invalid Date", f64::NAN, 60.0, false),
            ("-8.64e15", Dayjs::utc_from_number(-8.64e15), "-271821-04-20T00:00:00.000Z", Some("-271821-04-20T00:00:00.000Z"), "Tue, 20 Apr -271821 00:00:00 GMT", -8.64e15, 0.0, true),
            ("8.64e15", Dayjs::utc_from_number(8.64e15), "275760-09-13T00:00:00.000Z", Some("+275760-09-13T00:00:00.000Z"), "Sat, 13 Sep 275760 00:00:00 GMT", 8.64e15, 0.0, true),
            ("recorded local", Dayjs::from_recorded(true, Some("2021-01-01T00:00:00.000Z"), 0.0, false), "2021-01-01T00:00:00.000Z", Some("2021-01-01T00:00:00.000Z"), "Fri, 01 Jan 2021 00:00:00 GMT", 1609459200000.0, -0.0, false),
        ];
        for (case, d, format_json, iso, js_string, epoch_ms, utc_offset, utc) in rows {
            assert_eq!(d.format_json(), format_json, "{case}");
            assert_eq!(d.to_iso_string().as_deref(), iso, "{case}");
            assert_eq!(d.to_js_string(), js_string, "{case}");
            assert_eq!(bits(d.epoch_ms()), bits(epoch_ms), "{case}");
            assert_eq!(bits(d.utc_offset()), bits(utc_offset), "{case}");
            assert_eq!(d.is_utc(), utc, "{case}");
        }
    }
}
