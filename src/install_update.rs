//! Verifies a portable GitHub release before offering in-place replacement.
//!
//! Both Windows helpers run fixed scripts. Their paths and other inputs are
//! JSON data on standard input.

use crate::{
    removal::{helper_command, process_creation, wait_for_helper},
    updates::ReleaseAsset,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    process::{self, Child, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{GetCurrentProcess, WaitForSingleObject},
};

pub const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
const STAGING_TIMEOUT_MS: u32 = 75_000;

#[derive(Serialize)]
struct StageRequest<'a> {
    url: &'a str,
    size: u64,
    sha256: String,
    version: &'a str,
    executable: &'a Path,
    staged: &'a Path,
    max_bytes: u64,
    parent_id: u32,
}

#[derive(Serialize, Deserialize)]
struct Fingerprints {
    original: String,
    staged: String,
}

#[derive(Serialize)]
struct InstallRequest<'a> {
    executable: &'a Path,
    staged: &'a Path,
    backup: &'a Path,
    fingerprints: &'a Fingerprints,
    parent_id: u32,
    parent_created: [u32; 2],
    report_failure: bool,
}

pub struct StagedUpdate {
    executable: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    fingerprints: Fingerprints,
    handed_off: bool,
}

impl StagedUpdate {
    /// After this returns success, exit normally so preferences are saved.
    pub fn start(mut self) -> Result<(), String> {
        let plan = InstallRequest {
            executable: &self.executable,
            staged: &self.staged,
            backup: &self.backup,
            fingerprints: &self.fingerprints,
            parent_id: process::id(),
            parent_created: process_creation(unsafe { GetCurrentProcess() })?,
            report_failure: true,
        };
        drop(start_helper(&plan)?);
        self.handed_off = true;
        Ok(())
    }
}

impl Drop for StagedUpdate {
    fn drop(&mut self) {
        if !self.handed_off {
            let _ = fs::remove_file(&self.staged);
        }
    }
}

/// Downloads and verifies an asset on a worker thread. Owns the staging file
/// until the caller either hands it to the replacement helper or drops it.
pub fn stage(asset: &ReleaseAsset, version: &str) -> Result<StagedUpdate, String> {
    let Some(sha256) = asset
        .sha256
        .filter(|_| (1..=MAX_ARCHIVE_BYTES).contains(&asset.size))
    else {
        return Err("This update does not have a supported verified download.".into());
    };
    let version = version.strip_prefix('v').unwrap_or(version);
    if version.split('.').count() != 3
        || version.split('.').any(|part| part.parse::<u16>().is_err())
    {
        return Err("The release has an invalid application version.".into());
    }
    let executable =
        std::env::current_exe().map_err(|_| "Could not locate the running executable.")?;
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Could not prepare a unique update filename.")?
        .as_nanos();
    let staged = executable.with_file_name(format!(".study-pen-update-{}-{id}.exe", process::id()));
    let backup = staged.with_extension("backup.exe");
    let mut hex = String::with_capacity(64);
    for byte in sha256 {
        write!(&mut hex, "{byte:02x}").expect("writing into a String succeeds");
    }
    let plan = StageRequest {
        url: &asset.url,
        size: asset.size,
        sha256: hex,
        version,
        executable: &executable,
        staged: &staged,
        max_bytes: MAX_ARCHIVE_BYTES,
        parent_id: process::id(),
    };
    let fingerprints = stage_files(&plan)?;
    Ok(StagedUpdate {
        executable,
        staged,
        backup,
        fingerprints,
        handed_off: false,
    })
}

fn stage_files(plan: &StageRequest<'_>) -> Result<Fingerprints, String> {
    let input = serde_json::to_vec(plan).map_err(|_| "Could not prepare the update download.")?;
    let mut child = helper_command(STAGE_SCRIPT)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start the Windows update verifier.")?;
    let input_writer = child.stdin.take().and_then(|mut stdin| {
        std::thread::Builder::new()
            .name("update-staging-input".into())
            .spawn(move || stdin.write_all(&input))
            .ok()
    });
    let Some(input_writer) = input_writer else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(
            "Could not send the download information to the Windows update verifier.".into(),
        );
    };
    wait_for_staging(&mut child, STAGING_TIMEOUT_MS)?;
    if !matches!(input_writer.join(), Ok(Ok(()))) {
        return Err(
            "Could not send the download information to the Windows update verifier.".into(),
        );
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "Could not verify the update download.")?;
    if !output.status.success() {
        return Err("Could not download, verify, or stage the update. You can download it manually from GitHub.".into());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "The Windows update verifier returned an invalid response.".into())
}

