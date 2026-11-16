//! Deferred removal of one confirmed portable copy.
//!
//! The helper receives an exact manifest over stdin. It checks and deletes
//! each file on one handle, which prevents a path swap after verification.

use crate::{identity, settings::Settings};
use serde::Serialize;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{self, Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE},
    System::Threading::{CREATE_NO_WINDOW, GetCurrentProcess, GetProcessTimes},
};

const READ_ATTRIBUTES: u32 = 0x80;
const GENERIC_READ: u32 = 0x8000_0000;
const SHARE_READ: u32 = 1;
const SHARE_ALL: u32 = 7;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const UNSUPPORTED_FILE_ATTRIBUTES: u32 = 0x410; // directory or reparse point
const MAX_LINK_BYTES: u64 = 64 * 1024;

#[repr(C)]
#[derive(Default)]
struct FileInformation {
    attributes: u32,
    created: FILETIME,
    accessed: FILETIME,
    modified: FILETIME,
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

// These two kernel APIs keep the removal module independent of additional
// windows-sys feature groups in the main application.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandle(file: HANDLE, information: *mut FileInformation) -> i32;
    fn GetSystemDirectoryW(buffer: *mut u16, length: u32) -> u32;
}

#[derive(Clone, PartialEq, Eq, Serialize)]
struct ApprovedFile {
    path: String,
    stamp: [u32; 9],
    link_bytes: Option<String>,
}

#[derive(Serialize)]
struct Manifest {
    parent_id: u32,
    parent_created: [u32; 2],
    files: Vec<ApprovedFile>,
    report_failure: bool,
}

pub struct RemovalPlan {
    manifest: Manifest,
}

pub struct RemovalImage(ApprovedFile);

impl RemovalImage {
    pub fn capture() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|_| "Could not identify the running Study Pen executable.")?;
        snapshot(&executable, None)
            .map_err(|_| "Could not verify the running Study Pen executable for removal.")?
            .map(Self)
            .ok_or_else(|| "The running Study Pen executable is unavailable for removal.".into())
    }

    pub fn path(&self) -> &Path {
        Path::new(&self.0.path)
    }
}

impl RemovalPlan {
    /// Snapshot only the running copy and its exact owned files.
    pub fn prepare(image: &RemovalImage, settings: &Settings) -> Result<Self, String> {
        let executable = image.path();
        let current = snapshot(executable, None)
            .map_err(|_| "Could not verify the running Study Pen executable for removal.")?;
        if current.as_ref() != Some(&image.0) {
            return Err("This Study Pen executable changed or moved after launch.".into());
        }
        let mut files = vec![image.0.clone()];
        for base in settings
            .removal_settings_paths(executable)
            .into_iter()
            .flatten()
        {
            for path in settings_files(&base)
                .map_err(|_| "Could not inspect this copy's settings for removal.")?
            {
                if let Some(entry) = snapshot(&path, None).map_err(
                    |_| "Could not verify one of this copy's settings files for removal.",
                )? {
                    files.push(entry);
                }
            }
        }
        for owned_shortcut in [
            identity::owned_start_menu_shortcut,
            identity::owned_desktop_shortcut,
        ] {
            let Some(shortcut) = owned_shortcut()? else {
                continue;
            };
            // Hold the link against writes and replacement while rechecking
            // Shell ownership and taking the content snapshot.
            let locked = open_snapshot_file(&shortcut, true)
                .map_err(|_| "Could not open one of this copy's shortcuts for removal.")?;
            if owned_shortcut()?.as_deref() != Some(shortcut.as_path()) {
                return Err(
                    "A Study Pen shortcut changed. Review the removal and try again.".into(),
                );
            }
            let entry = snapshot(&shortcut, Some(locked))
                .map_err(|_| "Could not verify one of this copy's shortcuts for removal.")?
                .ok_or("A Study Pen shortcut is unavailable for removal.")?;
            if !files.iter().any(|file| file.path == entry.path) {
                files.push(entry);
            }
        }
        Ok(Self {
            manifest: Manifest {
                parent_id: process::id(),
                parent_created: process_creation(unsafe { GetCurrentProcess() })?,
                files,
                report_failure: true,
            },
        })
    }

    /// Starts the helper after user confirmation. After success the caller
    /// must quit without rewriting any files in the approved manifest.
    pub fn start(self) -> Result<(), String> {
        self.spawn_helper().map(drop)
    }

    fn spawn_helper(self) -> Result<Child, String> {
        let input = serde_json::to_vec(&self.manifest)
            .map_err(|_| "Could not encode the approved removal files.")?;
        let mut child = helper_command(REMOVAL_SCRIPT)?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Could not start the Windows removal helper.")?;
        if let Err(error) = wait_for_helper(&mut child, input) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(child)
    }
}

