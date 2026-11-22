use crate::error::Error;
use serde::Serialize;

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Project {
    pub project_id: String,
    pub owner_id: String,
    pub repository: String,
    pub name: String,
    pub default_branch: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_at: i64,
}

/// Repository and project names: non-empty after trimming, at most 512 bytes.
pub fn validate_project_text(value: &str, field: &str) -> Result<(), Error> {
    if value.trim().is_empty() {
        return Err(Error::validation(format!("{field} must not be empty")));
    }
    if value.len() > 512 {
        return Err(Error::validation(format!(
            "{field} exceeds the 512 byte limit"
        )));
    }
    Ok(())
}

/// `cairn init` default name: the last repository segment, without a `local:` prefix.
pub fn default_project_name(repository: &str) -> String {
    let short = repository.rsplit('/').next().unwrap_or(repository);
    short.strip_prefix("local:").unwrap_or(short).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_limits() {
        assert_eq!(default_project_name("github.com/acme/cairn"), "cairn");
        assert_eq!(default_project_name("local:scratch"), "scratch");
        assert!(validate_project_text(" ", "repository").is_err());
        assert!(validate_project_text(&"x".repeat(513), "repository").is_err());
        assert!(validate_project_text("github.com/acme/cairn", "repository").is_ok());
    }
}
