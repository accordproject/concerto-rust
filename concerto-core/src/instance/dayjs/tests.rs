use super::*;

fn iso(s: &str) -> Option<String> {
    Dayjs::utc_parse(s).to_iso_string()
}

/// `epoch_ms()` round-trips through `utc_from_number`/`utc_offset_set`,
/// the pair the Serializer fast path's wire codec crosses the WASM
/// boundary with, and is `NaN` for an invalid date.
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

/// BC-07: every lenient form dayjs and V8 accept is invalid.
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

/// BC-42: the fields must name a real calendar instant. No
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
    assert_eq!(
        d.to_iso_string().as_deref(),
        Some("2021-01-01T00:00:00.000Z")
    );
    let d = Dayjs::from_recorded(true, Some("-000001-12-31T23:00:00.000Z"), 60.0, true);
    assert_eq!(
        d.to_iso_string().as_deref(),
        Some("-000001-12-31T23:00:00.000Z")
    );
    assert_eq!(d.format_json(), "0000-01-01T00:00:00.000+01:00");
    let d = Dayjs::from_recorded(true, Some("+010000-01-01T00:00:00.000Z"), 0.0, true);
    assert_eq!(
        d.to_iso_string().as_deref(),
        Some("+010000-01-01T00:00:00.000Z")
    );
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
    let z = Dayjs::utc_parse("2021-01-01T00:00:00Z").utc_offset_set(&UtcOffset::String("Z".into()));
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
