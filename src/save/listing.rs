use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use uuid::Uuid;

use crate::world::MapType;

use super::format::{SAVE_EXTENSION, SAVE_MAGIC};
use super::types::WorldSave;

/// Result of a `list_worlds()` call. Loadable saves are returned in `worlds`;
/// any save files that failed to parse (truncated, wrong magic, bad format
/// version, corrupted zstd payload, …) are surfaced separately in
/// `corrupted` so the player can see what's broken instead of being shown
/// an empty list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldListing {
    pub worlds: Vec<WorldSummary>,
    pub corrupted: Vec<CorruptedWorld>,
}

/// A save file that was present on disk but could not be loaded. The file
/// name is preserved (rather than the parsed UUID) because the parse is
/// exactly what failed, there's no save struct to extract an id from.
///
/// `id` is recovered from the file name (`{uuid}.save`) when possible so the
/// UI can still wire up a Delete action against the same `WorldStore::delete_world`
/// path it uses for healthy worlds. `recovered_name` is a best-effort decode
/// of the save's name field; it lets the worlds list show something
/// human-readable instead of a raw file name when the failure is something
/// other than a postcard layout change.
#[derive(Debug, Clone, PartialEq)]
pub struct CorruptedWorld {
    pub file_name: String,
    pub id: Option<Uuid>,
    pub recovered_name: Option<String>,
    pub error: String,
}

impl CorruptedWorld {
    /// Display name for the worlds list. Falls back to the file name if the
    /// best-effort recovery turned up nothing (or the recovered name is
    /// empty/control-only, postcard layout drift can produce junk bytes
    /// that decode but aren't human-readable).
    pub fn display_name(&self) -> &str {
        self.recovered_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.file_name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorldSummary {
    pub id: Uuid,
    pub name: String,
    pub map: MapType,
    pub created_at_unix: u64,
    pub path: PathBuf,
}

impl WorldSummary {
    pub(super) fn from_save(save: &WorldSave, path: PathBuf) -> Self {
        Self {
            id: save.id,
            name: save.name.clone(),
            map: save.map.clone(),
            created_at_unix: save.created_at_unix,
            path,
        }
    }
}

/// Minimal prefix of [`WorldSave`] used to recover a name for unloadable
/// saves. Postcard is positional, so as long as the on-disk schema still
/// starts with `id` then `name` (which it has for every shipped format
/// version), this can deserialize the first two fields even when the full
/// `WorldSave` decode fails because of a later field change or a version
/// mismatch. Anything after `name` is left in the trailing bytes and
/// ignored.
#[derive(Debug, Clone, Deserialize)]
struct WorldSaveNamePrefix {
    #[expect(
        dead_code,
        reason = "decoded positionally to reach `name`; the id field is not read here"
    )]
    id: Uuid,
    name: String,
}

pub(super) fn uuid_from_save_file_name(file_name: &str) -> Option<Uuid> {
    let stem = file_name.strip_suffix(&format!(".{SAVE_EXTENSION}"))?;
    Uuid::parse_str(stem).ok()
}

pub(super) fn read_world_name_best_effort(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    decode_world_name_best_effort(&bytes)
}

/// Try to recover just the world's display name from a save file, ignoring
/// version mismatches and trailing payload errors. Returns `None` if even
/// the header/compression layer can't be peeled back, or if the recovered
/// name is empty / nothing but control characters (which happens when the
/// postcard layout itself has drifted and the decode is reading garbage
/// bytes as a string length + payload).
fn decode_world_name_best_effort(bytes: &[u8]) -> Option<String> {
    if bytes.len() < SAVE_MAGIC.len() + 4 {
        return None;
    }
    if &bytes[..SAVE_MAGIC.len()] != SAVE_MAGIC {
        return None;
    }
    let compressed = &bytes[SAVE_MAGIC.len() + 4..];
    let payload = zstd::stream::decode_all(compressed).ok()?;
    let (prefix, _rest) = postcard::take_from_bytes::<WorldSaveNamePrefix>(&payload).ok()?;
    let name = prefix.name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::super::format::{SAVE_FORMAT_VERSION, decode_world_save, encode_world_save};
    use super::*;

    /// Canonical migration scenario: a save written by a newer build the
    /// loader rejects on version, but whose name we can still surface in the
    /// "couldn't load" banner because the postcard prefix (`id` then `name`)
    /// is unchanged.
    #[test]
    fn recovers_name_from_a_future_version_save() {
        let save = WorldSave::new("Future World", Some(1));
        let mut bytes = encode_world_save(&save).expect("encode");
        // Bump the format version byte to a value the loader does not accept.
        let future_version = SAVE_FORMAT_VERSION + 1;
        bytes[SAVE_MAGIC.len()..SAVE_MAGIC.len() + 4]
            .copy_from_slice(&future_version.to_le_bytes());

        // Strict decode refuses it (version gate)...
        assert!(decode_world_save(&bytes).is_err());
        // ...but the best-effort name recovery still reads the original name.
        assert_eq!(
            decode_world_name_best_effort(&bytes).as_deref(),
            Some("Future World")
        );
    }

    #[test]
    fn rejects_all_control_or_whitespace_names() {
        // A name that is nothing but control characters does not survive the
        // best-effort filter (postcard layout drift produces junk like this).
        let save = WorldSave::new("\u{7}\u{1}\u{2}", Some(1));
        let bytes = encode_world_save(&save).expect("encode");
        assert_eq!(decode_world_name_best_effort(&bytes), None);
    }

    #[test]
    fn rejects_truncated_and_wrong_magic_bytes() {
        // Shorter than magic + version field: nothing to peel back.
        assert_eq!(decode_world_name_best_effort(b"short"), None);
        // Right length but the magic header is wrong.
        let mut wrong_magic = Vec::new();
        wrong_magic.extend_from_slice(b"NOTGAME!");
        wrong_magic.extend_from_slice(&SAVE_FORMAT_VERSION.to_le_bytes());
        wrong_magic.extend_from_slice(b"payload bytes that never get read");
        assert_eq!(decode_world_name_best_effort(&wrong_magic), None);
    }

    #[test]
    fn uuid_from_save_file_name_parses_uuid_stem_only() {
        let id = Uuid::new_v4();
        let file_name = format!("{id}.save");
        assert_eq!(uuid_from_save_file_name(&file_name), Some(id));
        // A non-uuid stem is rejected even with the right extension.
        assert_eq!(uuid_from_save_file_name("not-a-uuid.save"), None);
        // Missing the `.save` extension is rejected too.
        assert_eq!(uuid_from_save_file_name(&id.to_string()), None);
    }

    #[test]
    fn display_name_falls_back_to_file_name() {
        // Recovered name present and non-blank: used directly.
        let with_name = CorruptedWorld {
            file_name: "abc.save".to_owned(),
            id: None,
            recovered_name: Some("My World".to_owned()),
            error: "boom".to_owned(),
        };
        assert_eq!(with_name.display_name(), "My World");

        // Recovered name is `None`: fall back to the file name.
        let no_name = CorruptedWorld {
            file_name: "abc.save".to_owned(),
            id: None,
            recovered_name: None,
            error: "boom".to_owned(),
        };
        assert_eq!(no_name.display_name(), "abc.save");

        // Recovered name is blank: also falls back to the file name.
        let blank_name = CorruptedWorld {
            file_name: "abc.save".to_owned(),
            id: None,
            recovered_name: Some("   ".to_owned()),
            error: "boom".to_owned(),
        };
        assert_eq!(blank_name.display_name(), "abc.save");
    }
}
