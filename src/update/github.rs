//! GitHub Releases client + the pure "what's newer than me" logic.
//!
//! The repo is public, so the releases API needs no auth, only the
//! `User-Agent` header GitHub requires of every caller. Network I/O lives in
//! [`fetch_releases`]; everything else here is pure and unit-tested.

use std::time::Duration;

use serde::Deserialize;

use crate::protocol::GAME_VERSION;

use super::{asset, version::Version};

const REPO_OWNER: &str = "Ashwend";
const REPO_NAME: &str = "game";

/// Cap the listing; 30 covers a very stale client without paginating. If a
/// player is somehow more than 30 releases behind, the changelog is truncated
/// to the most recent 30 (still strictly correct about *which* is latest).
const RELEASES_PER_PAGE: u32 = 30;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Release {
    pub(crate) tag_name: String,
    #[serde(default)]
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) draft: bool,
    #[serde(default)]
    pub(crate) prerelease: bool,
    #[serde(default)]
    pub(crate) assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReleaseAsset {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) browser_download_url: String,
    /// `"sha256:<hex>"` when GitHub has computed it. Used to verify the
    /// download before we ever hand it to the updater.
    #[serde(default)]
    pub(crate) digest: Option<String>,
    #[serde(default)]
    pub(crate) size: u64,
}

impl Release {
    fn version(&self) -> Option<Version> {
        Version::parse(&self.tag_name)
    }

    fn is_stable(&self) -> bool {
        !self.draft && !self.prerelease && self.version().is_some()
    }

    /// The asset carrying this host's build, if present.
    pub(crate) fn host_asset(&self) -> Option<&ReleaseAsset> {
        if asset::HOST_ASSET_NAME.is_empty() {
            return None;
        }
        self.assets
            .iter()
            .find(|a| a.name == asset::HOST_ASSET_NAME)
    }
}

pub(crate) fn releases_page_url() -> String {
    format!("https://github.com/{REPO_OWNER}/{REPO_NAME}/releases")
}

fn releases_api_url() -> String {
    format!(
        "https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases?per_page={RELEASES_PER_PAGE}"
    )
}

/// Build the shared blocking agent used for the check and the download.
pub(crate) fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()
}

fn user_agent() -> String {
    // GitHub rejects requests without a User-Agent. Identify the build so
    // traffic is attributable in their logs.
    format!("ashwend/{GAME_VERSION}")
}

/// Fetch the most recent releases. Network errors propagate; the caller treats
/// any failure as "up to date" so a flaky network never blocks the game.
pub(crate) fn fetch_releases(agent: &ureq::Agent) -> Result<Vec<Release>, String> {
    let response = agent
        .get(&releases_api_url())
        .set("User-Agent", &user_agent())
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| format!("releases request failed: {e}"))?;
    response
        .into_json::<Vec<Release>>()
        .map_err(|e| format!("releases response parse failed: {e}"))
}

/// The newest stable (non-draft, non-prerelease, semver-tagged) release.
pub(crate) fn latest_stable(releases: &[Release]) -> Option<&Release> {
    releases
        .iter()
        .filter(|r| r.is_stable())
        .max_by_key(|r| r.version().expect("is_stable implies a parseable version"))
}

