use std::collections::HashSet;

use apkw_proto::apkw::v1::{
    AvailableToolchain, ToolchainArtifact, ToolchainProvider, ToolchainVersion,
};
use reqwest::Client;
use serde::Deserialize;

use crate::catalog::{
    available_for_provider, host_candidates, provider_from_catalog, Catalog, CatalogArtifact,
};

const GITHUB_API_BASE: &str = "https://api.github.com/repos";
const SDK_REPO: &str = "HomuHomu833/android-sdk-custom";
const NDK_REPO: &str = "HomuHomu833/android-ndk-custom";

#[derive(Clone)]
pub(crate) struct DiscoveredRelease {
    pub(crate) version: ToolchainVersion,
    pub(crate) artifact: CatalogArtifact,
    pub(crate) published_at: String,
    pub(crate) release_url: String,
    pub(crate) in_catalog: bool,
}

#[derive(Clone)]
pub(crate) struct UpstreamReleaseCheck {
    pub(crate) provider: ToolchainProvider,
    pub(crate) latest_catalog_version: String,
    pub(crate) latest_upstream_version: String,
    pub(crate) catalog_outdated: bool,
    pub(crate) releases: Vec<DiscoveredRelease>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct GithubRelease {
    pub(crate) tag_name: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) draft: bool,
    #[serde(default)]
    pub(crate) prerelease: bool,
    #[serde(default)]
    pub(crate) published_at: String,
    #[serde(default)]
    pub(crate) assets: Vec<GithubAsset>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct GithubAsset {
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) browser_download_url: String,
    #[serde(default)]
    pub(crate) size: u64,
    #[serde(default)]
    pub(crate) digest: Option<String>,
}

pub(crate) fn provider_repo(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "provider-android-sdk-custom" => Some(SDK_REPO),
        "provider-android-ndk-custom" => Some(NDK_REPO),
        _ => None,
    }
}

pub(crate) async fn fetch_repo_releases(repo: &str) -> Result<Vec<GithubRelease>, String> {
    let url = format!("{GITHUB_API_BASE}/{repo}/releases?per_page=100");
    let client = Client::builder()
        .user_agent("apkw-toolchain")
        .build()
        .map_err(|err| format!("failed to build github client: {err}"))?;
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| format!("github releases request failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "github releases request failed with status {}",
            response.status()
        ));
    }
    let body = response
        .text()
        .await
        .map_err(|err| format!("failed to read github releases response: {err}"))?;
    serde_json::from_str::<Vec<GithubRelease>>(&body)
        .map_err(|err| format!("failed to parse github releases response: {err}"))
}

pub(crate) fn build_release_check(
    catalog: &Catalog,
    provider_id: &str,
    host: &str,
    releases: &[GithubRelease],
) -> Result<UpstreamReleaseCheck, String> {
    let provider = catalog
        .providers
        .iter()
        .find(|item| item.provider_id == provider_id)
        .ok_or_else(|| format!("provider_id not found: {provider_id}"))?;
    let provider_proto = provider_from_catalog(provider);
    let candidates = host_candidates(host);
    let catalog_versions = provider
        .versions
        .iter()
        .map(|item| item.version.clone())
        .collect::<HashSet<_>>();
    let latest_catalog_version = available_for_provider(catalog, provider_id, host, None)
        .first()
        .and_then(|item| item.version.as_ref().map(|value| value.version.clone()))
        .unwrap_or_default();

    let releases = releases
        .iter()
        .filter(|release| !release.draft)
        .filter_map(|release| discovered_release(release, &candidates, &catalog_versions))
        .collect::<Vec<_>>();
    let latest_upstream_version = releases
        .first()
        .map(|item| item.version.version.clone())
        .unwrap_or_default();

    Ok(UpstreamReleaseCheck {
        provider: provider_proto,
        latest_catalog_version: latest_catalog_version.clone(),
        latest_upstream_version: latest_upstream_version.clone(),
        catalog_outdated: !latest_upstream_version.is_empty()
            && latest_upstream_version != latest_catalog_version,
        releases,
    })
}

