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
    Number(f64),
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

    /// `dayjs.utc(date)` for a string `date`: dayjs's `parseDate` with
    /// `utc: true`.
    ///
    /// A string that does not end in `Z` (case-insensitively) and matches
    /// dayjs's `REGEX_PARSE` is built with `Date.UTC` from its parts;
    /// anything else goes to `new Date(string)`, ECMAScript `Date.parse`
    /// ([`date_parse`]).
    pub fn utc_parse(s: &str) -> Self {
        Self::with_time(parse_date_utc(s), true)
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
        let iso = iso.unwrap_or_default();
        if utc {
            let d = Self::utc_parse(iso);
            if offset == 0.0 {
                d
            } else {
                d.utc_offset_set(&UtcOffset::Number(offset))
            }
        } else {
            let local = Self::with_time(parse_date_utc(iso), false);
            if local.utc_offset() == offset {
                local
            } else {
                local.utc_offset_set(&UtcOffset::Number(offset))
            }
        }
    }

    /// `Date.parse(s)` under `TZ=UTC`, as a time value (`NaN` when it does
    /// not parse): the ECMAScript date time string format ([`date_parse`]).
    pub fn parse_instant(s: &str) -> f64 {
        date_parse(s)
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
    let re = regress::Regex::new(r"[+-]\d\d(?::?\d\d)?").expect("static pattern");
    let m = re.find(value)?;
    let offset = &value[m.range];
    // `("" + offset[0]).match(/([+-]|\d\d)/g) || ['-', 0, 0]`
    let parts_re = regress::Regex::new(r"([+-]|\d\d)").expect("static pattern");
    let parts: Vec<&str> = parts_re
        .find_iter(offset)
        .map(|m| &offset[m.range])
        .collect();
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

/// dayjs `parseDate` for a string with `utc: true`, under `TZ=UTC`.
fn parse_date_utc(s: &str) -> f64 {
    let ends_with_z = s.ends_with('Z') || s.ends_with('z');
    if !ends_with_z {
        // C.REGEX_PARSE
        let re = regress::Regex::new(
            r"^(\d{4})[-/]?(\d{1,2})?[-/]?(\d{0,2})[Tt\s]*(\d{1,2})?:?(\d{1,2})?:?(\d{1,2})?[.:]?(\d+)?$",
        )
        .expect("static pattern");
        if let Some(m) = re.find(s) {
            let group = |i: usize| m.group(i).map(|r| &s[r]);
            let num = |g: Option<&str>| g.map_or(f64::NAN, ecma::string_to_number);
            // `const m = d[2] - 1 || 0`
            let month = {
                let v = num(group(2)) - 1.0;
                if v == 0.0 || v.is_nan() { 0.0 } else { v }
            };
            // `(d[7] || '0').substring(0, 3)`
            let ms_text: String = group(7)
                .filter(|t| !t.is_empty())
                .unwrap_or("0")
                .chars()
                .take(3)
                .collect();
            // `d[3] || 1`, `d[4] || 0`, ...: a present, non-empty group is
            // truthy as a string, whatever its digits.
            let or = |g: Option<&str>, default: f64| match g {
                Some(t) if !t.is_empty() => ecma::string_to_number(t),
                _ => default,
            };
            return date_utc(
                num(group(1)),
                month,
                or(group(3), 1.0),
                or(group(4), 0.0),
                or(group(5), 0.0),
                or(group(6), 0.0),
                ecma::string_to_number(&ms_text),
            );
        }
    }
    date_parse(s)
}

/// ECMAScript `ToIntegerOrInfinity`.
fn to_integer_or_infinity(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n.trunc() }
}

/// ECMAScript `Date.UTC(year, month, date, hours, minutes, seconds, ms)`.
fn date_utc(year: f64, month: f64, date: f64, h: f64, min: f64, s: f64, ms: f64) -> f64 {
    let year = if !year.is_nan() {
        let yi = to_integer_or_infinity(year);
        if (0.0..=99.0).contains(&yi) {
            1900.0 + yi
        } else {
            year
        }
    } else {
        year
    };
    time_clip(make_date(
        make_day(year, month, date),
        make_time(h, min, s, ms),
    ))
}

/// ECMAScript `MakeTime`.
fn make_time(hour: f64, min: f64, sec: f64, ms: f64) -> f64 {
    if !hour.is_finite() || !min.is_finite() || !sec.is_finite() || !ms.is_finite() {
        return f64::NAN;
    }
    to_integer_or_infinity(hour) * 3_600_000.0
        + to_integer_or_infinity(min) * MS_PER_MINUTE
        + to_integer_or_infinity(sec) * 1000.0
        + to_integer_or_infinity(ms)
}