pub(crate) fn wait_for_helper(child: &mut Child, input: Vec<u8>) -> Result<(), String> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or("The Windows helper's input is unavailable.")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("The Windows helper's output is unavailable.")?;
    let (send, receive) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("windows-helper-ready".into())
        .spawn(move || {
            let written = stdin.write_all(&input);
            drop(stdin);
            let mut ready = String::new();
            let answer =
                written.and_then(|_| BufReader::new(stdout).take(256).read_line(&mut ready));
            let _ = send.send(answer.is_ok() && ready.trim() == "READY");
        })
        .map_err(|_| "Could not check whether the Windows helper is ready.")?;
    match receive.recv_timeout(Duration::from_secs(15)) {
        Ok(true) => Ok(()),
        _ => Err("Windows could not prepare the helper. Study Pen is still running.".into()),
    }
}

pub(crate) fn helper_command(script: &str) -> Result<Command, String> {
    let mut buffer = [0u16; 260];
    let size = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if size == 0 || size >= buffer.len() {
        return Err("Could not locate the Windows system directory.".into());
    }
    let directory = PathBuf::from(OsString::from_wide(&buffer[..size]))
        .join("WindowsPowerShell")
        .join("v1.0");
    let script: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut command = Command::new(directory.join("powershell.exe"));
    command
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(base64(&script))
        .env("PSModulePath", directory.join("Modules"))
        .creation_flags(CREATE_NO_WINDOW);
    Ok(command)
}

pub(crate) fn process_creation(handle: HANDLE) -> Result<[u32; 2], String> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err("Could not verify the running application process.".into());
    }
    Ok([created.dwLowDateTime, created.dwHighDateTime])
}

fn settings_files(base: &Path) -> io::Result<Vec<PathBuf>> {
    let Some(parent) = base.parent() else {
        return Ok(Vec::new());
    };
    let Some(name) = base.file_name().and_then(|value| value.to_str()) else {
        return Ok(Vec::new());
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let base_name = name.to_ascii_lowercase();
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| owned_settings_name(&base_name, name))
            && entry.file_type()?.is_file()
        {
            paths.push(entry.path());
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

fn owned_settings_name(base: &str, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let Some(suffix) = name.strip_prefix(base) else {
        return false;
    };
    if suffix.is_empty() || suffix == ".bak" {
        return true;
    }
    if let Some(sequence) = suffix.strip_prefix(".corrupt-") {
        return decimal(sequence).is_some_and(|value| value < 1000);
    }
    // Both the primary file and its backup use the atomic writer.
    let temporary = suffix
        .strip_prefix(".tmp-")
        .or_else(|| suffix.strip_prefix(".bak.tmp-"));
    temporary
        .and_then(|value| value.split_once('-'))
        .is_some_and(|(pid, sequence)| {
            decimal(pid).is_some_and(|value| (1..=u32::MAX as u64).contains(&value))
                && decimal(sequence).is_some()
        })
}

fn decimal(value: &str) -> Option<u64> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    value.parse().ok()
}

fn open_snapshot_file(path: &Path, lock_contents: bool) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(READ_ATTRIBUTES | if lock_contents { GENERIC_READ } else { 0 })
        .share_mode(if lock_contents { SHARE_READ } else { SHARE_ALL })
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
}

