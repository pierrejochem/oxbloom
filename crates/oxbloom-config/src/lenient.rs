//! Deserializer over the [`Node`] tree that parses string leaves into the number or bool the
//! target field expects, so environment overrides (always strings) bind to typed fields.
//!
//! Keys match relaxed: a table entry binds the struct field (or unit enum variant) whose
//! canonical form equals the entry's canonical key, and the declared name is emitted. Map keys
//! and unmatched keys are emitted as written.
//!
//! Error messages are built from type names and key names only, never from the offending value.
//! Every `Invalid` error carries the origin of the node it was produced for.

use std::fmt;
use std::time::Duration;

use serde::de::value::MapDeserializer;
use serde::de::{self, DeserializeOwned, Deserializer, IntoDeserializer, Unexpected, Visitor};

use crate::error::ConfigError;
use crate::node::{Entry, Node, NodeValue, canonical};
use crate::origin::Origin;
use crate::units::parse_duration;

/// Binds `node` to `T`; `key` is the dotted path of `node`, used in error messages.
pub(crate) fn deserialize<T: DeserializeOwned>(node: &Node, key: &str) -> Result<T, ConfigError> {
    T::deserialize(Lenient {
        node,
        key: key.to_string(),
    })
    .map_err(|e| match e {
        Error::Missing { field, parent } => ConfigError::Missing {
            key: join(parent.as_deref().unwrap_or(key), &field),
        },
        Error::Invalid {
            key: k,
            message,
            origin,
        } => ConfigError::Invalid {
            key: k.unwrap_or_else(|| key.to_string()),
            message,
            origin,
        },
    })
}

fn join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

/// The declared name that `written` binds to: an exact match first, then the first name with
/// the same canonical form; `written` itself when nothing matches (so `deny_unknown_fields`
/// and unknown-variant errors name it).
fn resolve<'a>(written: &'a str, names: &'static [&'static str]) -> &'a str {
    if names.contains(&written) {
        return written;
    }
    let wanted = canonical(written);
    names
        .iter()
        .copied()
        .find(|name| canonical(name) == wanted)
        .unwrap_or(written)
}