/// ECMAScript `MakeDay`.
fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !year.is_finite() || !month.is_finite() || !date.is_finite() {
        return f64::NAN;
    }
    let y = to_integer_or_infinity(year);
    let m = to_integer_or_infinity(month);
    let dt = to_integer_or_infinity(date);
    let ym = y + (m / 12.0).floor();
    if ym.abs() > 400_000.0 {
        return f64::NAN;
    }
    let mn = m.rem_euclid(12.0);
    days_from_civil(ym as i64, mn as i64 + 1, 1) as f64 + dt - 1.0
}

/// ECMAScript `MakeDate`.
fn make_date(day: f64, time: f64) -> f64 {
    if !day.is_finite() || !time.is_finite() {
        return f64::NAN;
    }
    day * MS_PER_DAY + time
}

/// ECMAScript `TimeClip`.
fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > 8.64e15 {
        return f64::NAN;
    }
    // `ToIntegerOrInfinity`, which also turns -0 into +0.
    to_integer_or_infinity(time) + 0.0
}

/// Days since 1970-01-01 of a proleptic Gregorian date (month 1-12).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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

/// `new Date(string)` under `TZ=UTC`: ECMAScript's date time string format
/// (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`, each optionally followed by
/// `THH:mm`, `THH:mm:ss` or `THH:mm:ss.sss`, and `Z` or `±HH:mm`; expanded
/// `±YYYYYY` years), with the V8 extensions the recorded corpus reaches:
/// any number of fraction digits (truncated to milliseconds), `24:00`, a
/// space or a lower-case `t` instead of `T`, a lower-case `z`, a `±HHmm`
/// offset and a bare `Z` after a date; and, from V8's legacy fallback
/// parser, a date of numbers alone ([`legacy_numeric_date`]). Anything else
/// is `NaN` (the rest of V8's legacy parser is not ported; DIVERGENCES.md
/// DV-009).
fn date_parse(s: &str) -> f64 {
    // V8's date tokenizer treats U+0000 as end of input, so `new
    // Date(string)` parses only the part before the first NUL and ignores
    // whatever follows it (DV-009; accordproject/concerto-rust#169). Mirror
    // that here, ahead of both the ECMAScript date time string format match
    // and the legacy numeric-date fallback below.
    let s = match s.find('\u{0}') {
        Some(i) => &s[..i],
        None => s,
    };
    let re = regress::Regex::new(
        r"^([+-]\d{6}|\d{4})(?:-(\d{2})(?:-(\d{2}))?)?(?:[Tt ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d+))?)?)?(Z|z|[+-]\d{2}:?\d{2})?$",
    )
    .expect("static pattern");
    let Some(m) = re.find(s) else {
        return legacy_numeric_date(s);
    };
    let group = |i: usize| m.group(i).map(|r| &s[r]);
    let year_text = group(1).unwrap_or_default();
    if year_text == "-000000" {
        return f64::NAN;
    }
    let year = ecma::string_to_number(year_text);
    let month = group(2).map_or(1.0, ecma::string_to_number);
    let day = group(3).map_or(1.0, ecma::string_to_number);
    let hour = group(4).map_or(0.0, ecma::string_to_number);
    let minute = group(5).map_or(0.0, ecma::string_to_number);
    let second = group(6).map_or(0.0, ecma::string_to_number);
    let ms = group(7).map_or(0.0, |f| {
        let digits: String = f.chars().chain("000".chars()).take(3).collect();
        ecma::string_to_number(&digits)
    });
    if !(1.0..=12.0).contains(&month) || !(1.0..=31.0).contains(&day) {
        return f64::NAN;
    }
    if minute > 59.0 || second > 59.0 {
        return f64::NAN;
    }
    if hour > 24.0 || (hour == 24.0 && (minute != 0.0 || second != 0.0 || ms != 0.0)) {
        return f64::NAN;
    }
    let offset_minutes = match group(8) {
        None | Some("Z") | Some("z") => 0.0,
        Some(z) => {
            let sign = if z.starts_with('-') { -1.0 } else { 1.0 };
            let digits: String = z[1..].chars().filter(char::is_ascii_digit).collect();
            let hh = ecma::string_to_number(&digits[..2]);
            let mm = ecma::string_to_number(&digits[2..]);
            if hh > 23.0 || mm > 59.0 {
                return f64::NAN;
            }
            sign * (hh * 60.0 + mm)
        }
    };
    let day_number = make_day(year, month - 1.0, day);
    let time = make_time(hour, minute, second, ms);
    time_clip(make_date(day_number, time) - offset_minutes * MS_PER_MINUTE)
}

