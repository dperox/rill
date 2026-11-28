use std::env::{self, VarError};
use std::{ffi::OsString, fmt, io};

pub(crate) fn var_os(name: &str) -> Option<OsString> {
    env::var_os(name).or_else(|| {
        let suffix = name.strip_prefix("JAPKA_")?;
        env::var_os(format!("RILL_{suffix}"))
    })
}

pub(crate) fn var(name: &str) -> Result<String, VarError> {
    var_os(name)
        .ok_or(VarError::NotPresent)?
        .into_string()
        .map_err(VarError::NotUnicode)
}

#[derive(Debug)]
pub(crate) struct FieldError {
    pub(crate) path: String,
    message: String,
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for FieldError {}

pub(crate) fn field_error(path: String, message: impl fmt::Display) -> io::Error {
    io::Error::other(FieldError {
        path,
        message: message.to_string(),
    })
}

pub(crate) fn context(error: io::Error, label: &str) -> io::Error {
    if let Some(field) = error.get_ref().and_then(|e| e.downcast_ref::<FieldError>()) {
        return field_error(field.path.clone(), format!("{label}: {}", field.message));
    }
    io::Error::new(error.kind(), format!("{label}: {error}"))
}

pub(crate) fn diagnostic(error: &io::Error) -> serde_json::Value {
    let mut diagnostic =
        serde_json::json!({"code":"invalid_configuration", "message":error.to_string()});
    if let Some(field) = error.get_ref().and_then(|e| e.downcast_ref::<FieldError>()) {
        diagnostic["path"] = serde_json::json!(field.path);
    }
    diagnostic
}

pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    let mut reader = serde_json::Deserializer::from_slice(bytes);
    let value = serde_path_to_error::deserialize(&mut reader)
        .map_err(|e| field_error(e.path().to_string(), e.inner()))?;
    reader.end()?;
    Ok(value)
}
