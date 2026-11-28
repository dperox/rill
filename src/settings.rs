use std::env::{self, VarError};
use std::ffi::OsString;

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