/// V8's legacy date parser (`DateParser::Parse`, `DayComposer::Write`) for
/// a date written as two or three numbers alone, separated by `-` or `/`
/// (leading `-` signs are skipped): three numbers are year, month, day when
/// the first cannot be a day (outside 1-31), and month, day, year
/// otherwise; two numbers are month and day, in V8's default year 2001. A
/// year 0-49 is 20xx and 50-99 is 19xx. The time is midnight, local time,
/// which is UTC here.
fn legacy_numeric_date(s: &str) -> f64 {
    let re = regress::Regex::new(r"^-*(\d+)[-/](\d+)(?:[-/](\d+))?$").expect("static pattern");
    let Some(m) = re.find(s) else {
        return f64::NAN;
    };
    let num = |i: usize| m.group(i).map(|r| ecma::string_to_number(&s[r]));
    let (Some(a), Some(b)) = (num(1), num(2)) else {
        return f64::NAN;
    };
    let is_day = |n: f64| (1.0..=31.0).contains(&n);
    let (mut year, month, day) = match num(3) {
        Some(c) if !is_day(a) => (a, b, c),
        Some(c) => (c, a, b),
        None => (2001.0, a, b),
    };
    if (0.0..=49.0).contains(&year) {
        year += 2000.0;
    } else if (50.0..=99.0).contains(&year) {
        year += 1900.0;
    }
    if !(1.0..=12.0).contains(&month) || !is_day(day) {
        return f64::NAN;
    }
    time_clip(make_date(make_day(year, month - 1.0, day), 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn utc_parse_regex_path_and_iso() {
        let d = Dayjs::utc_parse("2021-01-01T00:00:00");
        assert_eq!(
            d.to_iso_string().as_deref(),
            Some("2021-01-01T00:00:00.000Z")
        );
        // Two-digit years go through Date.UTC's 1900 mapping.
        let d = Dayjs::utc_parse("0050-01-01");
        assert_eq!(
            d.to_iso_string().as_deref(),
            Some("1950-01-01T00:00:00.000Z")
        );
        // A trailing Z goes to Date.parse.
        let d = Dayjs::utc_parse("2021-01-01T10:20:30.123456Z");
        assert_eq!(
            d.to_iso_string().as_deref(),
            Some("2021-01-01T10:20:30.123Z")
        );
        // An offset is not matched by REGEX_PARSE.
        let d = Dayjs::utc_parse("2021-01-01T10:00:00+05:00");
        assert_eq!(
            d.to_iso_string().as_deref(),
            Some("2021-01-01T05:00:00.000Z")
        );
        assert!(!Dayjs::utc_parse("not a date").is_valid());
        assert!(!Dayjs::utc_parse("2021-13-01T00:00:00Z").is_valid());
        // Date.parse rolls an out-of-range day over, as V8 does.
        let d = Dayjs::utc_parse("2021-02-30T00:00:00Z");
        assert_eq!(
            d.to_iso_string().as_deref(),
            Some("2021-03-02T00:00:00.000Z")
        );
    }

    #[test]
    fn legacy_numeric_dates_match_v8() {
        let iso = |s: &str| Dayjs::utc_parse(s).to_iso_string();
        assert_eq!(iso("--11-28").as_deref(), Some("2001-11-28T00:00:00.000Z"));
        assert_eq!(iso("11-28").as_deref(), Some("2001-11-28T00:00:00.000Z"));
        assert_eq!(
            iso("2022-11-28t01:02:03.987Z").as_deref(),
            Some("2022-11-28T01:02:03.987Z")
        );
        assert_eq!(
            Dayjs::parse_instant("--11-28"),
            Dayjs::parse_instant("2001-11-28")
        );
        assert_eq!(
            Dayjs::parse_instant("11/28/2022"),
            Dayjs::parse_instant("2022-11-28")
        );
        assert_eq!(
            Dayjs::parse_instant("--2022-11-28"),
            Dayjs::parse_instant("2022-11-28")
        );
        assert!(Dayjs::parse_instant("13-28").is_nan());
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

    /// DV-009 / accordproject/concerto-rust#169 (P5-05 fuzz cluster T1c): an
    /// embedded NUL truncates `new Date(string)`'s input, as V8's date
    /// tokenizer does, instead of failing the whole parse.
    #[test]
    fn embedded_nul_truncates_like_v8() {
        let iso = |s: &str| Dayjs::utc_parse(s).to_iso_string();
        assert_eq!(
            iso("1970-01-01T00:00:00.000+00:00\u{0}").as_deref(),
            Some("1970-01-01T00:00:00.000Z")
        );
        assert_eq!(
            iso("1970-01-01T00:00:00.000Z\u{0}").as_deref(),
            Some("1970-01-01T00:00:00.000Z")
        );
        // Anything after the NUL is ignored, exactly as it is by V8.
        assert_eq!(
            iso("1970-01-01T00:00:00.000+00:00\u{0}junk").as_deref(),
            Some("1970-01-01T00:00:00.000Z")
        );
        assert_eq!(
            iso("1970-01-01\u{0}").as_deref(),
            Some("1970-01-01T00:00:00.000Z")
        );
        // A control character other than NUL gets no special treatment: it
        // fails to parse in both TS and Rust.
        assert!(!Dayjs::utc_parse("1970-01-01T00:00:00.000+00:00\u{1}").is_valid());
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
