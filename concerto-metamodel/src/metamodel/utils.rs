use chrono::{ DateTime, Utc };
use serde::{ Deserialize, Serialize, Deserializer, Serializer };
   
pub fn serialize_datetime_option<S>(datetime: &Option<chrono::DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   match datetime {
      Some(dt) => {
         serialize_datetime(&dt, serializer)
      },
      _ => unreachable!(),
   }
}

pub fn deserialize_datetime_option<'de, D>(deserializer: D) -> Result<Option<chrono::DateTime<Utc>>, D::Error>
where
   D: Deserializer<'de>,
{
   match deserialize_datetime(deserializer) {
      Ok(result)=>Ok(Some(result)),
      Err(error) => Err(error),
   }
}

pub fn deserialize_datetime<'de, D>(deserializer: D) -> Result<chrono::DateTime<Utc>, D::Error>
where
   D: Deserializer<'de>,
{
   let datetime_str = String::deserialize(deserializer)?;
   DateTime::parse_from_str(&datetime_str, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)
}
   
pub fn serialize_datetime<S>(datetime: &chrono::DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   let datetime_str = datetime.format("%+").to_string();
   serializer.serialize_str(&datetime_str)
}

pub fn serialize_datetime_array<S>(datetime_array: &Vec<chrono::DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   let datetime_strings: Vec<String> = datetime_array
      .iter()
      .map(|dt| dt.format("%+").to_string())
      .collect();
   datetime_strings.serialize(serializer)
}

pub fn deserialize_datetime_array<'de, D>(deserializer: D) -> Result<Vec<chrono::DateTime<Utc>>, D::Error>
where
   D: Deserializer<'de>,
{
   let datetime_strings = Vec::<String>::deserialize(deserializer)?;
   datetime_strings
      .iter()
      .map(|s| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom))
      .collect()
}

pub fn serialize_datetime_array_option<S>(datetime_array: &Option<Vec<chrono::DateTime<Utc>>>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   match datetime_array {
      Some(arr) => {
         serialize_datetime_array(&arr, serializer)
      },
      None => serializer.serialize_none(),
   }
}

pub fn deserialize_datetime_array_option<'de, D>(deserializer: D) -> Result<Option<Vec<chrono::DateTime<Utc>>>, D::Error>
where
   D: Deserializer<'de>,
{
   match Option::<Vec<String>>::deserialize(deserializer)? {
      Some(datetime_strings) => {
         let result: Result<Vec<_>, _> = datetime_strings
            .iter()
            .map(|s| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom))
            .collect();
         result.map(Some)
      },
      None => Ok(None),
   }
}

pub fn serialize_hashmap_datetime_key<S>(hashmap: &std::collections::HashMap<chrono::DateTime<Utc>, String>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   let string_map: std::collections::HashMap<String, String> = hashmap
      .iter()
      .map(|(k, v)| (k.format("%+").to_string(), v.clone()))
      .collect();
   string_map.serialize(serializer)
}

pub fn deserialize_hashmap_datetime_key<'de, D>(deserializer: D) -> Result<std::collections::HashMap<chrono::DateTime<Utc>, String>, D::Error>
where
   D: Deserializer<'de>,
{
   let string_map = std::collections::HashMap::<String, String>::deserialize(deserializer)?;
   let mut result = std::collections::HashMap::new();
   for (k, v) in string_map {
      let datetime_key = DateTime::parse_from_str(&k, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)?;
      result.insert(datetime_key, v);
   }
   Ok(result)
}

pub fn serialize_hashmap_datetime_value<S>(hashmap: &std::collections::HashMap<String, chrono::DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   let string_map: std::collections::HashMap<String, String> = hashmap
      .iter()
      .map(|(k, v)| (k.clone(), v.format("%+").to_string()))
      .collect();
   string_map.serialize(serializer)
}

pub fn deserialize_hashmap_datetime_value<'de, D>(deserializer: D) -> Result<std::collections::HashMap<String, chrono::DateTime<Utc>>, D::Error>
where
   D: Deserializer<'de>,
{
   let string_map = std::collections::HashMap::<String, String>::deserialize(deserializer)?;
   let mut result = std::collections::HashMap::new();
   for (k, v) in string_map {
      let datetime_value = DateTime::parse_from_str(&v, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)?;
      result.insert(k, datetime_value);
   }
   Ok(result)
}

