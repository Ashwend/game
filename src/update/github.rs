//! GitHub Releases client + the pure "what's newer than me" logic.
//!
//! The repo is public, so the releases API needs no auth — only the
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
pub(crate) fn changelog_since(releases: &[Release], current: Version) -> String {
    let mut newer: Vec<&Release> = releases
        .iter()
        .filter(|r| r.is_stable() && r.version().is_some_and(|v| v > current))
        .collect();
    newer.sort_by_key(|r| std::cmp::Reverse(r.version().expect("filtered to stable")));

    newer
        .iter()
        .map(|r| {
            let body = strip_release_assets_section(&r.body);
            let body = body.trim();
            if body.is_empty() {
                format!("## {}\n\n_No release notes._", r.tag_name)
            } else {
                body.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Remove the auto-generated "Release Assets" section (download links) from a
/// release body — useful in-app, where the player updates from the modal and
/// doesn't need raw artifact links. Drops everything from a heading whose text
/// is "Release Assets" up to the next heading of the same-or-shallower level
/// (or end of body). Any other markdown is left untouched.
fn strip_release_assets_section(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut skipping = false;
    let mut skip_level = 0usize;
    for line in body.lines() {
        if let Some((level, title)) = heading(line) {
            if title.eq_ignore_ascii_case("release assets") {
                skipping = true;
                skip_level = level;
                continue;
            }
            // A heading at the same or shallower level ends the skipped block.
            if skipping && level <= skip_level {
                skipping = false;
            }
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
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
        let pos_new = log.find("## v0.18.0").expect("newest present");
        let pos_mid = log.find("## v0.17.0").expect("middle present");
        assert!(pos_new < pos_mid, "newest must come first");
        assert!(!log.contains("## v0.16.2"), "current version excluded");
    }

    #[test]
    fn changelog_since_is_empty_when_up_to_date() {
        let releases = vec![rel("v0.16.2", "notes")];
        assert!(changelog_since(&releases, Version::parse("0.16.2").unwrap()).is_empty());
        assert!(changelog_since(&releases, Version::parse("1.0.0").unwrap()).is_empty());
    }

    #[test]
    fn changelog_falls_back_when_body_is_only_assets() {
        let body = "## Ashwend v0.17.0\n\n### Release Assets\n- [x](http://y)\n";
        let log = changelog_since(&[rel("v0.17.0", body)], Version::parse("0.16.0").unwrap());
        assert!(log.contains("## Ashwend v0.17.0"));
        assert!(!log.contains("Release Assets"));
    }

    #[test]
    fn strip_release_assets_keeps_changelog_and_drops_links() {
        let body = "## Ashwend v0.17.0\n\nintro\n\n### Release Assets\n- [Linux](http://a)\n- [Mac](http://b)\n\n### Changelog\n\n#### Feature\n- did a thing\n";
        let cleaned = strip_release_assets_section(body);
        assert!(cleaned.contains("## Ashwend v0.17.0"));
        assert!(cleaned.contains("intro"));
        assert!(cleaned.contains("### Changelog"));
        assert!(cleaned.contains("did a thing"));
        assert!(!cleaned.contains("Release Assets"));
        assert!(!cleaned.contains("http://a"));
        assert!(!cleaned.contains("http://b"));
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