/// Two names in `names` with one canonical form that belong to different fields.
///
/// serde lists each field's names (its name, `rename` and every `alias`) as one sorted run, so
/// equal canonical forms within a strictly ascending run may be aliases of one field and are
/// accepted; a span that is not strictly ascending crosses a field boundary.
fn colliding(names: &[&'static str]) -> Option<(&'static str, &'static str)> {
    let canon: Vec<String> = names.iter().map(|n| canonical(n)).collect();
    for i in 0..names.len() {
        for j in i + 1..names.len() {
            if canon[i] == canon[j] && !names[i..=j].windows(2).all(|w| w[0] < w[1]) {
                return Some((names[i], names[j]));
            }
        }
    }
    None
}

#[derive(Debug)]
enum Error {
    Missing {
        field: String,
        parent: Option<String>,
    },
    Invalid {
        key: Option<String>,
        message: String,
        origin: Option<Origin>,
    },
}

impl Error {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid {
            key: None,
            message: message.into(),
            origin: None,
        }
    }

    /// Attaches `key` and `origin` to an error that does not carry them yet.
    fn at(mut self, key: &str, origin: &Origin) -> Self {
        match &mut self {
            Self::Missing { parent, .. } if parent.is_none() => *parent = Some(key.to_string()),
            Self::Invalid {
                key: k, origin: o, ..
            } => {
                if k.is_none() {
                    *k = Some(key.to_string());
                }
                if o.is_none() {
                    *o = Some(origin.clone());
                }
            }
            _ => {}
        }
        self
    }

    /// Attaches `origin` only, for errors about an entry reported at its table's key.
    fn with_origin(mut self, origin: &Origin) -> Self {
        if let Self::Invalid { origin: o, .. } = &mut self
            && o.is_none()
        {
            *o = Some(origin.clone());
        }
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { field, .. } => write!(f, "missing field `{field}`"),
            Self::Invalid { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

fn kind(u: &Unexpected<'_>) -> &'static str {
    match u {
        Unexpected::Bool(_) => "boolean",
        Unexpected::Unsigned(_) | Unexpected::Signed(_) => "integer",
        Unexpected::Float(_) => "float",
        Unexpected::Char(_) => "character",
        Unexpected::Str(_) => "string",
        Unexpected::Bytes(_) => "bytes",
        Unexpected::Unit => "unit",
        Unexpected::Option => "option",
        Unexpected::NewtypeStruct => "newtype struct",
        Unexpected::Seq => "array",
        Unexpected::Map => "table",
        Unexpected::Enum => "enum",
        Unexpected::UnitVariant
        | Unexpected::NewtypeVariant
        | Unexpected::TupleVariant
        | Unexpected::StructVariant => "variant",
        Unexpected::Other(_) => "other",
    }
}

impl de::Error for Error {
    // Messages from third-party `Deserialize` impls routinely embed the offending value
    // (for example a URL parser echoing the URL), so they are replaced by a fixed text.
    fn custom<T: fmt::Display>(_msg: T) -> Self {
        Self::invalid("invalid value")
    }

    fn unknown_field(field: &str, expected: &'static [&'static str]) -> Self {
        Self::invalid(format!(
            "unknown field `{field}`, expected one of {expected:?}"
        ))
    }

    fn duplicate_field(field: &'static str) -> Self {
        Self::invalid(format!("duplicate field `{field}`"))
    }

    fn invalid_length(len: usize, exp: &dyn de::Expected) -> Self {
        Self::invalid(format!("invalid length {len}, expected {exp}"))
    }

    fn missing_field(field: &'static str) -> Self {
        Self::Missing {
            field: field.to_string(),
            parent: None,
        }
    }

    fn invalid_type(unexp: Unexpected<'_>, exp: &dyn de::Expected) -> Self {
        Self::invalid(format!("invalid type: {}, expected {exp}", kind(&unexp)))
    }

    fn invalid_value(unexp: Unexpected<'_>, exp: &dyn de::Expected) -> Self {
        Self::invalid(format!("invalid value: {}, expected {exp}", kind(&unexp)))
    }

    fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
        Self::invalid(format!("unknown variant, expected one of {expected:?}"))
    }
}

struct Lenient<'de> {
    node: &'de Node,
    key: String,
}

impl<'de> Lenient<'de> {
    fn child(&self, entry: &'de Entry) -> Lenient<'de> {
        Lenient {
            node: &entry.node,
            key: join(&self.key, &entry.spelling),
        }
    }

    fn any<V: Visitor<'de>>(&self, visitor: V) -> Result<V::Value, Error> {
        match &self.node.value {
            NodeValue::String(s) => visitor.visit_borrowed_str(s),
            NodeValue::Integer(i) => visitor.visit_i64(*i),
            NodeValue::Float(f) => visitor.visit_f64(*f),
            NodeValue::Boolean(b) => visitor.visit_bool(*b),
            NodeValue::Array(items) => visitor.visit_seq(Seq {
                items: items.iter(),
                index: 0,
                key: &self.key,
            }),
            NodeValue::Table(table) => visitor.visit_map(Map {
                parent: self,
                entries: table.iter().collect::<Vec<_>>().into_iter(),
                pending: None,
                fields: None,
            }),
            NodeValue::Datetime(d) => toml::Value::Datetime(*d)
                .deserialize_any(visitor)
                .map_err(|_: toml::de::Error| Error::invalid("invalid datetime")),
        }
    }

    fn at(&self, e: Error) -> Error {
        e.at(&self.key, &self.node.origin)
    }
}

macro_rules! coerce {
    ($method:ident, $visit:ident, $ty:ty, $name:literal) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            let result = match &self.node.value {
                NodeValue::String(s) => match s.parse::<$ty>() {
                    Ok(n) => visitor.$visit(n),
                    Err(_) => Err(Error::invalid(concat!("expected ", $name))),
                },
                _ => self.any(visitor),
            };
            result.map_err(|e| self.at(e))
        }
    };
}