fn snapshot(path: &Path, locked_link: Option<File>) -> io::Result<Option<ApprovedFile>> {
    let is_link = locked_link.is_some();
    let mut file = match locked_link {
        Some(file) => file,
        None => match open_snapshot_file(path, false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        },
    };
    let mut info = FileInformation::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.attributes & UNSUPPORTED_FILE_ATTRIBUTES != 0
        || info.links != 1
        || (info.index_high | info.index_low) == 0
    {
        return Ok(None);
    }
    let path = path
        .to_str()
        .filter(|_| path.is_absolute())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid removal path"))?;
    let link_bytes = if is_link {
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_LINK_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_LINK_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Shortcut exceeds the removal snapshot limit",
            ));
        }
        Some(base64(&bytes))
    } else {
        None
    };
    Ok(Some(ApprovedFile {
        path: path.to_owned(),
        stamp: [
            info.volume,
            info.index_high,
            info.index_low,
            info.created.dwHighDateTime,
            info.created.dwLowDateTime,
            info.modified.dwHighDateTime,
            info.modified.dwLowDateTime,
            info.size_high,
            info.size_low,
        ],
        link_bytes,
    }))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(a >> 2) as usize] as char);
        output.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() >= 2 {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() == 3 {
            ALPHABET[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

// Keep this script fixed. File names and identities are data on stdin and
// never become PowerShell source or executable arguments.
const REMOVAL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$reader = [IO.StreamReader]::new([Console]::OpenStandardInput(), [Text.UTF8Encoding]::new($false, $true))
$plan = ConvertFrom-Json -InputObject ($reader.ReadToEnd())
$reader.Dispose()
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class StudyPenRemoval {
    [StructLayout(LayoutKind.Sequential)]
    private struct Info {
        public uint Attributes, CreatedLow, CreatedHigh, AccessedLow, AccessedHigh;
        public uint ModifiedLow, ModifiedHigh, Volume, SizeHigh, SizeLow;
        public uint Links, IndexHigh, IndexLow;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inherit, uint id);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetProcessTimes(IntPtr handle, out long created, out long exited, out long kernel, out long user);
    [DllImport("kernel32.dll")]
    public static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")]
    public static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security, uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(SafeFileHandle handle, out Info information);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetFileInformationByHandle(SafeFileHandle handle, int kind, ref byte deleteFile, uint length);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern int MessageBoxW(IntPtr owner, string text, string title, uint type);

    public static IntPtr Parent(uint id, uint low, uint high) {
        IntPtr handle = OpenProcess(0x00101000, false, id);
        if (handle == IntPtr.Zero) throw new IOException("Could not open the parent process.");
        long created, exited, kernel, user;
        if (!GetProcessTimes(handle, out created, out exited, out kernel, out user)
            || unchecked((ulong)created) != (((ulong)high << 32) | low)) {
            CloseHandle(handle);
            throw new IOException("The parent process identity changed.");
        }
        return handle;
    }

    public static int Remove(string path, uint[] stamp, byte[] linkBytes) {
        uint access = 0x00010080; // DELETE | FILE_READ_ATTRIBUTES
        if (linkBytes != null) access |= 0x80000000;
        using (SafeFileHandle handle = CreateFileW(path, access, 1, IntPtr.Zero, 3, 0x00200000, IntPtr.Zero)) {
            if (handle.IsInvalid) {
                int error = Marshal.GetLastWin32Error();
                return error == 2 || error == 3 ? 0 : error;
            }
            Info actual;
            if (!GetFileInformationByHandle(handle, out actual)) return Marshal.GetLastWin32Error();
            if ((actual.Attributes & 0x410) != 0 || actual.Links != 1 || stamp.Length != 9
                || actual.Volume != stamp[0] || actual.IndexHigh != stamp[1] || actual.IndexLow != stamp[2]
                || actual.CreatedHigh != stamp[3] || actual.CreatedLow != stamp[4]
                || actual.ModifiedHigh != stamp[5] || actual.ModifiedLow != stamp[6]
                || actual.SizeHigh != stamp[7] || actual.SizeLow != stamp[8]) return 10001;
            if (linkBytes != null) {
                using (SafeFileHandle borrowed = new SafeFileHandle(handle.DangerousGetHandle(), false))
                using (FileStream stream = new FileStream(borrowed, FileAccess.Read)) {
                    foreach (byte expected in linkBytes) if (stream.ReadByte() != expected) return 10001;
                    if (stream.ReadByte() != -1) return 10001;
                }
            }
            byte deleteFile = 1;
            return SetFileInformationByHandle(handle, 4, ref deleteFile, 1) ? 0 : Marshal.GetLastWin32Error();
        }
    }

    public static void ReportFailure() {
        MessageBoxW(IntPtr.Zero, "Study Pen could not remove every approved file. A file may have changed or still be in use.", "Study Pen removal", 0x30);
    }
}
'@
$parent = [StudyPenRemoval]::Parent([uint32]$plan.parent_id, [uint32]$plan.parent_created[0], [uint32]$plan.parent_created[1])
try {
    [Console]::Out.WriteLine('READY')
    [Console]::Out.Flush()
    if ([StudyPenRemoval]::WaitForSingleObject($parent, 120000) -ne 0) { throw 'The parent process did not exit.' }
    $failed = $false
    for ($index = 0; $index -lt $plan.files.Count; $index++) {
        $entry = $plan.files[$index]
        $bytes = $null
        if ($null -ne $entry.link_bytes) { $bytes = [Convert]::FromBase64String([string]$entry.link_bytes) }
        $status = [StudyPenRemoval]::Remove([string]$entry.path, [uint32[]]$entry.stamp, $bytes)
        if ($status -ne 0) {
            $failed = $true
            if ($index -eq 0) { break }
        }
    }
    if ($failed) {
        if ($plan.report_failure) { [StudyPenRemoval]::ReportFailure() }
        exit 1
    }
} catch {
    if ($plan.report_failure) { [StudyPenRemoval]::ReportFailure() }
    exit 1
} finally {
    [void][StudyPenRemoval]::CloseHandle($parent)
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            loop {
                let path = std::env::temp_dir().join(format!(
                    "study-pen-removal-test-{}-{} İ & ['scope']",
                    process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("Could not create the removal fixture: {error}"),
                }
            }
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let (Ok(path), Ok(parent)) =
                (self.0.canonicalize(), std::env::temp_dir().canonicalize())
                && path.parent() == Some(parent.as_path())
                && path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with("study-pen-removal-test-")
                })
            {
                let _ = fs::remove_dir_all(path);
            }
        }
    }

    struct ManagedChild(Child);

    impl ManagedChild {
        fn finish(&mut self) -> process::ExitStatus {
            assert_eq!(
                unsafe { WaitForSingleObject(self.0.as_raw_handle(), 15000) },
                0
            );
            self.0.wait().unwrap()
        }
    }

    impl Drop for ManagedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn settings_scope_matches_only_owned_names_in_their_exact_directory() {
        let fixture = Fixture::new();
        let exe = fixture.file("study-pen.exe", b"fixture");
        let portable = fixture.0.join("study-pen.settings.json");
        let legacy = fixture.0.join("local").join("settings.json");
        let settings = Settings::load_from_paths(portable.clone(), Some(legacy.clone()));
        let [portable_path, fallback] = settings.removal_settings_paths(&exe);
        assert_eq!(portable_path, Some(portable.clone()));
        assert!(fallback.unwrap().parent().unwrap().ends_with("portable"));
        assert_eq!(
            settings.removal_settings_paths(&fixture.0.join("other").join("study-pen.exe")),
            [None, None]
        );
        let variants = [".bak", ".corrupt-999", ".tmp-41-7", ".bak.tmp-41-8"];
        for suffix in variants {
            fixture.file(&format!("study-pen.settings.json{suffix}"), b"fixture");
        }
        for suffix in [
            ".corrupt-1000",
            ".corrupt-01",
            ".tmp-0-1",
            ".tmp-41-1.extra",
            ".archive",
            ".bak.bak",
        ] {
            fixture.file(&format!("study-pen.settings.json{suffix}"), b"unrelated");
        }
        fs::create_dir(fixture.0.join("study-pen.settings.json.corrupt-1")).unwrap();
        let mut expected = vec![portable.clone()];
        expected.extend(
            variants.map(|suffix| fixture.0.join(format!("study-pen.settings.json{suffix}"))),
        );
        expected.sort_unstable();
        assert_eq!(settings_files(&portable).unwrap(), expected);
    }

    #[test]
    fn deferred_helper_waits_and_preserves_replaced_files_and_changed_shortcut_bytes() {
        let fixture = Fixture::new();
        let executable = fixture.file("study-pen.exe", b"test image");
        let settings = fixture.file("study-pen.settings.json", b"owned preferences");
        let unchanged_link = fixture.file("owned.lnk", b"original link");
        let changed_link = fixture.file("changed.lnk", b"old link");
        let replaced = fixture.file("study-pen.settings.json.bak", b"backup");
        let link_snapshot = |path| {
            snapshot(path, Some(open_snapshot_file(path, true).unwrap()))
                .unwrap()
                .unwrap()
        };
        let approved = vec![
            snapshot(&executable, None).unwrap().unwrap(),
            snapshot(&settings, None).unwrap().unwrap(),
            link_snapshot(&unchanged_link),
            link_snapshot(&changed_link),
            snapshot(&replaced, None).unwrap().unwrap(),
        ];
        let old_write_time = fs::metadata(&changed_link).unwrap().modified().unwrap();
        let mut owner = ManagedChild(
            helper_command("[void][Console]::In.ReadToEnd()")
                .unwrap()
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut worker = ManagedChild(
            RemovalPlan {
                manifest: Manifest {
                    parent_id: owner.0.id(),
                    parent_created: process_creation(owner.0.as_raw_handle()).unwrap(),
                    files: approved,
                    report_failure: false,
                },
            }
            .spawn_helper()
            .unwrap(),
        );
        assert!(executable.exists() && settings.exists() && unchanged_link.exists());
        let original_backup = fixture.0.join("original-backup");
        fs::rename(&replaced, &original_backup).unwrap();
        fs::write(&replaced, b"backup").unwrap();
        fs::write(&changed_link, b"new link").unwrap();
        OpenOptions::new()
            .write(true)
            .open(&changed_link)
            .unwrap()
            .set_modified(old_write_time)
            .unwrap();
        assert_eq!(
            fs::metadata(&changed_link).unwrap().modified().unwrap(),
            old_write_time
        );
        drop(owner.0.stdin.take());
        assert!(owner.finish().success());
        assert_eq!(worker.finish().code(), Some(1));
        assert!(!executable.exists() && !settings.exists() && !unchanged_link.exists());
        assert_eq!(fs::read(&replaced).unwrap(), b"backup");
        assert_eq!(fs::read(&original_backup).unwrap(), b"backup");
        assert_eq!(fs::read(&changed_link).unwrap(), b"new link");
    }
}
