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
    validator: crate::json::Value,
}

fn bits(n: f64) -> u64 {
    if n.is_nan() {
        f64::NAN.to_bits()
    } else {
        n.to_bits()
    }
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
            if rng.below(8) == 0 {
                60
            } else {
                rng.range(0, 59)
            }
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
const EMULATION_DIGEST: &str = "baf61b9ef99053beb841e3feae667dc650f1e83f135018fac9f94f30738b7b0e";

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
    type Row = (
        &'static str,
        UtcOffset,
        &'static str,
        Option<&'static str>,
        &'static str,
        f64,
        f64,
        bool,
    );
    let rows: [Row; 22] = [
        (
            "2021-06-15T12:34:56.789Z",
            Number(0.0),
            "2021-06-15T12:34:56.789Z",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            0.0,
            true,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(-0.0),
            "2021-06-15T12:34:56.789Z",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            0.0,
            true,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(5.0),
            "2021-06-15T17:34:56.789+05:00",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            300.0,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(-1.0),
            "2021-06-15T11:34:56.789-01:00",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            -60.0,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(16.0),
            "2021-06-16T04:34:56.789+16:00",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            960.0,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(16.5),
            "2021-06-15T12:51:26.789+00:16.5",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            16.5,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(17.0),
            "2021-06-15T12:51:56.789+00:17",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            17.0,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(-90.0),
            "2021-06-15T11:04:56.789-01:30",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            -90.0,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(1500.0),
            "2021-06-16T13:34:56.789+25:00",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            1500.0,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(17.00001),
            "2021-06-15T12:51:56.789+00:17.00001",
            Some("2021-06-15T12:34:56.788Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496788.4,
            17.00001,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Number(f64::NAN),
            "Invalid Date",
            None,
            "Invalid Date",
            f64::NAN,
            f64::NAN,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Str("+05:30".into()),
            "2021-06-15T18:04:56.789+05:30",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            330.0,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Str("-0800".into()),
            "2021-06-15T04:34:56.789-08:00",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            -480.0,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Str("+05".into()),
            "Invalid Date",
            None,
            "Invalid Date",
            f64::NAN,
            f64::NAN,
            false,
        ),
        (
            "2021-06-15T12:34:56.789Z",
            Str("Z".into()),
            "2021-06-15T12:34:56.789Z",
            Some("2021-06-15T12:34:56.789Z"),
            "Tue, 15 Jun 2021 12:34:56 GMT",
            1623760496789.0,
            0.0,
            true,
        ),
        (
            "0000-01-01T00:00:00Z",
            Number(5.0),
            "0000-01-01T05:00:00.000+05:00",
            Some("0000-01-01T00:00:00.000Z"),
            "Sat, 01 Jan 0000 00:00:00 GMT",
            -62167219200000.0,
            300.0,
            false,
        ),
        (
            "0000-01-01T00:00:00Z",
            Number(-1.0),
            "00-1-12-31T23:00:00.000-01:00",
            Some("0000-01-01T00:00:00.000Z"),
            "Sat, 01 Jan 0000 00:00:00 GMT",
            -62167219200000.0,
            -60.0,
            false,
        ),
        (
            "0000-01-01T00:00:00Z",
            Str("-0800".into()),
            "00-1-12-31T16:00:00.000-08:00",
            Some("0000-01-01T00:00:00.000Z"),
            "Sat, 01 Jan 0000 00:00:00 GMT",
            -62167219200000.0,
            -480.0,
            false,
        ),
        (
            "0000-01-01T00:00:00Z",
            Number(17.00001),
            "0000-01-01T00:17:00.001+00:17.00001",
            Some("0000-01-01T00:00:00.001Z"),
            "Sat, 01 Jan 0000 00:00:00 GMT",
            -62167219199999.6,
            17.00001,
            false,
        ),
        (
            "9999-12-31T23:59:59.999Z",
            Number(5.0),
            "10000-01-01T04:59:59.999+05:00",
            Some("9999-12-31T23:59:59.999Z"),
            "Fri, 31 Dec 9999 23:59:59 GMT",
            253402300799999.0,
            300.0,
            false,
        ),
        (
            "9999-12-31T23:59:59.999Z",
            Number(-90.0),
            "9999-12-31T22:29:59.999-01:30",
            Some("9999-12-31T23:59:59.999Z"),
            "Fri, 31 Dec 9999 23:59:59 GMT",
            253402300799999.0,
            -90.0,
            false,
        ),
        (
            "9999-12-31T23:59:59.999Z",
            Number(17.00001),
            "10000-01-01T00:16:59.999+00:17.00001",
            Some("9999-12-31T23:59:59.998Z"),
            "Fri, 31 Dec 9999 23:59:59 GMT",
            253402300799998.4,
            17.00001,
            false,
        ),
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
        (
            "utc_invalid",
            Dayjs::utc_invalid(),
            "Invalid Date",
            None,
            "Invalid Date",
            f64::NAN,
            0.0,
            true,
        ),
        (
            "from_recorded(false)",
            Dayjs::from_recorded(false, None, 0.0, true),
            "Invalid Date",
            None,
            "Invalid Date",
            f64::NAN,
            -0.0,
            false,
        ),
        (
            "8.64e15 at +01:00",
            Dayjs::utc_from_number(8.64e15).utc_offset_set(&UtcOffset::Number(60.0)),
            "Invalid Date",
            None,
            "Invalid Date",
            f64::NAN,
            60.0,
            false,
        ),
        (
            "-8.64e15",
            Dayjs::utc_from_number(-8.64e15),
            "-271821-04-20T00:00:00.000Z",
            Some("-271821-04-20T00:00:00.000Z"),
            "Tue, 20 Apr -271821 00:00:00 GMT",
            -8.64e15,
            0.0,
            true,
        ),
        (
            "8.64e15",
            Dayjs::utc_from_number(8.64e15),
            "275760-09-13T00:00:00.000Z",
            Some("+275760-09-13T00:00:00.000Z"),
            "Sat, 13 Sep 275760 00:00:00 GMT",
            8.64e15,
            0.0,
            true,
        ),
        (
            "recorded local",
            Dayjs::from_recorded(true, Some("2021-01-01T00:00:00.000Z"), 0.0, false),
            "2021-01-01T00:00:00.000Z",
            Some("2021-01-01T00:00:00.000Z"),
            "Fri, 01 Jan 2021 00:00:00 GMT",
            1609459200000.0,
            -0.0,
            false,
        ),
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