impl<'de> Deserializer<'de> for Lenient<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.any(visitor).map_err(|e| self.at(e))
    }

    coerce!(deserialize_bool, visit_bool, bool, "boolean");
    coerce!(deserialize_i8, visit_i64, i64, "integer");
    coerce!(deserialize_i16, visit_i64, i64, "integer");
    coerce!(deserialize_i32, visit_i64, i64, "integer");
    coerce!(deserialize_i64, visit_i64, i64, "integer");
    coerce!(deserialize_u8, visit_u64, u64, "unsigned integer");
    coerce!(deserialize_u16, visit_u64, u64, "unsigned integer");
    coerce!(deserialize_u32, visit_u64, u64, "unsigned integer");
    coerce!(deserialize_u64, visit_u64, u64, "unsigned integer");
    coerce!(deserialize_f32, visit_f64, f64, "float");
    coerce!(deserialize_f64, visit_f64, f64, "float");

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let (key, origin) = (self.key.clone(), self.node.origin.clone());
        visitor.visit_some(self).map_err(|e| e.at(&key, &origin))
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        let (key, origin) = (self.key.clone(), self.node.origin.clone());
        visitor
            .visit_newtype_struct(self)
            .map_err(|e| e.at(&key, &origin))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        if is_duration(name, fields) {
            match &self.node.value {
                NodeValue::String(s) => {
                    return duration_visit(parse_duration(s), visitor).map_err(|e| self.at(e));
                }
                NodeValue::Integer(i) => {
                    let secs = u64::try_from(*i).map_err(|_| "invalid duration");
                    return duration_visit(secs.map(Duration::from_secs), visitor)
                        .map_err(|e| self.at(e));
                }
                _ => {}
            }
        }
        let table = match &self.node.value {
            NodeValue::Table(table) => table,
            // `toml::value::Datetime` asks for its private struct form.
            NodeValue::Datetime(d) => {
                return toml::Value::Datetime(*d)
                    .deserialize_struct(name, fields, visitor)
                    .map_err(|_: toml::de::Error| self.at(Error::invalid("invalid datetime")));
            }
            _ => return self.any(visitor).map_err(|e| self.at(e)),
        };
        if let Some((a, b)) = colliding(fields) {
            return Err(self.at(Error::invalid(format!(
                "fields `{a}` and `{b}` have the same canonical key"
            ))));
        }
        visitor
            .visit_map(Map {
                parent: &self,
                entries: table.iter().collect::<Vec<_>>().into_iter(),
                pending: None,
                fields: Some(fields),
            })
            .map_err(|e| self.at(e))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let result = match &self.node.value {
            NodeValue::String(s) => {
                visitor.visit_enum(resolve(s.as_str(), variants).into_deserializer())
            }
            NodeValue::Table(table) if table.iter().count() == 1 => {
                let entry = table.iter().next().expect("one entry");
                visitor.visit_enum(Enum {
                    variant: resolve(&entry.spelling, variants),
                    value: self.child(entry),
                })
            }
            _ => Err(Error::invalid("invalid enum")),
        };
        result.map_err(|e| self.at(e))
    }

    /// A string leaf is a comma-separated list; any other node binds as an array.
    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let NodeValue::String(s) = &self.node.value else {
            return self.any(visitor).map_err(|e| self.at(e));
        };
        visitor
            .visit_seq(Items {
                items: split_list(s).into_iter(),
                index: 0,
                key: &self.key,
                origin: &self.node.origin,
            })
            .map_err(|e| self.at(e))
    }

    serde::forward_to_deserialize_any! {
        i128 u128 char str string bytes byte_buf unit unit_struct tuple tuple_struct map
        identifier ignored_any
    }
}

/// Whether serde asks for `std::time::Duration` (its impl calls
/// `deserialize_struct("Duration", &["secs", "nanos"], ..)`; a test pins that).
fn is_duration(name: &str, fields: &[&str]) -> bool {
    name == "Duration" && fields == ["secs", "nanos"]
}

/// Hands a parsed duration to serde's `Duration` visitor as a `{ secs, nanos }` map.
fn duration_visit<'de, V: Visitor<'de>>(
    parsed: Result<Duration, &'static str>,
    visitor: V,
) -> Result<V::Value, Error> {
    let d = parsed.map_err(Error::invalid)?;
    visitor.visit_map(MapDeserializer::new(
        [
            ("secs", d.as_secs()),
            ("nanos", u64::from(d.subsec_nanos())),
        ]
        .into_iter(),
    ))
}

/// Splits a comma-separated list: items are trimmed, `\,` is a literal comma (a backslash is
/// special only before a comma) and an empty string is an empty list.
fn split_list(s: &str) -> Vec<String> {
    if s.trim().is_empty() {
        return Vec::new();
    }
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&',') => {
                cur.push(',');
                chars.next();
            }
            ',' => items.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    items.push(cur);
    items.into_iter().map(|i| i.trim().to_string()).collect()
}

/// The items of a comma-separated string, each bound like a scalar leaf.
struct Items<'a> {
    items: std::vec::IntoIter<String>,
    index: usize,
    key: &'a str,
    origin: &'a Origin,
}

impl<'de> de::SeqAccess<'de> for Items<'_> {
    type Error = Error;

    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        let Some(text) = self.items.next() else {
            return Ok(None);
        };
        let key = format!("{}[{}]", self.key, self.index);
        self.index += 1;
        seed.deserialize(Item { text })
            .map(Some)
            .map_err(|e| e.at(&key, self.origin))
    }
}

/// One list item: a string that coerces to the number or bool the target expects.
struct Item {
    text: String,
}

macro_rules! coerce_item {
    ($method:ident, $visit:ident, $ty:ty, $name:literal) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match self.text.parse::<$ty>() {
                Ok(n) => visitor.$visit(n),
                Err(_) => Err(Error::invalid(concat!("expected ", $name))),
            }
        }
    };
}

