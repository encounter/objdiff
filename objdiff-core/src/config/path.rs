// For argp::FromArgs
#[cfg(feature = "std")]
pub fn platform_path(value: &str) -> Result<typed_path::Utf8PlatformPathBuf, String> {
    Ok(typed_path::Utf8PlatformPathBuf::from(value))
}

/// Checks if the path is valid UTF-8 and returns it as a [`Utf8PlatformPath`].
#[cfg(feature = "std")]
pub fn check_path(
    path: &std::path::Path,
) -> Result<&typed_path::Utf8PlatformPath, core::str::Utf8Error> {
    typed_path::Utf8PlatformPath::from_bytes_path(typed_path::PlatformPath::new(
        path.as_os_str().as_encoded_bytes(),
    ))
}

/// Checks if the path is valid UTF-8 and returns it as a [`Utf8NativePathBuf`].
#[cfg(feature = "std")]
pub fn check_path_buf(
    path: std::path::PathBuf,
) -> Result<typed_path::Utf8PlatformPathBuf, alloc::string::FromUtf8Error> {
    typed_path::Utf8PlatformPathBuf::from_bytes_path_buf(typed_path::PlatformPathBuf::from(
        path.into_os_string().into_encoded_bytes(),
    ))
}

#[cfg(feature = "serde")]
pub mod unix_path_serde_option {
    use serde::{Deserialize, Deserializer, Serializer};
    use typed_path::Utf8UnixPathBuf;

    pub fn serialize<S>(path: &Option<Utf8UnixPathBuf>, s: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        if let Some(path) = path { s.serialize_some(path.as_str()) } else { s.serialize_none() }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Utf8UnixPathBuf>, D::Error>
    where D: Deserializer<'de> {
        Ok(Option::<String>::deserialize(deserializer)?.map(Utf8UnixPathBuf::from))
    }
}

#[cfg(all(feature = "serde", feature = "std"))]
pub mod platform_path_serde_option {
    use serde::{Deserialize, Deserializer, Serializer};
    use typed_path::Utf8PlatformPathBuf;

    pub fn serialize<S>(path: &Option<Utf8PlatformPathBuf>, s: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        if let Some(path) = path { s.serialize_some(path.as_str()) } else { s.serialize_none() }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Utf8PlatformPathBuf>, D::Error>
    where D: Deserializer<'de> {
        Ok(Option::<String>::deserialize(deserializer)?.map(Utf8PlatformPathBuf::from))
    }
}
