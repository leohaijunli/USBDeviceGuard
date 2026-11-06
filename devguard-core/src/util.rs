//! Validation and formatting helpers shared by every front end.

use std::time::{SystemTime, UNIX_EPOCH};

/// `[a-z0-9-]+`, at most 32 characters.  The same string is the symlink name
/// under `/dev/devguard/`, so it must stay a plain path component.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the name must not be empty".into());
    }
    if name.len() > 32 {
        return Err("the name is limited to 32 characters".into());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(
            "the name may only contain a-z, 0-9 and '-' (no spaces, slashes or capitals)".into(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("the name must not start or end with '-'".into());
    }
    Ok(())
}

/// A four digit hexadecimal USB id, normalized to lower case.
pub fn normalize_hex4(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() != 4 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "`{value}` is not a four digit hexadecimal USB id (e.g. 26ac)"
        ));
    }
    Ok(value.to_ascii_lowercase())
}

/// Mode strings are written as `0660`, as in the configuration schema.
pub fn parse_mode(value: &str) -> Result<u32, String> {
    let value = value.trim();
    let digits = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix('0'))
        .unwrap_or(value);
    if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| (b'0'..=b'7').contains(&b))
    {
        return Err(format!("`{value}` is not an octal permission (e.g. 0660)"));
    }
    let mode = u32::from_str_radix(digits, 8)
        .map_err(|_| format!("`{value}` is not an octal permission (e.g. 0660)"))?;
    if mode & 0o400 == 0 || mode & 0o200 == 0 {
        return Err("the permission must let the owner read and write (at least 0600)".into());
    }
    if mode & 0o002 != 0 {
        return Err("the permission must not be world-writable".into());
    }
    if mode & 0o7777 != mode {
        return Err("only the octal digits 600-777 are allowed".into());
    }
    Ok(mode)
}

pub fn format_mode(mode: u32) -> String {
    format!("{mode:04o}")
}

/// POSIX portable group name, or the sentinel `auto`.
pub fn validate_group(value: &str) -> Result<(), String> {
    if value == "auto" {
        return Ok(());
    }
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(b) if b.is_ascii_alphabetic() || b == b'_' => {}
        _ => return Err("a group name must start with a letter or underscore".into()),
    }
    if value.len() > 32 {
        return Err("a group name is limited to 32 characters".into());
    }
    if !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err("a group name may only contain letters, digits, '_' and '-'".into());
    }
    Ok(())
}

/// User names in journal records: we keep the daemon's lookup result as-is,
/// but never let a newline corrupt a JSONL record.
pub fn clean_user(user: &str) -> String {
    user.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Sort key for configuration names, used for a stable journal and UI order.
pub fn sort_names(names: &mut [String]) {
    names.sort();
}

/// A short, human-readable diff of two TOML documents.  The GUI shows this
/// before writing; the daemon also puts it in the journal.
pub fn toml_diff(before: Option<&str>, after: &str) -> String {
    let old: Vec<&str> = before.unwrap_or("").lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let mut out = String::new();
    // A tiny LCS would hide moved lines; a line-by-line diff is easier to
    // audit and is enough for generated configuration files.
    let max = old.len().max(new.len());
    for i in 0..max {
        match (old.get(i), new.get(i)) {
            (Some(a), Some(b)) if a == b => {
                out.push(' ');
                out.push_str(a);
                out.push('\n');
            }
            (Some(a), Some(b)) => {
                out.push('-');
                out.push_str(a);
                out.push('\n');
                out.push('+');
                out.push_str(b);
                out.push('\n');
            }
            (Some(a), None) => {
                out.push('-');
                out.push_str(a);
                out.push('\n');
            }
            (None, Some(b)) => {
                out.push('+');
                out.push_str(b);
                out.push('\n');
            }
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rules() {
        assert!(validate_name("px4-main").is_ok());
        assert!(validate_name("a1").is_ok());
        assert!(validate_name("PX4").is_err());
        assert!(validate_name("-px4").is_err());
        assert!(validate_name("px4_main").is_err());
        assert!(validate_name("").is_err());
    }

    #[test]
    fn mode_rules() {
        assert_eq!(parse_mode("0660").unwrap(), 0o660);
        assert_eq!(parse_mode("600").unwrap(), 0o600);
        assert!(parse_mode("0666").is_err());
        assert!(parse_mode("0644").is_ok());
        assert!(parse_mode("0444").is_err());
        assert!(parse_mode("0999").is_err());
    }

    #[test]
    fn group_rules() {
        assert!(validate_group("auto").is_ok());
        assert!(validate_group("dialout").is_ok());
        assert!(validate_group("1bad").is_err());
        assert!(validate_group("bad name").is_err());
    }
}