impl<'de> Deserializer<'de> for Item {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_string(self.text)
    }

    coerce_item!(deserialize_bool, visit_bool, bool, "boolean");
    coerce_item!(deserialize_i8, visit_i64, i64, "integer");
    coerce_item!(deserialize_i16, visit_i64, i64, "integer");
    coerce_item!(deserialize_i32, visit_i64, i64, "integer");
    coerce_item!(deserialize_i64, visit_i64, i64, "integer");
    coerce_item!(deserialize_u8, visit_u64, u64, "unsigned integer");
    coerce_item!(deserialize_u16, visit_u64, u64, "unsigned integer");
    coerce_item!(deserialize_u32, visit_u64, u64, "unsigned integer");
    coerce_item!(deserialize_u64, visit_u64, u64, "unsigned integer");
    coerce_item!(deserialize_f32, visit_f64, f64, "float");
    coerce_item!(deserialize_f64, visit_f64, f64, "float");

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        if is_duration(name, fields) {
            return duration_visit(parse_duration(&self.text), visitor);
        }
        self.deserialize_any(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_enum(resolve(&self.text, variants).into_deserializer())
    }

    serde::forward_to_deserialize_any! {
        i128 u128 char str string bytes byte_buf unit unit_struct seq tuple tuple_struct map
        identifier ignored_any
    }
}

struct Seq<'a, 'de> {
    items: std::slice::Iter<'de, Node>,
    index: usize,
    key: &'a str,
}

impl<'de> de::SeqAccess<'de> for Seq<'_, 'de> {
    type Error = Error;

    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        let Some(node) = self.items.next() else {
            return Ok(None);
        };
        let key = format!("{}[{}]", self.key, self.index);
        self.index += 1;
        let tag = key.clone();
        seed.deserialize(Lenient { node, key })
            .map(Some)
            .map_err(|e| e.at(&tag, &node.origin))
    }
}

/// Entries of a table. With `fields` (a struct), keys resolve to the declared field names.
struct Map<'a, 'de> {
    parent: &'a Lenient<'de>,
    entries: std::vec::IntoIter<&'de Entry>,
    pending: Option<&'de Entry>,
    fields: Option<&'static [&'static str]>,
}

impl<'de> de::MapAccess<'de> for Map<'_, 'de> {
    type Error = Error;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        let Some(entry) = self.entries.next() else {
            return Ok(None);
        };
        self.pending = Some(entry);
        let written = entry.spelling.as_str();
        let name = match self.fields {
            Some(fields) => resolve(written, fields),
            None => written,
        };
        // An unknown field is reported at the table's key, from the entry's origin.
        seed.deserialize(name.into_deserializer())
            .map(Some)
            .map_err(|e: Error| e.with_origin(&entry.node.origin))
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let entry = self
            .pending
            .take()
            .ok_or_else(|| Error::invalid("value requested before key"))?;
        let child = self.parent.child(entry);
        let (key, origin) = (child.key.clone(), &entry.node.origin);
        seed.deserialize(child).map_err(|e| e.at(&key, origin))
    }
}

/// An enum given as a one-entry table: `{ Variant = value }`.
struct Enum<'a, 'de> {
    variant: &'a str,
    value: Lenient<'de>,
}

impl<'a, 'de> de::EnumAccess<'de> for Enum<'a, 'de> {
    type Error = Error;
    type Variant = Lenient<'de>;

    fn variant_seed<S: de::DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Lenient<'de>), Error> {
        let origin = self.value.node.origin.clone();
        let variant = seed
            .deserialize(self.variant.into_deserializer())
            .map_err(|e: Error| e.with_origin(&origin))?;
        Ok((variant, self.value))
    }
}