fn wait_for_staging(child: &mut Child, timeout_ms: u32) -> Result<(), String> {
    let message = match unsafe { WaitForSingleObject(child.as_raw_handle(), timeout_ms) } {
        WAIT_OBJECT_0 => return Ok(()),
        WAIT_TIMEOUT => "The update download timed out. You can download it manually from GitHub.",
        _ => "Could not wait for the Windows update verifier.",
    };
    let _ = child.kill();
    let _ = child.wait();
    Err(message.into())
}

fn start_helper(plan: &InstallRequest<'_>) -> Result<process::Child, String> {
    let input =
        serde_json::to_vec(plan).map_err(|_| "Could not prepare the update installation.")?;
    let mut child = helper_command(INSTALL_SCRIPT)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start the Windows update installer.")?;
    if let Err(error) = wait_for_helper(&mut child, input) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}

const STAGE_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$reader = [IO.StreamReader]::new([Console]::OpenStandardInput(), [Text.UTF8Encoding]::new($false, $true))
$plan = ConvertFrom-Json -InputObject ($reader.ReadToEnd())
$reader.Dispose()
$owner = [Diagnostics.Process]::GetProcessById([int]$plan.parent_id)
Add-Type -AssemblyName System.Net.Http
Add-Type -AssemblyName System.IO.Compression
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$handler = [Net.Http.HttpClientHandler]::new()
$handler.AllowAutoRedirect = $false
$handler.UseCookies = $false
$client = [Net.Http.HttpClient]::new($handler)
$client.DefaultRequestHeaders.UserAgent.ParseAdd('StudyPen')
$deadline = [Threading.CancellationTokenSource]::new(60000)
$response = $null
$archive = [IO.MemoryStream]::new()
$sha = [Security.Cryptography.SHA256]::Create()
$ownsStage = $false
$finished = $false
try {
    $uri = [Uri]::new([string]$plan.url)
    if ($uri.Host -ne 'github.com') { throw 'Invalid GitHub release URL.' }
    for ($hop = 0; $hop -lt 6; $hop++) {
        if ($uri.Scheme -ne 'https' -or !$uri.IsDefaultPort -or $uri.UserInfo -or
            $uri.Host -notin @('github.com', 'release-assets.githubusercontent.com', 'objects.githubusercontent.com')) {
            throw 'Invalid GitHub download redirect.'
        }
        $response = $client.GetAsync($uri, [Net.Http.HttpCompletionOption]::ResponseHeadersRead, $deadline.Token).GetAwaiter().GetResult()
        if ([int]$response.StatusCode -notin @(301, 302, 303, 307, 308)) { break }
        if (!$response.Headers.Location) { throw 'The update redirect has no destination.' }
        $uri = [Uri]::new($uri, $response.Headers.Location)
        $response.Dispose()
        $response = $null
    }
    if (!$response -or [int]$response.StatusCode -ne 200) { throw 'The download failed.' }
    $declared = $response.Content.Headers.ContentLength
    if ($null -ne $declared -and $declared -ne $plan.size) { throw 'Unexpected archive length.' }
    $network = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
    $buffer = [byte[]]::new(65536)
    while (($count = $network.ReadAsync($buffer, 0, $buffer.Length, $deadline.Token).GetAwaiter().GetResult()) -gt 0) {
        if ($archive.Length + $count -gt $plan.size) { throw 'Archive exceeds its approved size.' }
        $archive.Write($buffer, 0, $count)
    }
    if ($archive.Length -ne $plan.size) { throw 'Incomplete archive.' }
    $archive.Position = 0
    $actual = [BitConverter]::ToString($sha.ComputeHash($archive)).Replace('-', '')
    if ($actual -ne $plan.sha256) { throw 'The release archive checksum does not match.' }
    $archive.Position = 0
    $zip = [IO.Compression.ZipArchive]::new($archive, [IO.Compression.ZipArchiveMode]::Read, $true)
    try {
        $entry = $null
        foreach ($item in $zip.Entries) {
            if ($item.FullName -ceq 'study-pen.exe') {
                if ($entry) { throw 'Duplicate executable.' }
                $entry = $item
            }
        }
        if (!$entry -or $entry.Length -lt 64 -or $entry.Length -gt $plan.max_bytes -or $owner.HasExited) {
            throw 'The archive has no supported executable.'
        }
        $source = $entry.Open()
        try {
            $output = [IO.File]::Open($plan.staged, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
            $ownsStage = $true
            try {
                while (($count = $source.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    if ($output.Length + $count -gt $entry.Length) { throw 'Executable exceeds its archive entry size.' }
                    $output.Write($buffer, 0, $count)
                }
                if ($output.Length -ne $entry.Length) { throw 'Incomplete executable.' }
                $output.Flush($true)
            } finally { $output.Dispose() }
        } finally { $source.Dispose() }
    } finally { $zip.Dispose() }
    $image = [IO.BinaryReader]::new([IO.File]::OpenRead($plan.staged))
    try {
        if ($image.ReadUInt16() -ne 0x5a4d) { throw 'Invalid executable header.' }
        $image.BaseStream.Position = 0x3c
        $pe = $image.ReadUInt32()
        if ($pe -lt 64 -or $pe + 6 -gt $image.BaseStream.Length) { throw 'Invalid executable offset.' }
        $image.BaseStream.Position = $pe
        if ($image.ReadUInt32() -ne 0x4550 -or $image.ReadUInt16() -ne 0x8664) { throw 'Wrong executable architecture.' }
    } finally { $image.Dispose() }
    $metadata = [Diagnostics.FileVersionInfo]::GetVersionInfo($plan.staged)
    if ($metadata.ProductName -cne 'Study Pen' -or $metadata.ProductVersion -cne $plan.version) { throw 'Wrong release executable.' }
    function fingerprint($path) {
        if (([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Invalid file path.' }
        $stream = [IO.File]::OpenRead($path)
        try { [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '') }
        finally { $stream.Dispose() }
    }
    $result = @{ original = (fingerprint $plan.executable); staged = (fingerprint $plan.staged) }
    if ($owner.HasExited) { throw 'Study Pen closed.' }
    [Console]::Out.WriteLine(($result | ConvertTo-Json -Compress))
    [Console]::Out.Flush()
    $finished = $true
} finally {
    if ($ownsStage -and !$finished) { [IO.File]::Delete($plan.staged) }
    if ($response) { $response.Dispose() }
    $deadline.Dispose()
    $client.Dispose()
    $archive.Dispose()
    $sha.Dispose()
    $owner.Dispose()
}
"#;

const INSTALL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$reader = [IO.StreamReader]::new([Console]::OpenStandardInput(), [Text.UTF8Encoding]::new($false, $true))
$plan = ConvertFrom-Json -InputObject ($reader.ReadToEnd())
$reader.Dispose()
$owner = [Diagnostics.Process]::GetProcessById([int]$plan.parent_id)
$created = ([long]$plan.parent_created[1] -shl 32) -bor [long]$plan.parent_created[0]
if ($owner.StartTime.ToUniversalTime().ToFileTimeUtc() -ne $created) { exit 1 }
$sha = [Security.Cryptography.SHA256]::Create()
function matches($path, $expected) {
    if (![IO.File]::Exists($path)) { return $false }
    if (([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) -ne 0) { return $false }
    $stream = [IO.File]::OpenRead($path)
    try { return [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '') -eq $expected }
    finally { $stream.Dispose() }
}
if (!(matches $plan.executable $plan.fingerprints.original) -or !(matches $plan.staged $plan.fingerprints.staged) -or
    [IO.File]::Exists($plan.backup) -or [IO.Directory]::Exists($plan.backup)) { exit 1 }

function launch($path) {
    $info = [Diagnostics.ProcessStartInfo]::new($path)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.Arguments = '--after-update'
    $info.WorkingDirectory = [IO.Path]::GetDirectoryName($path)
    $child = [Diagnostics.Process]::Start($info)
    if (!$child) { throw 'Could not relaunch Study Pen.' }
    try {
        if ($child.WaitForExit(1200)) { throw 'Study Pen closed immediately after launch.' }
    } finally { $child.Dispose() }
}
$parentExited = $false
try {
    [Console]::Out.WriteLine('READY')
    [Console]::Out.Flush()
    if (!$owner.WaitForExit(120000)) { throw 'Study Pen did not exit.' }
    $parentExited = $true
    if (!(matches $plan.executable $plan.fingerprints.original) -or !(matches $plan.staged $plan.fingerprints.staged) -or
        [IO.File]::Exists($plan.backup) -or [IO.Directory]::Exists($plan.backup)) { throw 'An update file changed.' }
    [IO.File]::Replace($plan.staged, $plan.executable, $plan.backup)
    launch $plan.executable
    # A locked backup is recoverable and does not invalidate the active update.
    try {
        if (matches $plan.backup $plan.fingerprints.original) { [IO.File]::Delete($plan.backup) }
    } catch { }
} catch {
    if (matches $plan.backup $plan.fingerprints.original) {
        try {
            if (matches $plan.executable $plan.fingerprints.staged) { [IO.File]::Replace($plan.backup, $plan.executable, [NullString]::Value) }
            elseif (![IO.File]::Exists($plan.executable)) { [IO.File]::Move($plan.backup, $plan.executable) }
        } catch { }
    }
    if ($parentExited -and (matches $plan.executable $plan.fingerprints.original)) {
        try { launch $plan.executable } catch { }
    }
    if ($plan.report_failure) {
        Add-Type -AssemblyName System.Windows.Forms
        $message = 'Study Pen could not finish installing the update. Open the release page to update manually.'
        if (matches $plan.backup $plan.fingerprints.original) { $message += "`n`nYour previous executable is saved at $($plan.backup)." }
        [void][Windows.Forms.MessageBox]::Show($message, 'Study Pen update')
    }
    exit 1
} finally {
    if (matches $plan.staged $plan.fingerprints.staged) { [IO.File]::Delete($plan.staged) }
    $owner.Dispose()
    $sha.Dispose()
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const FIXTURE_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$path = [Console]::In.ReadToEnd()
Add-Type -OutputAssembly $path -OutputType ConsoleApplication -TypeDefinition @'
using System.IO;
using System.Threading;
public static class Fixture {
    public static int Main(string[] args) {
        if (args.Length != 1 || args[0] != "--after-update") return 2;
        if (File.Exists("lock-backup")) {
            using (File.OpenRead("backup.exe")) Thread.Sleep(2500);
        } else {
            Thread.Sleep(2000);
        }
        return 0;
    }
}
'@
"#;

    fn fixture_update(should_launch: bool, lock_backup: bool) {
        let directory = std::env::temp_dir().join(format!(
            "study-pen-update-test-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let executable = directory.join("study-pen.exe");
        let staged = directory.join("staged.exe");
        let backup = directory.join("backup.exe");
        let backup_lock = directory.join("lock-backup");
        struct FixtureCleanup(Vec<PathBuf>, PathBuf);
        impl Drop for FixtureCleanup {
            fn drop(&mut self) {
                for path in &self.0 {
                    let _ = fs::remove_file(path);
                }
                let _ = fs::remove_dir(&self.1);
            }
        }
        let _cleanup = FixtureCleanup(
            vec![
                executable.clone(),
                staged.clone(),
                backup.clone(),
                backup_lock.clone(),
            ],
            directory,
        );
        fs::write(&executable, b"previous copy").unwrap();
        if lock_backup {
            fs::write(backup_lock, []).unwrap();
        }
        if should_launch {
            let mut compiler = helper_command(FIXTURE_SCRIPT)
                .unwrap()
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            compiler
                .stdin
                .take()
                .unwrap()
                .write_all(staged.to_str().unwrap().as_bytes())
                .unwrap();
            assert!(compiler.wait().unwrap().success());
        } else {
            fs::write(&staged, b"unlaunchable fixture").unwrap();
        }

        let expected = fs::read(&staged).unwrap();
        let mut owner = helper_command("[void][Console]::In.ReadToEnd()")
            .unwrap()
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut hasher = helper_command("$paths = ConvertFrom-Json -InputObject ([Console]::In.ReadToEnd()); foreach ($path in $paths) { (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash }").unwrap()
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        serde_json::to_writer(hasher.stdin.take().unwrap(), &[&executable, &staged]).unwrap();
        let output = hasher.wait_with_output().unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        let mut sums = text.lines();
        let fingerprints = Fingerprints {
            original: sums.next().unwrap().to_owned(),
            staged: sums.next().unwrap().to_owned(),
        };
        let plan = InstallRequest {
            executable: &executable,
            staged: &staged,
            backup: &backup,
            fingerprints: &fingerprints,
            parent_id: owner.id(),
            parent_created: process_creation(owner.as_raw_handle()).unwrap(),
            report_failure: false,
        };
        let mut install = start_helper(&plan).unwrap();
        assert_eq!(fs::read(&executable).unwrap(), b"previous copy");
        assert!(!backup.exists());
        drop(owner.stdin.take());
        assert!(owner.wait().unwrap().success());
        let result = install.wait().unwrap();
        assert_eq!(result.success(), should_launch);
        assert_eq!(
            fs::read(&executable).unwrap(),
            if should_launch {
                expected.as_slice()
            } else {
                b"previous copy"
            }
        );
        assert!(!staged.exists());
        if lock_backup {
            assert_eq!(fs::read(&backup).unwrap(), b"previous copy");
        } else {
            assert!(!backup.exists());
        }
        // The successful fixture briefly keeps the staged image mapped.
        if should_launch {
            std::thread::sleep(Duration::from_millis(if lock_backup { 1500 } else { 1000 }));
        }
    }

    #[test]
    fn deferred_update_rolls_back_and_preserves_previous_executable_after_launch_failure() {
        fixture_update(false, false);
    }

    #[test]
    fn deferred_update_replaces_only_after_owner_exits_and_relaunches() {
        fixture_update(true, false);
    }

    #[test]
    fn a_locked_backup_does_not_roll_back_a_successful_update() {
        fixture_update(true, true);
    }

    #[test]
    fn staging_timeout_stops_the_blocked_verifier() {
        let mut child = helper_command("[void][Console]::In.ReadToEnd()")
            .unwrap()
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(
            wait_for_staging(&mut child, 25)
                .unwrap_err()
                .contains("timed out")
        );
        assert!(child.try_wait().unwrap().is_some());
    }
}
