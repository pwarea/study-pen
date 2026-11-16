//! The executable's Windows identity and current-user shortcuts.

use std::{
    ffi::{OsStr, OsString, c_void},
    fs::{self, OpenOptions},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::{Path, PathBuf},
    ptr::{self, NonNull},
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::{
    Win32::{
        Foundation::{PROPERTYKEY, RPC_E_CHANGED_MODE},
        Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, MoveFileExW, WIN32_FIND_DATAW},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, STGM_READ,
                StructuredStorage::{PROPVARIANT, PropVariantClear, PropVariantToString},
            },
            LibraryLoader::GetModuleHandleW,
            Variant::VT_LPWSTR,
        },
        UI::{
            Shell::{
                FOLDERID_Desktop, FOLDERID_Programs, KF_FLAG_DONT_VERIFY, SHCNE_CREATE,
                SHCNF_PATHW, SHChangeNotify, SHGetKnownFolderPath, SLGP_RAWPATH,
                SetCurrentProcessExplicitAppUserModelID,
            },
            WindowsAndMessaging::{HICON, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW},
        },
    },
    core::{GUID, HRESULT, PCWSTR, PWSTR},
};

const APP_ID: &str = "StudyPen.Desktop";
const SHORTCUT_NAME: &str = "Study Pen.lnk";
const ICON_RESOURCE_ID: usize = 101;
const SHELL_LINK_CLASS: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
const SHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const PERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
const PROPERTY_STORE: GUID = GUID::from_u128(0x886d8eeb_8cf2_4446_8d02_cdba1dbdcf99);
const APP_ID_PROPERTY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutStatus {
    Created,
    AlreadyOwned,
    NameOccupied,
}

pub fn set_process_identity() -> Result<(), String> {
    let app_id = wide(APP_ID);
    check(
        unsafe { SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr()) },
        "Could not set the Windows application identity",
    )
}

/// Returns an owned icon handle. Release it with DestroyIcon after use.
pub unsafe fn load_icon(size: i32) -> Option<HICON> {
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return None;
    }
    let icon = unsafe {
        LoadImageW(
            instance,
            ICON_RESOURCE_ID as PCWSTR,
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
    } as HICON;
    (!icon.is_null()).then_some(icon)
}

pub fn ensure_start_menu_shortcut() -> Result<ShortcutStatus, String> {
    ensure_known_folder_shortcut(&FOLDERID_Programs, "Programs")
}

pub fn ensure_desktop_shortcut() -> Result<ShortcutStatus, String> {
    ensure_known_folder_shortcut(&FOLDERID_Desktop, "Desktop")
}

fn ensure_known_folder_shortcut(
    folder: &GUID,
    folder_name: &str,
) -> Result<ShortcutStatus, String> {
    let executable = current_executable()?;
    let shortcut = known_folder(folder, folder_name)?.join(SHORTCUT_NAME);
    let status = ensure_shortcut(&shortcut, &executable)?;
    if status == ShortcutStatus::Created {
        let path = wide(&shortcut);
        unsafe {
            SHChangeNotify(
                SHCNE_CREATE as i32,
                SHCNF_PATHW,
                path.as_ptr().cast(),
                ptr::null(),
            );
        }
    }
    Ok(status)
}

/// Identifies the current copy's shortcut for a separately verified removal.
/// A shortcut for another portable copy or a user-created link is left alone.
pub fn owned_start_menu_shortcut() -> Result<Option<PathBuf>, String> {
    owned_known_folder_shortcut(&FOLDERID_Programs, "Programs")
}

/// A desktop link is eligible for removal only if its identity and target
/// belong to the executable currently running.
pub fn owned_desktop_shortcut() -> Result<Option<PathBuf>, String> {
    owned_known_folder_shortcut(&FOLDERID_Desktop, "Desktop")
}

