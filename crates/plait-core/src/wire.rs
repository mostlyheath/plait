//! Serde helpers for values that cross to a remote desktop.
//!
//! Paths are sent as base64 of their raw bytes: a checkout on Linux may hold
//! names that are not UTF-8, and JSON strings would lose or refuse them.

/// A path as base64 of its bytes.
pub mod path {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

    pub fn serialize<S: Serializer>(path: &std::path::Path, s: S) -> Result<S::Ok, S::Error> {
        use std::os::unix::ffi::OsStrExt;
        s.serialize_str(&STANDARD.encode(path.as_os_str().as_bytes()))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = STANDARD.decode(text).map_err(D::Error::custom)?;
        Ok(PathBuf::from(OsString::from_vec(bytes)))
    }
}

/// An optional path as base64 of its bytes, or null.
pub mod opt_path {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::path::PathBuf;

    #[derive(serde::Serialize, Deserialize)]
    struct Wrapped(#[serde(with = "super::path")] PathBuf);

    pub fn serialize<S: Serializer>(path: &Option<PathBuf>, s: S) -> Result<S::Ok, S::Error> {
        match path {
            Some(path) => s.serialize_some(&Wrapped(path.clone())),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<PathBuf>, D::Error> {
        Ok(Option::<Wrapped>::deserialize(d)?.map(|w| w.0))
    }
}

/// A list of paths, each as base64 of its bytes.
pub mod paths {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::path::PathBuf;

    #[derive(Serialize, Deserialize)]
    struct Wrapped(#[serde(with = "super::path")] PathBuf);

    pub fn serialize<S: Serializer>(paths: &[PathBuf], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(paths.iter().map(|p| Wrapped(p.clone())))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<PathBuf>, D::Error> {
        Ok(Vec::<Wrapped>::deserialize(d)?
            .into_iter()
            .map(|w| w.0)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Holder {
        #[serde(with = "super::path")]
        one: PathBuf,
        #[serde(with = "super::opt_path")]
        maybe: Option<PathBuf>,
        #[serde(with = "super::paths")]
        many: Vec<PathBuf>,
    }

    #[test]
    fn non_utf8_paths_survive_the_round_trip() {
        let odd = PathBuf::from(OsString::from_vec(b"caf\xe9/n\xffme".to_vec()));
        let value = Holder {
            one: odd.clone(),
            maybe: Some(odd.clone()),
            many: vec![odd, PathBuf::from("plain.rs")],
        };
        let text = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<Holder>(&text).unwrap(), value);
        let none = Holder {
            maybe: None,
            ..value
        };
        let text = serde_json::to_string(&none).unwrap();
        assert!(text.contains("\"maybe\":null"));
        assert_eq!(serde_json::from_str::<Holder>(&text).unwrap(), none);
    }
}
