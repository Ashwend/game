/// Maximum number of characters allowed in a player-supplied world name.
/// Saves themselves can hold the historical 64-character form (via
/// `normalize_world_name`), but the UI rejects new inputs above this cap so
/// names stay legible in the worlds list.
pub const MAX_WORLD_NAME_LEN: usize = 48;

/// Validate a player-supplied world name. Returns the canonical (trimmed)
/// form on success, or a human-readable error otherwise.
///
/// The rules are intentionally tighter than `normalize_world_name`'s fallback
/// behaviour: callers that surface validation to the player should reject
/// rather than silently fixing up the input.
pub fn validate_world_name(name: &str) -> Result<&str, &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Name cannot be empty.");
    }
    let char_count = trimmed.chars().count();
    if char_count > MAX_WORLD_NAME_LEN {
        return Err("Name is too long (48 characters max).");
    }
    for ch in trimmed.chars() {
        if ch.is_control() {
            return Err("Name cannot contain control characters.");
        }
        if matches!(ch, '/' | '\\') {
            return Err("Name cannot contain '/' or '\\'.");
        }
    }
    Ok(trimmed)
}

pub(super) fn normalize_world_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "New World".to_owned()
    } else {
        trimmed.chars().take(64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_world_name_accepts_normal_input_and_trims() {
        assert_eq!(
            validate_world_name("  Spruce Valley  "),
            Ok("Spruce Valley")
        );
        assert_eq!(validate_world_name("a"), Ok("a"));
    }

    #[test]
    fn validate_world_name_rejects_empty_and_whitespace_only() {
        assert!(validate_world_name("").is_err());
        assert!(validate_world_name("   \t  ").is_err());
    }

    #[test]
    fn validate_world_name_rejects_overflowing_names() {
        let too_long: String = "a".repeat(MAX_WORLD_NAME_LEN + 1);
        assert!(validate_world_name(&too_long).is_err());
        let at_cap: String = "a".repeat(MAX_WORLD_NAME_LEN);
        assert!(validate_world_name(&at_cap).is_ok());
    }

    #[test]
    fn validate_world_name_rejects_path_separators_and_control_chars() {
        assert!(validate_world_name("nice/name").is_err());
        assert!(validate_world_name("nice\\name").is_err());
        assert!(validate_world_name("nice\nname").is_err());
        assert!(validate_world_name("nice\tname").is_err());
    }
}
