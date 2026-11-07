//! Lossless TOML storage for full-width identifiers and capability bitsets.
//! Read legacy nonnegative integers; write exactly 16 uppercase hex digits.
use serde::{de, Deserializer, Serializer};
use std::fmt;

pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("{value:016X}"))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    struct Visitor;
    impl de::Visitor<'_> for Visitor {
        type Value = u64;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("16 uppercase hexadecimal digits or a legacy nonnegative integer")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<u64, E> {
            if value.len() != 16
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
            {
                return Err(E::custom(
                    "expected exactly 16 uppercase hexadecimal digits",
                ));
            }
            u64::from_str_radix(value, 16).map_err(E::custom)
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<u64, E> {
            u64::try_from(value).map_err(|_| E::custom("negative legacy integer"))
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<u64, E> {
            Ok(value)
        }
    }
    deserializer.deserialize_any(Visitor)
}