/// Concatenated markdown changelog for every stable release strictly newer
/// than `current`, newest first. Empty when nothing is newer.
///
/// The bodies are compacted for the modal (see [`clean_release_body`]). When
/// more than one release is stacked, each is labelled with its version and
/// separated by a rule; for a single update the modal header already names it,
/// so no label is added.
pub(crate) fn changelog_since(releases: &[Release], current: Version) -> String {
    let mut newer: Vec<&Release> = releases
        .iter()
        .filter(|r| r.is_stable() && r.version().is_some_and(|v| v > current))
        .collect();
    newer.sort_by_key(|r| std::cmp::Reverse(r.version().expect("filtered to stable")));

    let multi = newer.len() > 1;
    newer
        .iter()
        .map(|r| {
            let body = clean_release_body(&r.body);
            let body = body.trim();
            let notes = if body.is_empty() {
                "_No release notes._"
            } else {
                body
            };
            if multi {
                format!("**{}**\n\n{notes}", display_version(&r.tag_name))
            } else {
                notes.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

/// `vX.Y.Z` for a tag, tolerating tags with or without a leading `v`.
fn display_version(tag: &str) -> String {
    let trimmed = tag.trim();
    if trimmed.starts_with(['v', 'V']) {
        trimmed.to_owned()
    } else {
        format!("v{trimmed}")
    }
}

/// Turn an auto-generated release body into compact in-app changelog markdown.
///
/// The release notes are written for the GitHub releases page, which repeats a
/// lot the modal already frames. This drops the redundant `## Ashwend vX`
/// title, the "Changes since vY." preamble, the "Release Assets" download
/// links, and the "Changelog" label, and renders the per-category headings as
/// **bold** lines instead of large markdown headings (so the modal isn't a
/// cascade of oversized headers). Bullets and prose are kept as-is.
fn clean_release_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut skipping = false; // inside the "Release Assets" section
    let mut skip_level = 0usize;
    for line in body.lines() {
        if let Some((level, title)) = heading(line) {
            // A heading at the same or shallower level ends a skipped section.
            if skipping && level <= skip_level {
                skipping = false;
            }
            if title.eq_ignore_ascii_case("release assets") {
                skipping = true;
                skip_level = level;
                continue;
            }
            if skipping {
                continue;
            }
            // Drop the top-level version title and the "Changelog" label; turn
            // every other heading (the categories) into a bold line.
            if level <= 2 || title.eq_ignore_ascii_case("changelog") {
                continue;
            }
            out.push_str("**");
            out.push_str(title);
            out.push_str("**\n");
            continue;
        }
        if skipping {
            continue;
        }
        // Drop the "Changes since vX." preamble.
        if line
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("changes since")
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// `(level, trimmed title)` for an ATX heading line (`#`..`######`), else None.
fn heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
        Some((hashes, trimmed[hashes..].trim()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, body: &str) -> Release {
        Release {
            tag_name: tag.to_owned(),
            body: body.to_owned(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        }
    }

    #[test]
    fn latest_stable_ignores_drafts_prereleases_and_bad_tags() {
        let mut draft = rel("v9.9.9", "");
        draft.draft = true;
        let mut pre = rel("v9.9.8", "");
        pre.prerelease = true;
        let releases = vec![
            rel("v0.16.2", ""),
            rel("v0.17.0", ""),
            rel("not-a-version", ""),
            draft,
            pre,
        ];
        assert_eq!(latest_stable(&releases).unwrap().tag_name, "v0.17.0");
    }

    #[test]
    fn changelog_since_collects_only_newer_stable_releases_newest_first() {
        let releases = vec![
            rel("v0.16.2", "## v0.16.2\nold"),
            rel("v0.17.0", "## v0.17.0\nmid"),
            rel("v0.18.0", "## v0.18.0\nnew"),
        ];
        let log = changelog_since(&releases, Version::parse("0.16.2").unwrap());
        // Several stacked releases get bold version labels, newest first.
        let pos_new = log.find("**v0.18.0**").expect("newest present");
        let pos_mid = log.find("**v0.17.0**").expect("middle present");
        assert!(pos_new < pos_mid, "newest must come first");
        assert!(!log.contains("v0.16.2"), "current version excluded");
        // The raw markdown title is dropped; the body prose is kept.
        assert!(!log.contains("## v0.18.0"));
        assert!(log.contains("new") && log.contains("mid"));
    }

    #[test]
    fn changelog_single_release_omits_the_version_label() {
        // One update → the modal header names the version, so the body doesn't
        // repeat it; the `## Ashwend` title is stripped either way.
        let body = "## Ashwend v0.17.0\n\nintro\n\n#### Feature\n- did a thing\n";
        let log = changelog_since(&[rel("v0.17.0", body)], Version::parse("0.16.0").unwrap());
        assert!(
            !log.contains("**v0.17.0**"),
            "no label for a single release"
        );
        assert!(!log.contains("## Ashwend"));
        assert!(log.contains("intro"));
        // Category heading is rendered as bold, not a large markdown heading.
        assert!(log.contains("**Feature**"));
        assert!(!log.contains("#### Feature"));
        assert!(log.contains("did a thing"));
    }

    #[test]
    fn changelog_since_is_empty_when_up_to_date() {
        let releases = vec![rel("v0.16.2", "notes")];
        assert!(changelog_since(&releases, Version::parse("0.16.2").unwrap()).is_empty());
        assert!(changelog_since(&releases, Version::parse("1.0.0").unwrap()).is_empty());
    }

    #[test]
    fn changelog_falls_back_when_body_is_only_assets() {
        let body =
            "## Ashwend v0.17.0\n\nChanges since v0.16.0.\n\n### Release Assets\n- [x](http://y)\n";
        let log = changelog_since(&[rel("v0.17.0", body)], Version::parse("0.16.0").unwrap());
        assert_eq!(log, "_No release notes._");
        assert!(!log.contains("Release Assets"));
        assert!(!log.contains("http://y"));
    }

    #[test]
    fn clean_release_body_compacts_to_bold_categories_and_drops_boilerplate() {
        let body = "## Ashwend v0.17.0\n\nChanges since v0.16.0.\n\nintro\n\n### Release Assets\n- [Linux](http://a)\n- [Mac](http://b)\n\n### Changelog\n\n#### Feature\n- did a thing\n";
        let cleaned = clean_release_body(body);
        assert!(cleaned.contains("intro"));
        assert!(cleaned.contains("**Feature**"));
        assert!(cleaned.contains("did a thing"));
        assert!(!cleaned.contains("## Ashwend"));
        assert!(!cleaned.contains("Changes since"));
        assert!(!cleaned.contains("Changelog"));
        assert!(!cleaned.contains("Release Assets"));
        assert!(!cleaned.contains("http://a"));
        assert!(!cleaned.contains("http://b"));
    }

    #[test]
    fn display_version_adds_v_prefix_when_missing() {
        assert_eq!(display_version("0.17.0"), "v0.17.0");
        assert_eq!(display_version("v0.17.0"), "v0.17.0");
    }

    #[test]
    fn heading_parses_atx_levels_only() {
        assert_eq!(heading("# Title"), Some((1, "Title")));
        assert_eq!(heading("###  Spaced  "), Some((3, "Spaced")));
        assert_eq!(heading("#NoSpace"), None);
        assert_eq!(heading("plain text"), None);
        assert_eq!(heading("####### too many"), None);
    }
}
