use std::borrow::Cow;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, Visitor};

use super::{Class, ErrorBridge, PropertySeed, property_kind, read_class, read_property_struct};

/// Reads one property node as every property node was read before
/// P5-93: `$class`, then the rest by its generated struct.
struct ByStruct;

impl<'de> DeserializeSeed<'de> for ByStruct {
    type Value = String;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<String, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ByStruct {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a property object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<String, A::Error> {
        let mut map = ErrorBridge(map);
        let mut read = || -> Result<String, serde_json::Error> {
            let class: Cow<'de, str> = match read_class(&mut map)? {
                Class::First(class) => class,
                Class::Reordered(_) => return Ok("reordered".to_string()),
            };
            let Some(kind) = property_kind(&class) else {
                return Err(de::Error::custom(format_args!(
                    "unrecognised property $class {class}"
                )));
            };
            let mut decorators = None;
            let mut location = None;
            let (property, taken) =
                read_property_struct(&mut map, &class, kind, &mut decorators, None, &mut location)?;
            Ok(format!("{:?} {taken:?} {location:?}", Some(property)))
        };
        read().map_err(de::Error::custom)
    }
}

/// Reads one property node as the typed read reads it.
fn as_read(text: &str) -> String {
    let mut properties = Vec::new();
    let mut kept = Vec::new();
    let read = PropertySeed {
        properties: &mut properties,
        kept: &mut kept,
    }
    .deserialize(&mut serde_json::Deserializer::from_str(text));
    match read {
        Ok(()) => {
            let kept = kept.pop().unwrap_or_default();
            format!(
                "{:?} {:?} {:?}",
                properties.pop(),
                kept.date_time_default,
                kept.location
            )
        }
        Err(err) => format!("error {err}"),
    }
}

fn by_struct(text: &str) -> String {
    match ByStruct.deserialize(&mut serde_json::Deserializer::from_str(text)) {
        Ok(read) => read,
        Err(err) => format!("error {err}"),
    }
}

const MM: &str = "concerto.metamodel@1.0.0";