fn owned_known_folder_shortcut(
    folder: &GUID,
    folder_name: &str,
) -> Result<Option<PathBuf>, String> {
    let shortcut = known_folder(folder, folder_name)?.join(SHORTCUT_NAME);
    owned_shortcut(shortcut, &current_executable()?)
}

fn owned_shortcut(shortcut: PathBuf, executable: &Path) -> Result<Option<PathBuf>, String> {
    let _com = ComApartment::new()?;
    Ok(
        (existing_status(&shortcut, executable)? == Some(ShortcutStatus::AlreadyOwned))
            .then_some(shortcut),
    )
}

fn current_executable() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| io_error("Could not locate Study Pen", error))
}

fn known_folder(folder: &GUID, folder_name: &str) -> Result<PathBuf, String> {
    struct TaskString(PWSTR);
    impl Drop for TaskString {
        fn drop(&mut self) {
            unsafe {
                CoTaskMemFree(self.0.cast());
            }
        }
    }
    let mut result = TaskString(ptr::null_mut());
    check(
        unsafe {
            SHGetKnownFolderPath(
                folder,
                KF_FLAG_DONT_VERIFY as u32,
                ptr::null_mut(),
                &mut result.0,
            )
        },
        &format!("Could not find the current user's {folder_name} folder"),
    )?;
    if result.0.is_null() {
        return Err(format!(
            "Windows returned an empty {folder_name} folder path."
        ));
    }
    let mut length = 0;
    unsafe {
        while *result.0.add(length) != 0 {
            length += 1;
        }
        let value = std::slice::from_raw_parts(result.0, length);
        Ok(PathBuf::from(OsString::from_wide(value)))
    }
}

fn ensure_shortcut(shortcut: &Path, executable: &Path) -> Result<ShortcutStatus, String> {
    let _com = ComApartment::new()?;
    if let Some(status) = existing_status(shortcut, executable)? {
        return Ok(status);
    }
    let directory = shortcut
        .parent()
        .ok_or("The shortcut has no parent directory.")?;
    fs::create_dir_all(directory)
        .map_err(|error| io_error("Could not open the shortcut folder", error))?;
    let temporary = TemporaryLink::new(directory)?;
    write_shortcut(&temporary.0, executable)?;
    let source = wide(&temporary.0);
    let destination = wide(shortcut);
    // No replacement flag: even a link created after the initial check is kept.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
        let error = io::Error::last_os_error();
        if let Some(status) = existing_status(shortcut, executable)? {
            return Ok(status);
        }
        return Err(io_error("Could not install the shortcut", error));
    }
    Ok(ShortcutStatus::Created)
}

