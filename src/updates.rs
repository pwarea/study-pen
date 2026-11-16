//! Optional GitHub Releases checks using the operating system's HTTPS client.
//! Call from a worker thread. No network request is made without a repository.

use serde::Deserialize;
use std::{cmp::Ordering, ffi::c_void, ptr};
use windows_sys::Win32::Networking::WinHttp::*;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const PORTABLE_ASSET: &str = "study-pen-windows-x86_64-portable.zip";
const BUILD_REPOSITORY: Option<&str> = option_env!("STUDY_PEN_GITHUB_REPOSITORY");

pub fn repository(configured: Option<&str>) -> Option<&str> {
    configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(BUILD_REPOSITORY)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub url: String,
    pub size: u64,
    pub sha256: Option<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: String,
    pub url: String,
    pub asset: Option<ReleaseAsset>,
    trusted_repository: bool,
}

impl ReleaseInfo {
    pub fn installable(&self) -> bool {
        self.trusted_repository
            && !self.version.contains('+')
            && stable_version(&self.version)
                .is_some_and(|parts| parts.into_iter().all(|part| u16::try_from(part).is_ok()))
            && self.asset.as_ref().is_some_and(|asset| {
                asset.sha256.is_some()
                    && (1..=crate::install_update::MAX_ARCHIVE_BYTES).contains(&asset.size)
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Unconfigured,
    UpToDate,
    Available(ReleaseInfo),
    Failed(String),
}

impl UpdateStatus {
    pub fn message(&self) -> String {
        match self {
            Self::Unconfigured => {
                "This build will check for updates once a GitHub repository is configured.".into()
            }
            Self::UpToDate => "You have the latest version.".into(),
            Self::Available(release) => {
                format!("Study Pen {} is available.", release.version)
            }
            Self::Failed(message) => message.clone(),
        }
    }
}

pub fn check(repository: Option<&str>, current_version: &str) -> UpdateStatus {
    let Some(repository) = repository.map(str::trim).filter(|value| !value.is_empty()) else {
        return UpdateStatus::Unconfigured;
    };
    if !valid_repository(repository) {
        return UpdateStatus::Failed(
            "The GitHub repository must use the owner/repository format.".into(),
        );
    }
    match fetch_latest_release(repository)
        .and_then(|release| release_status(repository, current_version, release))
    {
        Ok(status) => status,
        Err(message) => UpdateStatus::Failed(message),
    }
}

fn release_status(
    repository: &str,
    current_version: &str,
    release: Release,
) -> Result<UpdateStatus, String> {
    if release.draft || release.prerelease {
        return Err("No stable release was found.".into());
    }
    let latest = stable_version(&release.tag_name)
        .ok_or("The release tag must use a version such as v1.2.3.")?;
    let current =
        stable_version(current_version).ok_or("The application version could not be compared.")?;
    if latest.cmp(&current) != Ordering::Greater {
        return Ok(UpdateStatus::UpToDate);
    }

    let tag = &release.tag_name;
    let root = format!("https://github.com/{repository}/releases");
    let download_url = format!("{root}/download/{tag}/{PORTABLE_ASSET}");
    let asset = release.assets.into_iter().find_map(|asset| {
        (asset.name == PORTABLE_ASSET
            && asset.state == "uploaded"
            && asset.size > 0
            && asset
                .browser_download_url
                .eq_ignore_ascii_case(&download_url))
        .then(|| ReleaseAsset {
            url: download_url.clone(),
            size: asset.size,
            sha256: asset.digest.as_deref().and_then(parse_sha256),
        })
    });

    let url = format!("{root}/tag/{tag}");
    Ok(UpdateStatus::Available(ReleaseInfo {
        version: release.tag_name,
        url,
        asset,
        trusted_repository: BUILD_REPOSITORY
            .is_some_and(|value| value.eq_ignore_ascii_case(repository)),
    }))
}

fn parse_sha256(digest: &str) -> Option<[u8; 32]> {
    let hex = digest.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(result)
}

fn valid_repository(repository: &str) -> bool {
    let mut parts = repository.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(name) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && (1..=39).contains(&owner.len())
        && (1..=100).contains(&name.len())
        && owner != "."
        && owner != ".."
        && name != "."
        && name != ".."
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn stable_version(value: &str) -> Option<[u64; 3]> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let value = if let Some((version, metadata)) = value.split_once('+') {
        if metadata.split('.').any(|identifier| {
            identifier.is_empty()
                || !identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }) {
            return None;
        }
        version
    } else {
        value
    };
    let mut numbers = value.split('.');
    let mut result = [0; 3];
    for number in &mut result {
        let part = numbers.next()?;
        if part.is_empty()
            || (part.len() > 1 && part.starts_with('0'))
            || !part.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        *number = part.parse().ok()?;
    }
    numbers.next().is_none().then_some(result)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    state: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

struct InternetHandle(*mut c_void);

impl InternetHandle {
    fn checked(handle: *mut c_void) -> Result<Self, String> {
        if handle.is_null() {
            Err("Could not connect to GitHub. Check your connection and try again.".into())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for InternetHandle {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn fetch_latest_release(repository: &str) -> Result<Release, String> {
    unsafe {
        let session = InternetHandle::checked(WinHttpOpen(
            wide("StudyPen").as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        ))?;
        if WinHttpSetTimeouts(session.0, 3000, 4000, 4000, 5000) == 0 {
            return Err("Could not initialize the update connection.".into());
        }
        let connection = InternetHandle::checked(WinHttpConnect(
            session.0,
            wide("api.github.com").as_ptr(),
            443,
            0,
        ))?;
        let request = InternetHandle::checked(WinHttpOpenRequest(
            connection.0,
            wide("GET").as_ptr(),
            wide(&format!("/repos/{repository}/releases/latest")).as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        let headers =
            wide("Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n");
        if WinHttpSendRequest(
            request.0,
            headers.as_ptr(),
            (headers.len() - 1) as u32,
            ptr::null(),
            0,
            0,
            0,
        ) == 0
            || WinHttpReceiveResponse(request.0, ptr::null_mut()) == 0
        {
            return Err("Could not check for updates. Check your connection and try again.".into());
        }
        let mut status: u32 = 0;
        let mut length = std::mem::size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            ptr::null(),
            (&mut status as *mut u32).cast(),
            &mut length,
            ptr::null_mut(),
        ) == 0
        {
            return Err("GitHub returned an unreadable response.".into());
        }
        match status {
            200 => {}
            404 => return Err("No public release was found for the configured repository.".into()),
            403 | 429 => return Err("GitHub is limiting update checks. Try again later.".into()),
            _ => {
                return Err(format!(
                    "GitHub could not complete the update check (HTTP {status})."
                ));
            }
        }
        let mut bytes = Vec::with_capacity(8192);
        let mut buffer = [0u8; 8192];
        loop {
            let mut read = 0;
            if WinHttpReadData(
                request.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
            ) == 0
            {
                return Err("The update response could not be read.".into());
            }
            if read == 0 {
                break;
            }
            if bytes.len() + read as usize > MAX_RESPONSE_BYTES {
                return Err("The update response is larger than expected.".into());
            }
            bytes.extend_from_slice(&buffer[..read as usize]);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| "GitHub returned invalid release information.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_repository_does_not_require_a_network_connection() {
        assert_eq!(check(None, "0.1.0"), UpdateStatus::Unconfigured);
        assert_eq!(check(Some(" "), "0.1.0"), UpdateStatus::Unconfigured);
    }

    #[test]
    fn repository_validation_prevents_unexpected_paths() {
        assert!(valid_repository("example/study-pen"));
        assert!(!valid_repository("https://github.com/example/study-pen"));
        assert!(!valid_repository("example/../other"));
        assert!(!valid_repository("example/study-pen?token=secret"));
        assert!(!valid_repository("example/.."));
    }

    #[test]
    fn an_explicit_repository_overrides_the_build_repository() {
        assert_eq!(
            repository(Some(" example/study-pen ")),
            Some("example/study-pen")
        );
        assert_eq!(repository(Some("  ")), BUILD_REPOSITORY);
        assert_eq!(repository(None), BUILD_REPOSITORY);
    }

    #[test]
    fn versions_compare_numerically_and_reject_prereleases() {
        assert!(stable_version("v1.10.0").unwrap() > stable_version("1.9.9").unwrap());
        assert_eq!(stable_version("0.1.0+build"), Some([0, 1, 0]));
        assert!(stable_version("1.2.3-beta.1").is_none());
        assert!(stable_version("1.2").is_none());
        assert!(stable_version("1.2.3.4").is_none());
        assert!(stable_version("1.2.3+bad/path").is_none());
        assert!(stable_version("1.2.3+build..test").is_none());
        assert!(stable_version("1.2.03").is_none());
    }

    const TEST_REPOSITORY: &str = "example/study-pen";
    const TEST_ROOT: &str = "https://github.com/example/study-pen/releases";

    fn test_asset(name: &str, tag: &str, digest: Option<String>) -> Asset {
        Asset {
            name: name.into(),
            state: "uploaded".into(),
            browser_download_url: format!("{TEST_ROOT}/download/{tag}/{name}"),
            size: 1024,
            digest,
        }
    }

    fn test_release(assets: Vec<Asset>) -> ReleaseInfo {
        let status = release_status(
            TEST_REPOSITORY,
            "0.4.1",
            Release {
                tag_name: "v0.4.2".into(),
                prerelease: false,
                draft: false,
                assets,
            },
        )
        .unwrap();
        let UpdateStatus::Available(release) = status else {
            panic!("the newer release should be available");
        };
        release
    }

    #[test]
    fn portable_download_uses_the_exact_current_release_asset() {
        let mut wrong_host = test_asset(PORTABLE_ASSET, "v0.4.2", None);
        wrong_host.browser_download_url = "https://example.org/payload.zip".into();
        let mut unuploaded = test_asset(PORTABLE_ASSET, "v0.4.2", None);
        unuploaded.state = "starter".into();
        let release = test_release(vec![
            test_asset("study-pen-0.4.2-source.zip", "v0.4.2", None),
            test_asset(PORTABLE_ASSET, "v0.4.1", None),
            wrong_host,
            unuploaded,
            test_asset(
                PORTABLE_ASSET,
                "v0.4.2",
                Some(format!("sha256:{}", "A1".repeat(32))),
            ),
        ]);

        assert_eq!(release.version, "v0.4.2");
        assert_eq!(release.url, format!("{TEST_ROOT}/tag/v0.4.2"));
        let asset = release.asset.unwrap();
        assert_eq!(
            asset.url,
            format!("{TEST_ROOT}/download/v0.4.2/{PORTABLE_ASSET}")
        );
        assert_eq!(asset.size, 1024);
        assert_eq!(asset.sha256, Some([0xA1; 32]));
    }

    #[test]
    fn installer_requires_a_trusted_source_digest_and_bounded_asset() {
        let mut release = test_release(vec![test_asset(PORTABLE_ASSET, "v0.4.2", None)]);
        release.trusted_repository = true;
        assert!(!release.installable());
        release.asset.as_mut().unwrap().sha256 = Some([42; 32]);
        assert!(release.installable());
        release.trusted_repository = false;
        assert!(!release.installable());
        release.trusted_repository = true;
        release.asset.as_mut().unwrap().size = crate::install_update::MAX_ARCHIVE_BYTES + 1;
        assert!(!release.installable());
        assert_eq!(parse_sha256("sha256:abc"), None);
        assert_eq!(parse_sha256(&format!("sha256:{}", "x".repeat(64))), None);
    }

    #[test]
    fn a_release_without_the_portable_asset_still_has_release_notes() {
        let release = test_release(vec![test_asset("replacement.zip", "v0.4.2", None)]);
        assert!(release.asset.is_none());
        assert_eq!(release.url, format!("{TEST_ROOT}/tag/v0.4.2"));
    }
}