/// P5-93: a property node's own keys, read by `read_property` itself,
/// give exactly the property, and the error, its generated struct
/// gives, whatever the node; from the first other key the struct reads
/// the rest, handed the values read before it.
#[test]
fn a_property_reads_as_its_generated_struct_reads_it() {
    let range = r#"{"$class":"concerto.metamodel@1.0.0.Range","start":{"offset":1,"line":1,"column":1,"$class":"concerto.metamodel@1.0.0.Position"},"end":{"offset":2,"line":1,"column":2,"$class":"concerto.metamodel@1.0.0.Position"}}"#;
    let decorators = r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"D","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x"}]}]"#;
    let type_ = r#"{"$class":"concerto.metamodel@1.0.0.TypeIdentifier","name":"T","namespace":"org.x@1.0.0"}"#;
    let nodes = [
            ("StringProperty", r#""name":"a","isArray":false,"isOptional":true"#.to_string()),
            ("StringProperty", r#""isOptional":true,"name":"a""#.to_string()),
            ("StringProperty", r#""name":"ab","isArray":true"#.to_string()),
            ("StringProperty", format!(r#""name":"a","decorators":{decorators},"location":{range}"#)),
            ("StringProperty", format!(r#""location":{range},"name":"a","decorators":[]"#)),
            ("StringProperty", r#""name":"a","decorators":null,"location":null"#.to_string()),
            ("StringProperty", r#""name":"a","location":{"start":1}"#.to_string()),
            ("StringProperty", r#""name":"a","decorators":[{"name":"D"}]"#.to_string()),
            ("StringProperty", r#""name":"a","validator":{"$class":"concerto.metamodel@1.0.0.StringRegexValidator","pattern":"x","flags":""},"isArray":true"#.to_string()),
            ("StringProperty", format!(r#""isArray":true,"name":"a","decorators":{decorators},"lengthValidator":{{"$class":"concerto.metamodel@1.0.0.StringLengthValidator","maxLength":3}},"isOptional":true"#)),
            ("StringProperty", r#""name":"a","defaultValue":"x""#.to_string()),
            ("StringProperty", r#""name":"a","sizeValidator":{"$class":"concerto.metamodel@1.0.0.CollectionSizeValidator","minSize":1},"name":"b""#.to_string()),
            ("StringProperty", r#""name":"a","name":"b""#.to_string()),
            ("StringProperty", r#""name":"a","isArray":true,"isArray":false"#.to_string()),
            ("StringProperty", r#""name":"a","decorators":[],"decorators":[]"#.to_string()),
            ("StringProperty", r#""decorators":[],"defaultValue":"x","decorators":[]"#.to_string()),
            ("StringProperty", format!(r#""location":{range},"defaultValue":"x","name":"a","location":null"#)),
            ("StringProperty", format!(r#""name":"a","location":{range},"location":null"#)),
            ("StringProperty", r#""isArray":false"#.to_string()),
            ("StringProperty", r#""name":1"#.to_string()),
            ("StringProperty", r#""name":"a","isArray":"yes""#.to_string()),
            ("StringProperty", r#""name":"a","isOptional":null"#.to_string()),
            ("StringProperty", r#""name":"a","extra":1"#.to_string()),
            ("StringProperty", r#""extra":1,"name":"a""#.to_string()),
            ("StringProperty", r#""name":"a","$class":"x""#.to_string()),
            ("StringProperty", r#""name":"a","type":{}"#.to_string()),
            ("BooleanProperty", r#""name":"a","defaultValue":true"#.to_string()),
            ("IntegerProperty", r#""name":"a","validator":{"$class":"concerto.metamodel@1.0.0.IntegerDomainValidator","lower":1},"isOptional":true"#.to_string()),
            ("LongProperty", r#""name":"a","isArray":true"#.to_string()),
            ("DoubleProperty", r#""name":"a","defaultValue":1.5"#.to_string()),
            ("DateTimeProperty", r#""name":"a","defaultValue":"2020-01-01T00:00:00Z","isOptional":true"#.to_string()),
            ("DateTimeProperty", r#""name":"a","isOptional":true"#.to_string()),
            ("ObjectProperty", format!(r#""name":"a","type":{type_},"isArray":true"#)),
            ("ObjectProperty", format!(r#""type":{type_},"name":"a","defaultValue":"x""#)),
            ("ObjectProperty", format!(r#""type":{type_},"type":{type_},"name":"a""#)),
            ("ObjectProperty", r#""name":"a""#.to_string()),
            ("ObjectProperty", r#""isArray":true"#.to_string()),
            ("ObjectProperty", r#""name":"a","type":{"name":"T"}"#.to_string()),
            ("ObjectProperty", r#""name":"a","type":{"$class":"concerto.metamodel@1.0.0.TypeIdentifier","name":"T","extra":1}"#.to_string()),
            ("ObjectProperty", r#""name":"a","type":"T""#.to_string()),
            ("RelationshipProperty", format!(r#""name":"a","type":{type_},"decorators":{decorators}"#)),
            ("RelationshipProperty", format!(r#""name":"a","type":{type_},"defaultValue":"x""#)),
            ("EnumProperty", r#""name":"A""#.to_string()),
            ("EnumProperty", format!(r#""name":"A","decorators":{decorators},"location":{range}"#)),
            ("EnumProperty", r#""name":"A","isArray":false"#.to_string()),
            ("EnumProperty", r#""decorators":[],"name":"A","type":{}"#.to_string()),
        ];
    for (kind, rest) in nodes {
        let text = format!(r#"{{"$class":"{MM}.{kind}",{rest}}}"#);
        assert_eq!(as_read(&text), by_struct(&text), "{text}");
    }
}