pub(crate) fn discovered_release_to_available(
    provider: &ToolchainProvider,
    release: &DiscoveredRelease,
) -> AvailableToolchain {
    AvailableToolchain {
        provider: Some(provider.clone()),
        version: Some(release.version.clone()),
        artifact: Some(ToolchainArtifact {
            url: release.artifact.url.clone(),
            sha256: release.artifact.sha256.clone(),
            size_bytes: release.artifact.size_bytes,
        }),
    }
}

pub(crate) fn matching_release_artifact_any_host(
    releases: &[GithubRelease],
    version: &str,
    url: &str,
    sha256: &str,
) -> Option<CatalogArtifact> {
    let version = version.trim();
    releases
        .iter()
        .filter(|release| !release.draft)
        .find(|release| release_version(release) == version)
        .and_then(|release| {
            release.assets.iter().find(|asset| {
                asset.browser_download_url == url
                    || (!sha256.is_empty() && asset_sha256(asset).as_deref() == Some(sha256))
            })
        })
        .and_then(release_artifact_from_asset)
}

fn discovered_release(
    release: &GithubRelease,
    host_candidates: &[String],
    catalog_versions: &HashSet<String>,
) -> Option<DiscoveredRelease> {
    let version = release_version(release);
    let in_catalog = catalog_versions.contains(&version);
    let (host, asset, sha256) = select_host_asset(&release.assets, host_candidates)?;
    let published_at = release.published_at.trim().to_string();

    Some(DiscoveredRelease {
        version: ToolchainVersion {
            version: version.clone(),
            channel: release_channel(release),
            notes: release_notes(&published_at, in_catalog),
        },
        artifact: CatalogArtifact {
            host,
            url: asset.browser_download_url.clone(),
            sha256,
            size_bytes: asset.size,
            signature: String::new(),
            signature_url: String::new(),
            signature_public_key: String::new(),
            transparency_log_entry: String::new(),
            transparency_log_entry_url: String::new(),
            transparency_log_public_key: String::new(),
        },
        published_at,
        release_url: release.html_url.clone(),
        in_catalog,
    })
}

fn release_version(release: &GithubRelease) -> String {
    let name = release.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    release.tag_name.trim().to_string()
}

fn release_channel(release: &GithubRelease) -> String {
    if release.prerelease {
        return "prerelease".into();
    }

    let lower = release_version(release).to_ascii_lowercase();
    if lower.contains("alpha") || lower.contains("beta") || lower.contains("rc") {
        "prerelease".into()
    } else {
        "stable".into()
    }
}

fn release_notes(published_at: &str, in_catalog: bool) -> String {
    let source = if in_catalog {
        "Pinned in catalog and confirmed upstream."
    } else {
        "Discovered from upstream GitHub release; not pinned in the local catalog."
    };
    if published_at.is_empty() {
        source.into()
    } else {
        format!("{source} Published at {published_at}.")
    }
}

fn select_host_asset(
    assets: &[GithubAsset],
    host_candidates: &[String],
) -> Option<(String, GithubAsset, String)> {
    for candidate in host_candidates {
        if let Some(asset) = assets
            .iter()
            .find(|asset| asset_matches_host_candidate(&asset.name, candidate))
        {
            if let Some(sha256) = asset_sha256(asset) {
                return Some((candidate.clone(), asset.clone(), sha256));
            }
        }
    }
    None
}

fn asset_matches_host_candidate(asset_name: &str, candidate: &str) -> bool {
    asset_name.contains(&format!("-{candidate}."))
}

fn asset_sha256(asset: &GithubAsset) -> Option<String> {
    let digest = asset.digest.as_deref()?.trim();
    let digest = digest.strip_prefix("sha256:").unwrap_or(digest);
    if digest.is_empty() {
        None
    } else {
        Some(digest.to_string())
    }
}

fn release_artifact_from_asset(asset: &GithubAsset) -> Option<CatalogArtifact> {
    Some(CatalogArtifact {
        host: String::new(),
        url: asset.browser_download_url.clone(),
        sha256: asset_sha256(asset)?,
        size_bytes: asset.size,
        signature: String::new(),
        signature_url: String::new(),
        signature_public_key: String::new(),
        transparency_log_entry: String::new(),
        transparency_log_entry_url: String::new(),
        transparency_log_public_key: String::new(),
    })
}
