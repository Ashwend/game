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
    #[allow(dead_code)]
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