impl<'de> de::VariantAccess<'de> for Lenient<'de> {
    type Error = Error;

    /// A unit variant in table form takes an empty table: `{ Variant = {} }`.
    fn unit_variant(self) -> Result<(), Error> {
        match &self.node.value {
            NodeValue::Table(t) if t.iter().next().is_none() => Ok(()),
            _ => Err(self.at(Error::invalid("expected an empty table for a unit variant"))),
        }
    }

    fn newtype_variant_seed<S: de::DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, Error> {
        seed.deserialize(self)
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_struct("", fields, visitor)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Config, ConfigError, Secret};
    use serde::Deserialize;

    fn cfg(s: &str) -> Config {
        Config::from_str(s).unwrap()
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Server {
        port: u16,
        host: String,
    }

    #[test]
    fn string_leaf_coerces_to_u16() {
        let s: Server = cfg("[server]\nport = \"9000\"\nhost = \"h\"")
            .get("server")
            .unwrap();
        assert_eq!(
            s,
            Server {
                port: 9000,
                host: "h".into()
            }
        );
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Mixed {
        a: i64,
        b: f64,
        c: bool,
        d: u64,
        e: i8,
        f: f32,
    }

    #[test]
    fn string_leaf_coerces_to_i64_f64_bool() {
        let m: Mixed = cfg("a = \"-5\"\nb = \"1.5\"\nc = \"true\"\nd = \"18446744073709551615\"\ne = \"-3\"\nf = \"2.5\"")
            .get("")
            .unwrap();
        assert_eq!(
            m,
            Mixed {
                a: -5,
                b: 1.5,
                c: true,
                d: u64::MAX,
                e: -3,
                f: 2.5
            }
        );
    }

    #[test]
    fn native_toml_values_still_bind() {
        let m: Mixed = cfg("a = 1\nb = 2.5\nc = false\nd = 3\ne = 4\nf = 0.5")
            .get("")
            .unwrap();
        assert_eq!(
            m,
            Mixed {
                a: 1,
                b: 2.5,
                c: false,
                d: 3,
                e: 4,
                f: 0.5
            }
        );
    }

    #[test]
    fn bad_number_is_invalid_naming_key_not_value() {
        let e = cfg("[server]\nport = \"abc\"\nhost = \"h\"")
            .get::<Server>("server")
            .unwrap_err();
        let text = format!("{e} {e:?}");
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "server.port"),
            "{e:?}"
        );
        assert!(!text.contains("abc"), "{text}");
    }

    #[test]
    fn out_of_range_number_is_invalid_without_value() {
        let e = cfg("[server]\nport = 70000\nhost = \"h\"")
            .get::<Server>("server")
            .unwrap_err();
        let text = format!("{e} {e:?}");
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "server.port"),
            "{e:?}"
        );
        assert!(!text.contains("70000"), "{text}");
    }

    #[test]
    fn string_field_never_coerced() {
        #[derive(Debug, Deserialize)]
        struct S {
            host: String,
        }
        let ok: S = cfg("host = \"9000\"").get("").unwrap();
        assert_eq!(ok.host, "9000");
        let e = cfg("host = 9000").get::<S>("").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "host"),
            "{e:?}"
        );
    }

    #[test]
    fn nested_struct_coerces() {
        #[derive(Deserialize)]
        struct Outer {
            server: Server,
        }
        let o: Outer = cfg("[server]\nport = \"1\"\nhost = \"h\"").get("").unwrap();
        assert_eq!(o.server.port, 1);
    }

    #[test]
    fn option_field_absent_is_none_and_present_coerces() {
        #[derive(Deserialize)]
        struct S {
            a: Option<u16>,
            b: Option<u16>,
        }
        let s: S = cfg("b = \"7\"").get("").unwrap();
        assert_eq!((s.a, s.b), (None, Some(7)));
    }

    #[test]
    fn vec_fields_bind_from_arrays() {
        #[derive(Deserialize)]
        struct S {
            names: Vec<String>,
            ports: Vec<u16>,
        }
        let s: S = cfg("names = [\"a\", \"b\"]\nports = [1, 2]")
            .get("")
            .unwrap();
        assert_eq!(
            (s.names, s.ports),
            (vec!["a".into(), "b".into()], vec![1, 2])
        );
    }

    #[test]
    fn missing_required_key_is_missing_naming_key() {
        let e = cfg("[server]\nhost = \"h\"")
            .get::<Server>("server")
            .unwrap_err();
        assert!(
            matches!(&e, ConfigError::Missing { key } if key == "server.port"),
            "{e:?}"
        );
    }

    #[test]
    fn dotted_prefix_binds_subtree() {
        let s: Server = cfg("[a.b]\nport = 1\nhost = \"h\"").get("a.b").unwrap();
        assert_eq!(s.port, 1);
    }

    #[test]
    fn empty_prefix_binds_whole_config() {
        let s: Server = cfg("port = 1\nhost = \"h\"").get("").unwrap();
        assert_eq!(s.port, 1);
    }

    #[test]
    fn missing_prefix_is_missing() {
        let e = cfg("a = 1").get::<Server>("server").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Missing { key } if key == "server"),
            "{e:?}"
        );
        let e = cfg("a = 1").get::<Server>("a.b").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Missing { key } if key == "a.b"),
            "{e:?}"
        );
    }

    #[test]
    fn env_string_over_table_node_is_invalid_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("application.toml"),
            "[server]\nport = 1\nhost = \"h\"",
        )
        .unwrap();
        let c = Config::builder()
            .dir(dir.path())
            .env_from([("APP_SERVER".to_string(), "x".to_string())])
            .build()
            .unwrap();
        let e = c.get::<Server>("server").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "server"),
            "{e:?}"
        );
    }

    #[test]
    fn secret_field_binds_and_stays_redacted() {
        #[derive(Debug, Deserialize)]
        struct S {
            token: Secret<String>,
        }
        let s: S = cfg("token = \"hunter2\"").get("").unwrap();
        assert_eq!(s.token.expose(), "hunter2");
        assert!(!format!("{s:?}").contains("hunter2"));
    }

    #[test]
    fn mistyped_secret_value_not_in_error() {
        #[derive(Debug, Deserialize)]
        struct S {
            #[allow(dead_code)]
            token: Secret<u16>,
        }
        let e = cfg("token = \"hunter2\"").get::<S>("").unwrap_err();
        let text = format!("{e} {e:?}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    #[test]
    fn unknown_enum_variant_error_omits_value() {
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum Mode {
            Fast,
        }
        #[derive(Debug, Deserialize)]
        struct S {
            #[allow(dead_code)]
            mode: Mode,
        }
        assert!(cfg("mode = \"fast\"").get::<S>("").is_ok());
        let e = cfg("mode = \"supersecret\"").get::<S>("").unwrap_err();
        assert!(!format!("{e} {e:?}").contains("supersecret"));
    }

    #[test]
    fn custom_deserialize_error_message_omits_value() {
        #[derive(Debug)]
        struct Url;
        impl<'de> Deserialize<'de> for Url {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Err(serde::de::Error::custom(format!("bad url {s}")))
            }
        }
        #[derive(Debug, Deserialize)]
        struct S {
            #[allow(dead_code)]
            u: Secret<Url>,
        }
        let e = cfg("u = \"hunter2\"").get::<S>("").unwrap_err();
        let text = format!("{e} {e:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "u"),
            "{e:?}"
        );
    }

    #[test]
    fn unknown_field_error_names_field() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct S {
            #[allow(dead_code)]
            a: u8,
        }
        let e = cfg("a = 1\nzzz = 2").get::<S>("").unwrap_err();
        assert!(format!("{e}").contains("zzz"), "{e}");
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Pool {
        max_connections: u8,
    }

    #[test]
    fn four_spellings_bind_to_one_field() {
        for key in [
            "maxConnections",
            "max_connections",
            "max-connections",
            "MAX_CONNECTIONS",
        ] {
            let p: Pool = cfg(&format!("{key} = 7")).get("").unwrap();
            assert_eq!(p, Pool { max_connections: 7 }, "{key}");
        }
    }

    #[test]
    fn camel_case_key_in_file_binds_snake_case_field() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("application.toml"),
            "[pool]\nmaxConnections = 3\n",
        )
        .unwrap();
        let c = Config::builder()
            .dir(dir.path())
            .env_from([])
            .build()
            .unwrap();
        assert_eq!(c.get::<Pool>("pool").unwrap(), Pool { max_connections: 3 });
    }

    #[test]
    fn unknown_key_is_emitted_as_written_for_deny_unknown_fields() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct S {
            #[allow(dead_code)]
            max_connections: u8,
        }
        assert!(cfg("MaxConnections = 1").get::<S>("").is_ok());
        let e = cfg("[s]\nmaxConnections = 1\nZzz-Top = 2")
            .get::<S>("s")
            .unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "s"),
            "{e:?}"
        );
        assert!(e.to_string().contains("`Zzz-Top`"), "{e}");
    }

    #[test]
    fn rename_and_alias_still_match_canonically() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct S {
            #[serde(rename = "jwk-set-uri", alias = "jwk_set_uri")]
            uri: String,
            #[serde(rename = "url", alias = "link")]
            u: String,
        }
        for (a, b) in [
            ("jwk-set-uri", "url"),
            ("jwk_set_uri", "link"),
            ("jwkSetUri", "URL"),
            ("JWK_SET_URI", "Link"),
        ] {
            let s: S = cfg(&format!("{a} = \"x\"\n{b} = \"y\""))
                .get("")
                .unwrap_or_else(|e| panic!("{a} {b}: {e}"));
            assert_eq!((s.uri.as_str(), s.u.as_str()), ("x", "y"), "{a} {b}");
        }
    }

    #[test]
    fn two_fields_with_one_canonical_form_is_invalid() {
        #[derive(Debug, Deserialize)]
        struct S {
            #[allow(dead_code)]
            maxconn: Option<u8>,
            #[allow(dead_code)]
            max_conn: Option<u8>,
        }
        let e = cfg("[s]\nmaxConn = 1").get::<S>("s").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "s"),
            "{e:?}"
        );
        let text = e.to_string();
        assert!(
            text.contains("maxconn") && text.contains("max_conn"),
            "{text}"
        );
    }

    #[test]
    fn map_keys_keep_winning_spelling() {
        use std::collections::BTreeMap;
        let m: BTreeMap<String, u8> = cfg("[m]\nFoo-Bar = 1\nbaz = 2").get("m").unwrap();
        assert_eq!(
            m.into_iter().collect::<Vec<_>>(),
            vec![("Foo-Bar".to_string(), 1), ("baz".to_string(), 2)]
        );

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("application.toml"),
            "[m]\nFoo-Bar = 1\nbaz = 2\n",
        )
        .unwrap();
        let c = Config::builder()
            .dir(dir.path())
            .env_from([("APP_M__FOO_BAR".to_string(), "3".to_string())])
            .build()
            .unwrap();
        let m: BTreeMap<String, u8> = c.get("m").unwrap();
        assert_eq!(
            m.into_iter().collect::<Vec<_>>(),
            vec![("baz".to_string(), 2), ("foo_bar".to_string(), 3)]
        );
    }

    #[test]
    fn enum_variant_matches_case_insensitively() {
        #[derive(Debug, Deserialize, PartialEq)]
        #[serde(rename_all = "snake_case")]
        enum Mode {
            FastPath,
            Slow,
        }
        #[derive(Debug, Deserialize)]
        struct S {
            mode: Mode,
        }
        for (text, want) in [
            ("fast_path", Mode::FastPath),
            ("FAST_PATH", Mode::FastPath),
            ("fastPath", Mode::FastPath),
            ("fast-path", Mode::FastPath),
            ("Slow", Mode::Slow),
        ] {
            let s: S = cfg(&format!("mode = \"{text}\""))
                .get("")
                .unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(s.mode, want, "{text}");
        }
    }

    #[test]
    fn enum_table_form_binds_newtype_struct_and_unit_variants() {
        #[derive(Debug, Deserialize, PartialEq)]
        enum Limit {
            Off,
            Fixed(u16),
            Range { lo: u16, hi: u16 },
        }
        #[derive(Debug, Deserialize)]
        struct S {
            limit: Limit,
        }
        let get = |text: &str| cfg(text).get::<S>("").map(|s| s.limit);
        assert_eq!(get("limit = \"off\"").unwrap(), Limit::Off);
        assert_eq!(get("limit = { Off = {} }").unwrap(), Limit::Off);
        assert_eq!(get("limit = { fixed = \"5\" }").unwrap(), Limit::Fixed(5));
        assert_eq!(
            get("[limit.RANGE]\nlo = 1\nHi = \"2\"").unwrap(),
            Limit::Range { lo: 1, hi: 2 }
        );
        let e = get("limit = { Fixed = \"x-secret\" }").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "limit.Fixed"),
            "{e:?}"
        );
        assert!(!e.to_string().contains("x-secret"), "{e}");
        for bad in ["limit = 3", "limit = { Fixed = 1, Off = {} }"] {
            let e = get(bad).unwrap_err();
            assert!(
                matches!(&e, ConfigError::Invalid { key, .. } if key == "limit"),
                "{bad}: {e:?}"
            );
        }
    }

    #[test]
    fn datetime_and_whole_tree_still_bind() {
        #[derive(Debug, Deserialize)]
        struct S {
            when: toml::value::Datetime,
        }
        let s: S = cfg("when = 1979-05-27T07:32:00Z").get("").unwrap();
        assert_eq!(s.when.to_string(), "1979-05-27T07:32:00Z");
        // A string field gets the datetime's text, as before.
        let text: String = cfg("when = 1979-05-27").get("when").unwrap();
        assert_eq!(text, "1979-05-27");
        let whole: toml::Value = cfg("Top = 1\n[a]\nb = [true, 1.5]").get("").unwrap();
        assert_eq!(
            whole,
            toml::from_str::<toml::Value>("Top = 1\n[a]\nb = [true, 1.5]").unwrap()
        );
    }

    #[test]
    fn unknown_field_error_carries_origin_of_the_entry() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct S {
            #[allow(dead_code)]
            a: u8,
        }
        let e = cfg("[s]\na = 1\n\nzzz = 2").get::<S>("s").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, origin: Some(crate::Origin::Inline { line: 4 }), .. } if key == "s"),
            "{e:?}"
        );
    }

    #[test]
    fn ascending_declared_canonical_twins_are_not_detected_and_exact_match_wins() {
        // FIELDS = ["jwt_secret", "jwtsecret"] is ascending, so it could be one field's
        // rename/alias run: no collision error. An exact name wins; a relaxed match takes the
        // first declared name.
        #[derive(Debug, Deserialize, PartialEq)]
        struct S {
            jwt_secret: Option<String>,
            jwtsecret: Option<String>,
        }
        let bind = |text: &str| cfg(text).get::<S>("").unwrap();
        let first = S {
            jwt_secret: Some("A".into()),
            jwtsecret: None,
        };
        let second = S {
            jwt_secret: None,
            jwtsecret: Some("A".into()),
        };
        assert_eq!(bind("jwtsecret = \"A\""), second);
        assert_eq!(bind("jwt_secret = \"A\""), first);
        assert_eq!(bind("jwtSecret = \"A\""), first);
        assert_eq!(bind("JWTSECRET = \"A\""), first);
    }

    #[test]
    fn rename_plus_alias_of_one_field_is_not_a_collision() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct S {
            #[serde(rename = "jwk-set-uri", alias = "jwk_set_uri")]
            jwk_set_uri: String,
        }
        for key in ["jwk-set-uri", "jwk_set_uri", "jwkSetUri", "JWK_SET_URI"] {
            let s: S = cfg(&format!("{key} = \"u\""))
                .get("")
                .unwrap_or_else(|e| panic!("{key}: {e}"));
            assert_eq!(s.jwk_set_uri, "u", "{key}");
        }
    }

    #[test]
    fn flatten_struct_fields_match_exactly_only() {
        // `#[serde(flatten)]` binds through a map, so the inner fields see keys as written.
        #[derive(Debug, Deserialize)]
        struct Inner {
            max_connections: u8,
        }
        #[derive(Debug, Deserialize)]
        struct Outer {
            #[serde(flatten)]
            inner: Inner,
        }
        let ok: Outer = cfg("max_connections = 2").get("").unwrap();
        assert_eq!(ok.inner.max_connections, 2);
        let e = cfg("maxConnections = 2").get::<Outer>("").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Missing { key } if key == "max_connections"),
            "{e:?}"
        );
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Ports {
        ports: Vec<u16>,
    }

    #[test]
    fn comma_list_binds_to_vec_u16() {
        let p: Ports = cfg("ports = \"80,443, 8080\"").get("").unwrap();
        assert_eq!(p.ports, vec![80, 443, 8080]);
        let native: Ports = cfg("ports = [1, 2]").get("").unwrap();
        assert_eq!(native.ports, vec![1, 2]);
        let indexed: Ports = Config::builder()
            .env_from([
                ("APP_PORTS__0".to_string(), "5".to_string()),
                ("APP_PORTS__1".to_string(), "6".to_string()),
            ])
            .build()
            .unwrap()
            .get("")
            .unwrap();
        assert_eq!(indexed.ports, vec![5, 6]);
    }

    #[test]
    fn comma_list_trims_and_escapes_comma() {
        #[derive(Deserialize)]
        struct S {
            names: Vec<String>,
        }
        let s: S = cfg("names = \" a , b\\\\,c ,d\\\\e \"").get("").unwrap();
        assert_eq!(s.names, vec!["a", "b,c", "d\\e"]);
    }

    #[test]
    fn empty_string_is_empty_list() {
        let p: Ports = cfg("ports = \"\"").get("").unwrap();
        assert!(p.ports.is_empty());
        let p: Ports = cfg("ports = \"  \"").get("").unwrap();
        assert!(p.ports.is_empty());
    }

    #[test]
    fn comma_string_bound_to_string_field_is_not_split() {
        #[derive(Deserialize)]
        struct S {
            host: String,
        }
        let s: S = cfg("host = \"a,b, c\"").get("").unwrap();
        assert_eq!(s.host, "a,b, c");
    }

    #[test]
    fn comma_list_bad_element_names_index() {
        let e = cfg("ports = \"80,secret-x,90\"")
            .get::<Ports>("")
            .unwrap_err();
        let text = format!("{e} {e:?}");
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "ports[1]"),
            "{e:?}"
        );
        assert!(!text.contains("secret-x"), "{text}");
    }

    #[test]
    fn duration_string_forms_bind() {
        use std::time::Duration;
        #[derive(Deserialize)]
        struct S {
            a: Duration,
            b: Duration,
            c: Option<Duration>,
            d: Vec<Duration>,
        }
        let s: S = cfg("a = \"1.5s\"\nb = 90\nc = \"2m\"\nd = \"1s, 500ms\"")
            .get("")
            .unwrap();
        assert_eq!(s.a, Duration::from_millis(1500));
        assert_eq!(s.b, Duration::from_secs(90));
        assert_eq!(s.c, Some(Duration::from_secs(120)));
        assert_eq!(
            s.d,
            vec![Duration::from_secs(1), Duration::from_millis(500)]
        );
    }

    #[test]
    fn duration_native_table_still_works() {
        use std::time::Duration;
        #[derive(Deserialize)]
        struct S {
            t: Duration,
        }
        let s: S = cfg("t = { secs = 3, nanos = 5 }").get("").unwrap();
        assert_eq!(s.t, Duration::new(3, 5));
    }

    #[test]
    fn duration_invalid_names_key_without_value() {
        use std::time::Duration;
        #[derive(Debug, Deserialize)]
        struct S {
            #[allow(dead_code)]
            t: Duration,
        }
        let e = cfg("[x]\nt = \"7parsecs\"").get::<S>("x").unwrap_err();
        let text = format!("{e} {e:?}");
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "x.t"),
            "{e:?}"
        );
        assert!(!text.contains("parsecs"), "{text}");
    }

    #[test]
    fn duration_inside_struct_with_relaxed_keys() {
        use crate::DataSize;
        use std::time::Duration;
        #[derive(Deserialize)]
        struct Pool {
            connect_timeout: Duration,
            max_body: DataSize,
        }
        let p: Pool = cfg("[pool]\nconnect-timeout = \"250ms\"\nMAX_BODY = \"2MiB\"")
            .get("pool")
            .unwrap();
        assert_eq!(p.connect_timeout, Duration::from_millis(250));
        assert_eq!(p.max_body.bytes(), 2 * 1024 * 1024);
    }
}