pub fn serialize_hashmap_datetime_both<S>(hashmap: &std::collections::HashMap<chrono::DateTime<Utc>, chrono::DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   let string_map: std::collections::HashMap<String, String> = hashmap
      .iter()
      .map(|(k, v)| (k.format("%+").to_string(), v.format("%+").to_string()))
      .collect();
   string_map.serialize(serializer)
}

pub fn deserialize_hashmap_datetime_both<'de, D>(deserializer: D) -> Result<std::collections::HashMap<chrono::DateTime<Utc>, chrono::DateTime<Utc>>, D::Error>
where
   D: Deserializer<'de>,
{
   let string_map = std::collections::HashMap::<String, String>::deserialize(deserializer)?;
   let mut result = std::collections::HashMap::new();
   for (k, v) in string_map {
      let datetime_key = DateTime::parse_from_str(&k, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)?;
      let datetime_value = DateTime::parse_from_str(&v, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)?;
      result.insert(datetime_key, datetime_value);
   }
   Ok(result)
}

pub fn serialize_hashmap_datetime_key_option<S>(hashmap: &Option<std::collections::HashMap<chrono::DateTime<Utc>, String>>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   match hashmap {
      Some(map) => serialize_hashmap_datetime_key(map, serializer),
      None => serializer.serialize_none(),
   }
}

pub fn deserialize_hashmap_datetime_key_option<'de, D>(deserializer: D) -> Result<Option<std::collections::HashMap<chrono::DateTime<Utc>, String>>, D::Error>
where
   D: Deserializer<'de>,
{
   match Option::<std::collections::HashMap<String, String>>::deserialize(deserializer)? {
      Some(string_map) => {
         let mut result = std::collections::HashMap::new();
         for (k, v) in string_map {
            let datetime_key = DateTime::parse_from_str(&k, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)?;
            result.insert(datetime_key, v);
         }
         Ok(Some(result))
      },
      None => Ok(None),
   }
}

pub fn serialize_hashmap_datetime_value_option<S>(hashmap: &Option<std::collections::HashMap<String, chrono::DateTime<Utc>>>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   match hashmap {
      Some(map) => serialize_hashmap_datetime_value(map, serializer),
      None => serializer.serialize_none(),
   }
}

pub fn deserialize_hashmap_datetime_value_option<'de, D>(deserializer: D) -> Result<Option<std::collections::HashMap<String, chrono::DateTime<Utc>>>, D::Error>
where
   D: Deserializer<'de>,
{
   match Option::<std::collections::HashMap<String, String>>::deserialize(deserializer)? {
      Some(string_map) => {
         let mut result = std::collections::HashMap::new();
         for (k, v) in string_map {
            let datetime_value = DateTime::parse_from_str(&v, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)?;
            result.insert(k, datetime_value);
         }
         Ok(Some(result))
      },
      None => Ok(None),
   }
}

pub fn serialize_hashmap_datetime_both_option<S>(hashmap: &Option<std::collections::HashMap<chrono::DateTime<Utc>, chrono::DateTime<Utc>>>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   match hashmap {
      Some(map) => serialize_hashmap_datetime_both(map, serializer),
      None => serializer.serialize_none(),
   }
}

pub fn deserialize_hashmap_datetime_both_option<'de, D>(deserializer: D) -> Result<Option<std::collections::HashMap<chrono::DateTime<Utc>, chrono::DateTime<Utc>>>, D::Error>
where
   D: Deserializer<'de>,
{
   match Option::<std::collections::HashMap<String, String>>::deserialize(deserializer)? {
      Some(string_map) => {
         let mut result = std::collections::HashMap::new();
         for (k, v) in string_map {
            let datetime_key = DateTime::parse_from_str(&k, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)?;
            let datetime_value = DateTime::parse_from_str(&v, "%Y-%m-%dT%H:%M:%S%.3f%Z").map(|dt| dt.with_timezone(&Utc)).map_err(serde::de::Error::custom)?;
            result.insert(datetime_key, datetime_value);
         }
         Ok(Some(result))
      },
      None => Ok(None),
   }
}
