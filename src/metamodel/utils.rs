use chrono::{ DateTime, TimeZone, Utc };
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
   Utc.datetime_from_str(&datetime_str, "%Y-%m-%dT%H:%M:%S%.3f%Z").map_err(serde::de::Error::custom)
}
   
pub fn serialize_datetime<S>(datetime: &chrono::DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
where
   S: Serializer,
{
   let datetime_str = datetime.format("%+").to_string();
   serializer.serialize_str(&datetime_str)
}