fn existing_status(shortcut: &Path, executable: &Path) -> Result<Option<ShortcutStatus>, String> {
    match fs::symlink_metadata(shortcut) {
        Ok(metadata) => {
            let owned = metadata.is_file()
                && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
                && shortcut_matches(shortcut, executable).unwrap_or(false);
            Ok(Some(if owned {
                ShortcutStatus::AlreadyOwned
            } else {
                ShortcutStatus::NameOccupied
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error("Could not inspect the shortcut", error)),
    }
}

struct TemporaryLink(PathBuf);

impl TemporaryLink {
    fn new(directory: &Path) -> Result<Self, String> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        for _ in 0..16 {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = directory.join(format!(
                ".study-pen-link-{}-{sequence}.tmp",
                std::process::id()
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(_) => return Ok(Self(candidate)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error("Could not prepare the shortcut", error)),
            }
        }
        Err("Could not reserve a temporary file for the shortcut.".into())
    }
}

impl Drop for TemporaryLink {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn write_shortcut(destination: &Path, executable: &Path) -> Result<(), String> {
    let link = Interface::shell_link()?;
    let path = wide(executable);
    let work = wide(
        executable
            .parent()
            .ok_or("Study Pen has no parent folder.")?,
    );
    let description = wide("Study Pen");
    unsafe {
        let vtable = link.vtable::<ShellLinkVtable>();
        check(
            (vtable.set_path)(link.as_ptr(), path.as_ptr()),
            "Could not set the shortcut target",
        )?;
        check(
            (vtable.set_working_directory)(link.as_ptr(), work.as_ptr()),
            "Could not set the shortcut working folder",
        )?;
        check(
            (vtable.set_description)(link.as_ptr(), description.as_ptr()),
            "Could not name the shortcut",
        )?;
        check(
            (vtable.set_icon_location)(link.as_ptr(), path.as_ptr(), 0),
            "Could not set the shortcut icon",
        )?;
        let store = link.query(&PROPERTY_STORE)?;
        let store_methods = store.vtable::<PropertyStoreVtable>();
        let mut app_id = wide(APP_ID);
        // The value borrows this buffer through Commit. Only native values
        // returned by GetValue below require PropVariantClear.
        let mut value = PROPVARIANT::default();
        value.Anonymous.Anonymous.vt = VT_LPWSTR;
        value.Anonymous.Anonymous.Anonymous.pwszVal = app_id.as_mut_ptr();
        check(
            (store_methods.set_value)(store.as_ptr(), &APP_ID_PROPERTY, &value),
            "Could not associate the shortcut with Study Pen",
        )?;
        check(
            (store_methods.commit)(store.as_ptr()),
            "Could not commit the shortcut identity",
        )?;
        let persistence = link.query(&PERSIST_FILE)?;
        let destination = wide(destination);
        check(
            (persistence.vtable::<PersistFileVtable>().save)(
                persistence.as_ptr(),
                destination.as_ptr(),
                0,
            ),
            "Could not save the shortcut",
        )
    }
}

fn shortcut_matches(shortcut: &Path, executable: &Path) -> Result<bool, String> {
    let link = Interface::shell_link()?;
    let persistence = link.query(&PERSIST_FILE)?;
    let file = wide(shortcut);
    unsafe {
        check(
            (persistence.vtable::<PersistFileVtable>().load)(
                persistence.as_ptr(),
                file.as_ptr(),
                STGM_READ,
            ),
            "Could not read the existing shortcut",
        )?;
        let store = link.query(&PROPERTY_STORE)?;
        let mut value = OwnedVariant(PROPVARIANT::default());
        check(
            (store.vtable::<PropertyStoreVtable>().get_value)(
                store.as_ptr(),
                &APP_ID_PROPERTY,
                &mut value.0,
            ),
            "Could not read the shortcut identity",
        )?;
        let mut identity = [0u16; 128];
        if PropVariantToString(&value.0, identity.as_mut_ptr(), identity.len() as u32) < 0
            || terminated(&identity) != terminated(&wide(APP_ID))
        {
            return Ok(false);
        }
        let methods = link.vtable::<ShellLinkVtable>();
        let mut arguments = [0u16; 2];
        if (methods.get_arguments)(
            link.as_ptr(),
            arguments.as_mut_ptr(),
            arguments.len() as i32,
        ) < 0
            || arguments[0] != 0
        {
            return Ok(false);
        }
        let mut target = vec![0u16; 32768];
        check(
            (methods.get_path)(
                link.as_ptr(),
                target.as_mut_ptr(),
                target.len() as i32,
                ptr::null_mut(),
                SLGP_RAWPATH as u32,
            ),
            "Could not read the shortcut target",
        )?;
        let target = PathBuf::from(OsString::from_wide(terminated(&target)));
        // Never call Resolve: Windows could redirect an old shortcut to a
        // different portable copy. Both paths must presently exist and match.
        Ok(target.is_absolute()
            && matches!((fs::canonicalize(target), fs::canonicalize(executable)),
                (Ok(target), Ok(executable)) if target == executable))
    }
}

struct OwnedVariant(PROPVARIANT);
impl Drop for OwnedVariant {
    fn drop(&mut self) {
        unsafe {
            PropVariantClear(&mut self.0);
        }
    }
}

struct ComApartment(bool);
impl ComApartment {
    fn new() -> Result<Self, String> {
        let result = unsafe { CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if result == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        check(result, "Could not initialize Windows shell access")?;
        Ok(Self(true))
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

struct Interface(NonNull<c_void>);
impl Interface {
    fn shell_link() -> Result<Self, String> {
        let mut result = ptr::null_mut();
        check(
            unsafe {
                CoCreateInstance(
                    &SHELL_LINK_CLASS,
                    ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &SHELL_LINK_W,
                    &mut result,
                )
            },
            "Could not create a Windows shell shortcut",
        )?;
        Self::from_result(result)
    }

    fn from_result(pointer: *mut c_void) -> Result<Self, String> {
        NonNull::new(pointer)
            .map(Self)
            .ok_or_else(|| "Windows returned an empty shell interface.".into())
    }

    fn query(&self, interface: &GUID) -> Result<Self, String> {
        let mut result = ptr::null_mut();
        check(
            unsafe {
                (self.vtable::<UnknownVtable>().query)(self.as_ptr(), interface, &mut result)
            },
            "Could not open the Windows shell interface",
        )?;
        Self::from_result(result)
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
    unsafe fn vtable<T>(&self) -> &T {
        unsafe { &**self.as_ptr().cast::<*const T>() }
    }
}
impl Drop for Interface {
    fn drop(&mut self) {
        unsafe {
            (self.vtable::<UnknownVtable>().release)(self.as_ptr());
        }
    }
}

// windows-sys exposes COM functions and data types but no interface methods.
// Unused slots preserve the layout in the Windows SDK's COM declarations.
#[repr(C)]
struct UnknownVtable {
    query: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct ShellLinkVtable {
    unknown: UnknownVtable,
    get_path:
        unsafe extern "system" fn(*mut c_void, PWSTR, i32, *mut WIN32_FIND_DATAW, u32) -> HRESULT,
    id_list_and_get_description: [usize; 3],
    set_description: unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT,
    get_working_directory: usize,
    set_working_directory: unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT,
    get_arguments: unsafe extern "system" fn(*mut c_void, PWSTR, i32) -> HRESULT,
    arguments_hotkey_show_and_get_icon: [usize; 6],
    set_icon_location: unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT,
    relative_path_and_resolve: [usize; 2],
    set_path: unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT,
}

#[repr(C)]
struct PersistFileVtable {
    unknown: UnknownVtable,
    get_class_and_is_dirty: [usize; 2],
    load: unsafe extern "system" fn(*mut c_void, PCWSTR, u32) -> HRESULT,
    save: unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT,
}

#[repr(C)]
struct PropertyStoreVtable {
    unknown: UnknownVtable,
    count_and_get_at: [usize; 2],
    get_value:
        unsafe extern "system" fn(*mut c_void, *const PROPERTYKEY, *mut PROPVARIANT) -> HRESULT,
    set_value:
        unsafe extern "system" fn(*mut c_void, *const PROPERTYKEY, *const PROPVARIANT) -> HRESULT,
    commit: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

fn check(result: HRESULT, message: &str) -> Result<(), String> {
    if result < 0 {
        Err(format!(
            "{message} (Windows error 0x{:08X}).",
            result as u32
        ))
    } else {
        Ok(())
    }
}

fn io_error(message: &str, error: io::Error) -> String {
    format!("{message}: {error}")
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn terminated(value: &[u16]) -> &[u16] {
    &value[..value
        .iter()
        .position(|item| *item == 0)
        .unwrap_or(value.len())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "study-pen-identity-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            Self(directory)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn native_shortcut_round_trips_and_cannot_be_claimed_by_another_copy() {
        let fixture = Fixture::new();
        let shortcut = fixture.0.join(SHORTCUT_NAME);
        let executable = current_executable().unwrap();
        assert_eq!(
            ensure_shortcut(&shortcut, &executable).unwrap(),
            ShortcutStatus::Created
        );
        assert_eq!(
            ensure_shortcut(&shortcut, &executable).unwrap(),
            ShortcutStatus::AlreadyOwned
        );
        let original = fs::read(&shortcut).unwrap();
        let other_copy = fixture.0.join("other study-pen.exe");
        fs::write(&other_copy, b"Another portable copy").unwrap();
        assert_eq!(
            ensure_shortcut(&shortcut, &other_copy).unwrap(),
            ShortcutStatus::NameOccupied
        );
        assert_eq!(fs::read(&shortcut).unwrap(), original);
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 2);
    }

    #[test]
    fn ownership_lookup_keeps_desktop_and_programs_links_independent() {
        let fixture = Fixture::new();
        let desktop = fixture.0.join("Desktop").join(SHORTCUT_NAME);
        let programs = fixture.0.join("Programs").join(SHORTCUT_NAME);
        let executable = current_executable().unwrap();
        let other_executable = fixture.0.join("other study-pen.exe");
        fs::write(&other_executable, b"Another portable copy").unwrap();

        assert_eq!(
            ensure_shortcut(&desktop, &executable).unwrap(),
            ShortcutStatus::Created
        );
        assert_eq!(
            ensure_shortcut(&programs, &other_executable).unwrap(),
            ShortcutStatus::Created
        );
        assert_eq!(
            owned_shortcut(desktop.clone(), &executable).unwrap(),
            Some(desktop.clone())
        );
        assert_eq!(owned_shortcut(programs, &executable).unwrap(), None);

        fs::remove_file(&desktop).unwrap();
        assert_eq!(owned_shortcut(desktop.clone(), &executable).unwrap(), None);
        fs::write(&desktop, b"An unrelated desktop file").unwrap();
        assert_eq!(
            ensure_shortcut(&desktop, &executable).unwrap(),
            ShortcutStatus::NameOccupied
        );
        assert_eq!(owned_shortcut(desktop, &executable).unwrap(), None);
    }

    #[test]
    fn an_unrelated_file_or_manual_link_keeps_its_start_name() {
        let fixture = Fixture::new();
        let shortcut = fixture.0.join(SHORTCUT_NAME);
        let executable = current_executable().unwrap();
        let existing = b"Unrelated existing file";
        fs::write(&shortcut, existing).unwrap();
        assert_eq!(
            ensure_shortcut(&shortcut, &executable).unwrap(),
            ShortcutStatus::NameOccupied
        );
        assert_eq!(fs::read(&shortcut).unwrap(), existing);

        let _com = ComApartment::new().unwrap();
        let link = Interface::shell_link().unwrap();
        let persistence = link.query(&PERSIST_FILE).unwrap();
        unsafe {
            check(
                (link.vtable::<ShellLinkVtable>().set_path)(
                    link.as_ptr(),
                    wide(&executable).as_ptr(),
                ),
                "Could not create the manual test shortcut",
            )
            .unwrap();
            check(
                (persistence.vtable::<PersistFileVtable>().save)(
                    persistence.as_ptr(),
                    wide(&shortcut).as_ptr(),
                    0,
                ),
                "Could not save the manual test shortcut",
            )
            .unwrap();
        }
        let manual_link = fs::read(&shortcut).unwrap();
        assert_eq!(
            ensure_shortcut(&shortcut, &executable).unwrap(),
            ShortcutStatus::NameOccupied
        );
        assert_eq!(fs::read(&shortcut).unwrap(), manual_link);
    }

    #[test]
    fn executable_icon_loads_at_shell_sizes() {
        for size in [16, 24, 32, 48, 64, 128, 256] {
            let icon = unsafe { load_icon(size) }
                .unwrap_or_else(|| panic!("Missing {size}px icon resource"));
            assert_ne!(unsafe { DestroyIcon(icon) }, 0);
        }
    }
}
