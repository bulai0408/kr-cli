//! Reject duplicate object keys, which serde_json::Value normally overwrites.
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

struct Unique(Value);

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Unique, M::Error> {
                let mut result = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if result.contains_key(&key) {
                        // Key names can themselves contain private text.
                        return Err(de::Error::custom("duplicate JSON object key"));
                    }
                    result.insert(key, map.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(result)))
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Unique, S::Error> {
                let mut result = Vec::new();
                while let Some(value) = seq.next_element::<Unique>()? {
                    result.push(value.0);
                }
                Ok(Unique(Value::Array(result)))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Unique, E> {
                Ok(Unique(Value::String(value.to_owned())))
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Unique, E> {
                Ok(Unique(Value::Bool(value)))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<Unique>(bytes).map(|u| u.0)
}
