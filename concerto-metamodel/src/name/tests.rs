use super::*;

#[test]
fn a_name_read_from_the_source_shares_it() {
    let source: Arc<str> = Arc::from(r#"{"name":"Person","other":"x\"y"}"#);
    let (shared, escaped) = with_source(&source, || {
        let value: std::collections::BTreeMap<String, Name> =
            serde_json::from_str(&source).unwrap();
        (value["name"].clone(), value["other"].clone())
    });
    assert_eq!(shared, "Person");
    assert!(Arc::ptr_eq(&shared.text, &source));
    assert_eq!(escaped, "x\"y");
    assert!(!Arc::ptr_eq(&escaped.text, &source));
    // Outside `with_source`, a name is a copy.
    let copy: Name = serde_json::from_str(r#""Person""#).unwrap();
    assert_eq!(copy, shared);
    assert!(!Arc::ptr_eq(&copy.text, &source));
}

#[test]
fn a_name_reads_compares_and_round_trips_as_its_text() {
    let name = Name::from("abc");
    assert_eq!(&*name, "abc");
    assert_eq!(name, String::from("abc"));
    assert!("abc" == name);
    assert_eq!(name.to_string(), "abc");
    assert_eq!(format!("{name:?}"), "\"abc\"");
    assert_eq!(serde_json::to_string(&name).unwrap(), "\"abc\"");
    assert_eq!(Name::default(), "");
    let (a, b) = (Name::from("a"), Name::from("b"));
    assert!(a < b);
    assert!(serde_json::from_str::<Name>("1").is_err());
}

#[test]
fn a_name_from_a_string_holds_its_text() {
    let name = Name::from(String::from("Person"));
    assert_eq!(name, "Person");
    assert_eq!(name.as_str(), "Person");
    let empty = Name::from(String::new());
    assert!(Arc::ptr_eq(&empty.text, &Name::default().text));
}
