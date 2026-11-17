use crate::toolbar_preferences::{self, ToolbarButton, ToolbarTheme};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    cell::RefCell,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

const MODIFIER_MASK: u32 = 0x000f;
const MAX_SETTINGS_BYTES: u64 = 64 * 1024;
const PORTABLE_SETTINGS_NAME: &str = "study-pen.settings.json";
const HOTKEY_DEFAULT_REVISION: u32 = 1;
const MISSING_HOTKEY: Hotkey = Hotkey::new(0, u32::MAX);
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    TogglePen,
    Clear,
    ObjectEraser,
    Undo,
    Eraser,
    SwapColor,
    ToggleUi,
    Quit,
    RadialMenu,
}

impl Action {
    pub const ALL: [Self; 9] = [
        Self::TogglePen,
        Self::Clear,
        Self::ObjectEraser,
        Self::Undo,
        Self::Eraser,
        Self::SwapColor,
        Self::ToggleUi,
        Self::Quit,
        Self::RadialMenu,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::TogglePen => "Toggle pen",
            Self::Clear => "Clear screen",
            Self::ObjectEraser => "Object eraser",
            Self::Undo => "Undo",
            Self::Eraser => "Eraser / hold to resize",
            Self::SwapColor => "Change color",
            Self::ToggleUi => "Hide / show toolbar",
            Self::Quit => "Exit Study Pen",
            Self::RadialMenu => "Radial menu",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

impl Hotkey {
    pub const fn new(modifiers: u32, key: u32) -> Self {
        Self { modifiers, key }
    }

    pub const fn unbound() -> Self {
        Self::new(0, 0)
    }

    pub const fn is_bound(self) -> bool {
        self.key != 0 || self.modifiers != 0
    }

    pub fn is_valid(self) -> bool {
        self.validation_error().is_none()
    }

    pub fn validation_error(self) -> Option<&'static str> {
        if !self.is_bound() {
            return None;
        }
        if self.modifiers & !MODIFIER_MASK != 0
            || !matches!(
                self.key,
                0x08 | 0x09 | 0x0d | 0x13 | 0x1b | 0x20..=0x28 | 0x2d | 0x2e
                    | 0x30..=0x39 | 0x41..=0x5a | 0x60..=0x87
                    | 0xba..=0xc0 | 0xdb..=0xde
            )
        {
            return Some("Choose a letter, number, function key or navigation key.");
        }
        if self.modifiers & 8 != 0 {
            return Some("Windows-key shortcuts are reserved for the operating system.");
        }
        if self.key == 0x7b {
            return Some("F12 is reserved by Windows for debugging.");
        }
        if self.key == 0x1b {
            return Some("Escape is reserved for canceling dialogs and shortcut capture.");
        }
        let alt = self.modifiers & 1 != 0;
        let control = self.modifiers & 2 != 0;
        let shift = self.modifiers & 4 != 0;
        if alt && control && self.key == 0x2e {
            return Some("Ctrl + Alt + Delete is reserved for Windows security.");
        }
        if alt && self.key == 0x09 {
            return Some("Alt + Tab shortcuts are reserved for switching windows.");
        }
        if alt && !control && self.key == 0x20 {
            return Some("Alt + Space is reserved for the window menu.");
        }
        if alt && !control && self.key == 0x73 {
            return Some("Alt + F4 is reserved for closing windows.");
        }
        if self.key == 0x79 && self.modifiers == 0 {
            return Some("F10 is reserved for activating application menus.");
        }
        if self.key == 0x79 && self.modifiers == 4 {
            return Some("Shift + F10 is reserved for opening context menus.");
        }
        let printable = matches!(self.key, 0x20 | 0x30..=0x39 | 0x41..=0x5a | 0x60..=0x6f | 0xba..=0xc0 | 0xdb..=0xde);
        if alt && control && printable {
            return Some(
                "Ctrl + Alt with a character key can type AltGr characters. Choose a function key or another combination.",
            );
        }
        if printable && !control {
            return Some("Include Ctrl with a character key to protect normal typing.");
        }
        let function_key = (0x70..=0x87).contains(&self.key);
        if !function_key && !control && !shift {
            return Some("Include Ctrl or Shift with a non-function key to protect normal typing.");
        }
        None
    }

    pub fn display(self) -> String {
        if !self.is_bound() {
            return "Unbound".into();
        }
        let mut parts = Vec::with_capacity(5);
        if self.modifiers & 2 != 0 {
            parts.push("Ctrl".to_owned());
        }
        if self.modifiers & 1 != 0 {
            parts.push("Alt".to_owned());
        }
        if self.modifiers & 4 != 0 {
            parts.push("Shift".to_owned());
        }
        if self.modifiers & 8 != 0 {
            parts.push("Win".to_owned());
        }
        let name = match self.key {
            0x08 => "Backspace".into(),
            0x09 => "Tab".into(),
            0x0d => "Enter".into(),
            0x13 => "Pause".into(),
            0x1b => "Esc".into(),
            0x20 => "Space".into(),
            0x21 => "Page Up".into(),
            0x22 => "Page Down".into(),
            0x23 => "End".into(),
            0x24 => "Home".into(),
            0x25 => "Left".into(),
            0x26 => "Up".into(),
            0x27 => "Right".into(),
            0x28 => "Down".into(),
            0x2d => "Insert".into(),
            0x2e => "Delete".into(),
            0x30..=0x39 | 0x41..=0x5a => char::from_u32(self.key).unwrap().to_string(),
            0x60..=0x69 => format!("Num {}", self.key - 0x60),
            0x6a => "Num *".into(),
            0x6b => "Num +".into(),
            0x6c => "Num Separator".into(),
            0x6d => "Num -".into(),
            0x6e => "Num .".into(),
            0x6f => "Num /".into(),
            0x70..=0x87 => format!("F{}", self.key - 0x6f),
            0xba => ";".into(),
            0xbb => "=".into(),
            0xbc => ",".into(),
            0xbd => "-".into(),
            0xbe => ".".into(),
            0xbf => "/".into(),
            0xc0 => "`".into(),
            0xdb => "[".into(),
            0xdc => "\\".into(),
            0xdd => "]".into(),
            0xde => "'".into(),
            _ => format!("Key {:02X}", self.key),
        };
        parts.push(name);
        parts.join(" + ")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub primary_color: String,
    pub secondary_color: String,
    pub pen_width: f32,
    pub eraser_diameter: f32,
    #[serde(deserialize_with = "deserialize_hotkeys")]
    pub hotkeys: [Hotkey; 9],
    #[serde(default)]
    pub hotkey_default_revision: u32,
    pub check_for_updates: bool,
    pub github_repository: Option<String>,
    pub toolbar_position: Option<(i32, i32)>,
    pub toolbar_visible: bool,
    pub desktop_shortcut_seen: bool,
    #[serde(default)]
    pub toolbar_vertical: bool,
    #[serde(deserialize_with = "toolbar_preferences::deserialize_theme")]
    pub toolbar_theme: ToolbarTheme,
    #[serde(deserialize_with = "toolbar_preferences::deserialize_labels")]
    pub toolbar_labels: bool,
    #[serde(deserialize_with = "toolbar_preferences::deserialize_buttons")]
    pub toolbar_buttons: Vec<ToolbarButton>,
    #[serde(skip)]
    storage: RefCell<Storage>,
    #[serde(skip)]
    hotkey_notice: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Storage {
    portable: Option<PathBuf>,
    fallback: Option<PathBuf>,
    location: Option<PathBuf>,
    warning: Option<String>,
}

fn default_hotkeys() -> [Hotkey; 9] {
    std::array::from_fn(|index| {
        if index == Action::RadialMenu.index() {
            Hotkey::unbound()
        } else {
            Hotkey::new(7, 0x70 + index as u32)
        }
    })
}

fn legacy_hotkeys() -> [Hotkey; 8] {
    (*b"DCOZEXHQ").map(|key| Hotkey::new(3, key as u32))
}

fn deserialize_hotkeys<'de, D>(deserializer: D) -> Result<[Hotkey; 9], D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<Hotkey>::deserialize(deserializer)?;
    if !matches!(values.len(), 6 | 8 | 9) {
        return Err(serde::de::Error::custom(
            "expected six, eight or nine shortcuts",
        ));
    }
    // Missing actions in the original six-entry format stay distinguishable
    // until normalization chooses this version's defaults or preserves customs.
    let mut hotkeys = [Hotkey::unbound(); 9];
    if values.len() == 6 {
        hotkeys[6..8].fill(MISSING_HOTKEY);
    }
    hotkeys[..values.len()].copy_from_slice(&values);
    Ok(hotkeys)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            primary_color: "#6366F1".into(),
            secondary_color: "#F43F5E".into(),
            pen_width: 3.0,
            eraser_diameter: 36.0,
            hotkeys: default_hotkeys(),
            hotkey_default_revision: HOTKEY_DEFAULT_REVISION,
            check_for_updates: true,
            github_repository: None,
            toolbar_position: None,
            toolbar_visible: true,
            desktop_shortcut_seen: false,
            toolbar_vertical: false,
            toolbar_theme: ToolbarTheme::default(),
            toolbar_labels: false,
            toolbar_buttons: toolbar_preferences::default_buttons(),
            storage: RefCell::default(),
            hotkey_notice: None,
        }
    }
}

impl Settings {
    /// Binds storage to the running app, preferring a sidecar beside its executable.
    /// Filesystem tests use `load_from_paths` so test executables never gain sidecars.
    #[cfg(not(test))]
    pub fn load() -> Self {
        let portable = std::env::current_exe().ok().and_then(|path| {
            path.parent()
                .map(|parent| parent.join(PORTABLE_SETTINGS_NAME))
        });
        let fallback = std::env::var_os("LOCALAPPDATA")
            .filter(|root| !root.is_empty())
            .map(|root| PathBuf::from(root).join("StudyPen").join("settings.json"));
        match portable {
            Some(path) => Self::load_from_paths(path, fallback),
            None => Self::load_locations(None, fallback, None),
        }
    }

    #[cfg(test)]
    pub fn load() -> Self {
        Self::default()
    }

    /// Binds an explicit portable path and an optional legacy local-data path.
    /// The paired fallback is derived from this copy's portable path.
    pub fn load_from_paths(portable: PathBuf, legacy: Option<PathBuf>) -> Self {
        let paired = legacy
            .as_ref()
            .and_then(|legacy| paired_settings_path(&portable, legacy));
        Self::load_locations(Some(portable), paired, legacy)
    }

    fn load_locations(
        portable: Option<PathBuf>,
        fallback: Option<PathBuf>,
        legacy: Option<PathBuf>,
    ) -> Self {
        let (portable_candidate, portable_problem) = read_candidate(portable.as_deref());
        let (fallback_candidate, fallback_problem) = read_candidate(fallback.as_deref());
        // Only this copy's paired fallback can supersede a valid sidecar.
        // Shared legacy preferences are migration input, never an override.
        let use_fallback = match (&portable_candidate, &fallback_candidate) {
            (Some(portable), Some(fallback)) => fallback.modified > portable.modified,
            (None, Some(_)) => true,
            _ => false,
        };
        let use_legacy = portable_candidate.is_none() && fallback_candidate.is_none();
        let (legacy_candidate, legacy_problem) = if use_legacy {
            read_candidate(legacy.as_deref())
        } else {
            (None, false)
        };
        let candidate = if use_fallback {
            fallback_candidate
        } else if use_legacy {
            legacy_candidate
        } else {
            portable_candidate
        };
        let restored_defaults = candidate.is_none();
        let needs_initial_save = use_fallback
            || use_legacy
            || candidate
                .as_ref()
                .is_none_or(|candidate| candidate.recovered);
        let recovered = candidate
            .as_ref()
            .is_some_and(|candidate| candidate.recovered);
        let location = candidate.as_ref().and_then(|_| {
            if use_fallback {
                fallback.clone()
            } else if use_legacy {
                legacy.clone()
            } else {
                portable.clone()
            }
        });
        let mut settings = candidate
            .map(|candidate| candidate.settings)
            .unwrap_or_default();
        settings.normalize();
        *settings.storage.borrow_mut() = Storage {
            portable,
            fallback,
            location,
            warning: None,
        };
        if needs_initial_save && let Err(error) = settings.save() {
            settings.storage.borrow_mut().warning = Some(error);
        }
        if recovered || portable_problem || fallback_problem || legacy_problem {
            let explanation = if recovered {
                "Settings were recovered from a backup."
            } else if restored_defaults {
                "Unreadable settings were preserved before restoring defaults."
            } else {
                "An unreadable settings file was ignored; readable preferences were kept."
            };
            let mut storage = settings.storage.borrow_mut();
            storage.warning = Some(match storage.warning.take() {
                Some(existing) => format!("{existing} {explanation}"),
                None => explanation.to_owned(),
            });
        }
        settings
    }

    pub fn persistence_location(&self) -> Option<PathBuf> {
        self.storage.borrow().location.clone()
    }

    /// Returns only this executable's portable sidecar and paired fallback.
    /// Shared legacy preferences can be imported by multiple copies.
    pub fn removal_settings_paths(&self, executable: &Path) -> [Option<PathBuf>; 2] {
        let Some(directory) = executable.parent() else {
            return [None, None];
        };
        let expected = directory.join(PORTABLE_SETTINGS_NAME);
        let storage = self.storage.borrow();
        if storage.portable.as_deref() == Some(expected.as_path()) {
            [storage.portable.clone(), storage.fallback.clone()]
        } else {
            [None, None]
        }
    }

    pub fn persistence_warning(&self) -> Option<String> {
        match (&self.storage.borrow().warning, &self.hotkey_notice) {
            (Some(storage), Some(hotkeys)) => Some(format!("{storage} {hotkeys}")),
            (Some(message), None) | (None, Some(message)) => Some(message.clone()),
            (None, None) => None,
        }
    }

    pub fn persistence_description(&self) -> String {
        let location = self.persistence_location();
        let storage = self.storage.borrow();
        if location.is_none() {
            "Settings storage is unavailable.".into()
        } else if location == storage.portable {
            "Portable settings are saved beside the application.".into()
        } else {
            "Settings are saved in the local application data folder.".into()
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| "Could not encode settings.")?;
        let storage = self.storage.borrow().clone();
        if let Some(path) = &storage.portable
            && write_settings(path, &bytes).is_ok()
        {
            let mut storage = self.storage.borrow_mut();
            storage.location = Some(path.clone());
            storage.warning = None;
            return Ok(());
        }
        if let Some(path) = &storage.fallback
            && write_settings(path, &bytes).is_ok()
        {
            let mut storage = self.storage.borrow_mut();
            storage.location = Some(path.clone());
            storage.warning = Some(
                "The application folder is not writable. Preferences for this portable copy are saved in the local application data folder.".into(),
            );
            return Ok(());
        }
        let error = if storage.portable.is_none() && storage.fallback.is_none() {
            "Settings storage has not been initialized."
        } else {
            "Preferences could not be saved beside the application or in local application data. Keep the application in a writable folder."
        };
        self.storage.borrow_mut().warning = Some(error.into());
        Err(error.into())
    }

    pub fn validate_hotkey(&self, action: Action, hotkey: Hotkey) -> Result<(), String> {
        if let Some(message) = hotkey.validation_error() {
            return Err(message.into());
        }
        if !hotkey.is_bound() {
            return Ok(());
        }
        if let Some(other) = Action::ALL
            .into_iter()
            .find(|other| *other != action && self.hotkeys[other.index()] == hotkey)
        {
            return Err(format!(
                "This shortcut is already assigned to {}.",
                other.label().to_lowercase()
            ));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        toolbar_preferences::sanitize_buttons(&mut self.toolbar_buttons);
        let defaults = Self::default();
        let previous_hotkeys = self.hotkeys;
        if self.hotkey_default_revision == 0 {
            let legacy = legacy_hotkeys();
            let exact_six = self.hotkeys[..6] == legacy[..6]
                && self.hotkeys[6..8].iter().all(|key| *key == MISSING_HOTKEY);
            let exact_eight = self.hotkeys[..8] == legacy;
            if !self.hotkeys[Action::RadialMenu.index()].is_bound() && (exact_eight || exact_six) {
                self.hotkeys = defaults.hotkeys;
            }
        }
        self.hotkey_default_revision = self.hotkey_default_revision.max(HOTKEY_DEFAULT_REVISION);
        self.primary_color = normalize_color(&self.primary_color).unwrap_or(defaults.primary_color);
        self.secondary_color =
            normalize_color(&self.secondary_color).unwrap_or(defaults.secondary_color);
        self.pen_width = bounded(self.pen_width, 1.0, 32.0, defaults.pen_width);
        self.eraser_diameter = bounded(self.eraser_diameter, 6.0, 240.0, defaults.eraser_diameter);
        // Retain valid custom assignments before repairing invalid or duplicate ones.
        // New actions must not steal an older action's custom shortcut on upgrade.
        let mut reserved: Vec<Hotkey> = self
            .hotkeys
            .iter()
            .copied()
            .filter(|key| key.is_bound() && key.is_valid())
            .collect();
        for index in 0..self.hotkeys.len() {
            let key = self.hotkeys[index];
            if !key.is_bound() || (key.is_valid() && !self.hotkeys[..index].contains(&key)) {
                continue;
            }
            let preferred = defaults.hotkeys[index];
            let replacement = if !reserved.contains(&preferred) {
                preferred
            } else {
                (0x70..=0x87)
                    .map(|key| Hotkey::new(7, key))
                    .find(|key| key.is_valid() && !reserved.contains(key))
                    .expect("nine actions cannot occupy all twenty-four function keys")
            };
            self.hotkeys[index] = replacement;
            reserved.push(replacement);
        }
        if self.hotkeys != previous_hotkeys {
            self.hotkey_notice = Some(
                "Some shortcuts were updated for this version. Review them in Shortcuts.".into(),
            );
        }
        self.github_repository = self
            .github_repository
            .take()
            .map(|repository| repository.trim().to_owned())
            .filter(|repository| !repository.is_empty());
    }
}

struct Candidate {
    settings: Settings,
    modified: SystemTime,
    recovered: bool,
}

fn paired_settings_path(portable: &Path, legacy: &Path) -> Option<PathBuf> {
    let directory = legacy.parent()?;
    let normalized = normalized_portable_path(portable)?;
    // FNV-1a is a stable namespace key, not a security or integrity mechanism.
    // Only the key is used in the fallback filename; paths are never serialized.
    let mut key = 0xcbf29ce484222325u64;
    for byte in normalized.as_bytes() {
        key = (key ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    Some(directory.join("portable").join(format!("{key:016x}.json")))
}

fn normalized_portable_path(path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let text = absolute.to_string_lossy().replace('/', "\\").to_lowercase();
    let text = if let Some(unc) = text.strip_prefix(r"\\?\unc\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    };
    let unc = text.starts_with(r"\\");
    let mut components = Vec::new();
    for component in text.split('\\') {
        match component {
            "" | "." => {}
            ".." if components
                .last()
                .is_some_and(|part: &&str| !part.ends_with(':')) =>
            {
                components.pop();
            }
            ".." => {}
            _ => components.push(component),
        }
    }
    let path = components.join("\\");
    Some(if unc { format!(r"\\{path}") } else { path })
}

fn read_candidate(path: Option<&Path>) -> (Option<Candidate>, bool) {
    let Some(path) = path else {
        return (None, false);
    };
    match read_settings(path) {
        Ok(Some(candidate)) => (Some(candidate), false),
        primary => {
            let problem = primary.is_err();
            match read_settings(&suffixed_path(path, ".bak")) {
                Ok(Some(mut candidate)) => {
                    candidate.recovered = true;
                    (Some(candidate), problem)
                }
                _ => (None, problem),
            }
        }
    }
}

fn read_settings(path: &Path) -> Result<Option<Candidate>, ()> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    if metadata.len() > MAX_SETTINGS_BYTES {
        return Err(());
    }
    let bytes = fs::read(path).map_err(|_| ())?;
    let settings = serde_json::from_slice(&bytes).map_err(|_| ())?;
    Ok(Some(Candidate {
        settings,
        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        recovered: false,
    }))
}

fn write_settings(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("The settings directory is unavailable.")?;
    fs::create_dir_all(parent).map_err(|_| "Could not create the settings directory.")?;
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_SETTINGS_BYTES => preserve_corrupt(path)?,
        Ok(_) => {
            let previous =
                fs::read(path).map_err(|_| "The previous preferences could not be read safely.")?;
            if serde_json::from_slice::<Settings>(&previous).is_ok() {
                write_atomic(&suffixed_path(path, ".bak"), &previous)?;
            } else {
                preserve_corrupt(path)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("The previous preferences could not be read safely.".into()),
    }
    write_atomic(path, bytes)
}

fn preserve_corrupt(path: &Path) -> Result<(), String> {
    for suffix in 0..1000 {
        let destination = suffixed_path(path, &format!(".corrupt-{suffix}"));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("The unreadable settings file could not be preserved.".into()),
        };
        let mut source = fs::File::open(path)
            .map_err(|_| "The unreadable settings file could not be preserved.")?;
        return std::io::copy(&mut source, &mut file)
            .and_then(|_| file.sync_all())
            .map_err(|_| "The unreadable settings file could not be preserved.".into());
    }
    Err("There are too many preserved unreadable settings files.".into())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = suffixed_path(path, &format!(".tmp-{}-{sequence}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "Could not create a settings transaction.")?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "Could not save preferences to disk.")?;
        drop(file);
        fs::rename(&temporary, path).map_err(|_| "Could not replace the settings file.")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn suffixed_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_owned();
    name.push(suffix);
    path.with_file_name(name)
}

fn normalize_color(value: &str) -> Option<String> {
    let digits = value.trim().trim_start_matches('#');
    if digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(format!("#{}", digits.to_ascii_uppercase()))
    } else {
        None
    }
}

fn bounded(value: f32, minimum: f32, maximum: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            loop {
                let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "study-pen-settings-test-{}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("Could not create an isolated test directory: {error}"),
                }
            }
        }

        fn portable(&self) -> PathBuf {
            self.0.join(PORTABLE_SETTINGS_NAME)
        }

        fn fallback(&self) -> PathBuf {
            self.0.join("local-data").join("settings.json")
        }

        fn paired(&self, portable: &Path) -> PathBuf {
            paired_settings_path(portable, &self.fallback()).unwrap()
        }

        fn write(&self, path: &Path, settings: &Settings, timestamp: u64) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, serde_json::to_vec(settings).unwrap()).unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp))
                .unwrap();
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            // Only directories created by this fixture beneath the OS temp root
            // can be removed, even when an assertion aborts the test early.
            if let (Ok(path), Ok(root)) =
                (self.0.canonicalize(), std::env::temp_dir().canonicalize())
            {
                let owned_name = path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with("study-pen-settings-test-")
                });
                if path.starts_with(root) && owned_name {
                    let _ = fs::remove_dir_all(path);
                }
            }
        }
    }

    #[test]
    fn defaults_enable_updates_without_a_repository() {
        let settings = Settings::default();
        assert!(settings.check_for_updates);
        assert!(settings.github_repository.is_none());
        assert!(settings.toolbar_visible);
        assert!(!settings.toolbar_vertical);
        assert!(!settings.desktop_shortcut_seen);
        assert_eq!(Action::ALL.len(), 9);
        assert_eq!(Action::RadialMenu.index(), 8);
        assert!(!settings.hotkeys[Action::RadialMenu.index()].is_bound());
        assert_eq!(
            settings.hotkeys[Action::ToggleUi.index()],
            Hotkey::new(7, 0x76)
        );
        assert_eq!(settings.hotkeys[Action::Quit.index()], Hotkey::new(7, 0x77));
        for action in Action::ALL {
            assert!(
                settings
                    .validate_hotkey(action, settings.hotkeys[action.index()])
                    .is_ok()
            );
        }
    }

    #[test]
    fn missing_fields_in_old_settings_get_defaults() {
        let settings: Settings = serde_json::from_str(r##"{"primary_color":"#102030"}"##).unwrap();
        assert_eq!(settings.primary_color, "#102030");
        assert_eq!(settings.hotkeys, Settings::default().hotkeys);
        assert_eq!(settings.hotkey_default_revision, 0);
        assert!(settings.check_for_updates);
        assert!(!settings.toolbar_vertical);
        assert_eq!(settings.toolbar_theme, ToolbarTheme::LiquidGlass);
        assert!(!settings.toolbar_labels);
        assert_eq!(settings.toolbar_buttons, ToolbarButton::DEFAULT);
    }

    #[test]
    fn damaged_toolbar_fields_preserve_other_preferences_and_known_button_order() {
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "primary_color": "#123456",
            "pen_width": 8.0,
            "check_for_updates": false,
            "toolbar_theme": "future_theme",
            "toolbar_labels": "invalid",
            "toolbar_buttons": ["ruler", "pen", "ruler", "future_button", null, 7, "fill"]
        }))
        .unwrap();
        settings.normalize();
        assert_eq!(settings.primary_color, "#123456");
        assert_eq!(settings.pen_width, 8.0);
        assert!(!settings.check_for_updates);
        assert_eq!(settings.toolbar_theme, ToolbarTheme::LiquidGlass);
        assert!(!settings.toolbar_labels);
        assert_eq!(
            settings.toolbar_buttons,
            [
                ToolbarButton::Ruler,
                ToolbarButton::Pen,
                ToolbarButton::Fill,
                ToolbarButton::Settings,
                ToolbarButton::Quit
            ]
        );
        for damaged in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!(["future"]),
        ] {
            let settings: Settings = serde_json::from_value(serde_json::json!({
                "secondary_color": "#ABCDEF",
                "toolbar_theme": 17,
                "toolbar_buttons": damaged,
            }))
            .unwrap();
            assert_eq!(settings.secondary_color, "#ABCDEF");
            assert_eq!(settings.toolbar_theme, ToolbarTheme::LiquidGlass);
            assert_eq!(settings.toolbar_buttons, ToolbarButton::DEFAULT);
        }
    }

    #[test]
    fn older_toolbar_preferences_append_quit_without_resetting_user_choices() {
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "primary_color": "#123456",
            "toolbar_theme": "liquid_air",
            "toolbar_labels": true,
            "toolbar_buttons": ["ruler", "settings", "fill", "pen"]
        }))
        .unwrap();
        settings.normalize();
        assert_eq!(
            settings.toolbar_buttons,
            [
                ToolbarButton::Ruler,
                ToolbarButton::Settings,
                ToolbarButton::Fill,
                ToolbarButton::Pen,
                ToolbarButton::Quit
            ]
        );
        assert_eq!(settings.primary_color, "#123456");
        assert_eq!(settings.toolbar_theme, ToolbarTheme::LiquidAir);
        assert!(settings.toolbar_labels);
        let serialized = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            serialized["toolbar_buttons"],
            serde_json::json!(["ruler", "settings", "fill", "pen", "quit"])
        );
    }

    #[test]
    fn normalization_rejects_invalid_data_and_shortcut_collisions() {
        let mut settings = Settings {
            primary_color: "not a color".into(),
            secondary_color: "abcdef".into(),
            pen_width: f32::NAN,
            eraser_diameter: 1000.0,
            ..Settings::default()
        };
        settings.hotkeys[1] = settings.hotkeys[0];
        settings.normalize();
        assert_eq!(settings.primary_color, Settings::default().primary_color);
        assert_eq!(settings.secondary_color, "#ABCDEF");
        assert_eq!(settings.pen_width, 3.0);
        assert_eq!(settings.eraser_diameter, 240.0);
        assert_eq!(settings.hotkeys, Settings::default().hotkeys);
    }

    #[test]
    fn hotkeys_have_readable_labels_and_disallow_modifier_only() {
        assert_eq!(Hotkey::new(3, b'D' as u32).display(), "Ctrl + Alt + D");
        assert_eq!(Hotkey::new(0, 0x7b).display(), "F12");
        assert!(!Hotkey::new(2, 0x11).is_valid());
        assert!(!Hotkey::new(0x4000, b'A' as u32).is_valid());
    }

    #[test]
    fn unbound_shortcuts_are_explicit_valid_and_distinct_from_malformed_modifier_only_values() {
        let unbound = Hotkey::unbound();
        assert_eq!(unbound, Hotkey::new(0, 0));
        assert!(!unbound.is_bound());
        assert!(unbound.is_valid());
        assert_eq!(unbound.validation_error(), None);
        assert_eq!(unbound.display(), "Unbound");
        let malformed = Hotkey::new(2, 0);
        assert!(malformed.is_bound());
        assert!(!malformed.is_valid());
    }

    #[test]
    fn multiple_actions_can_be_unbound_without_colliding_or_being_reenabled() {
        let mut settings = Settings::default();
        settings.hotkeys.fill(Hotkey::unbound());
        settings.normalize();
        assert!(settings.hotkeys.iter().all(|hotkey| !hotkey.is_bound()));
        for action in Action::ALL {
            assert!(settings.validate_hotkey(action, Hotkey::unbound()).is_ok());
        }
        assert!(settings.persistence_warning().is_none());
        let value = serde_json::to_value(&settings).unwrap();
        let mut reloaded: Settings = serde_json::from_value(value).unwrap();
        reloaded.normalize();
        assert_eq!(reloaded.hotkeys, settings.hotkeys);
    }

    #[test]
    fn radial_shortcut_is_unbound_by_default_and_can_reuse_a_deliberately_unbound_action() {
        let mut settings = Settings::default();
        let pen = settings.hotkeys[Action::TogglePen.index()];
        assert!(settings.validate_hotkey(Action::RadialMenu, pen).is_err());
        settings.hotkeys[Action::TogglePen.index()] = Hotkey::unbound();
        assert!(settings.validate_hotkey(Action::RadialMenu, pen).is_ok());
        settings.hotkeys[Action::RadialMenu.index()] = pen;
        settings.normalize();
        assert_eq!(settings.hotkeys[Action::RadialMenu.index()], pen);
        assert!(!settings.hotkeys[Action::TogglePen.index()].is_bound());
    }

    #[test]
    fn invalid_or_duplicate_radial_bindings_return_to_unbound_without_claiming_another_key() {
        for invalid in [Hotkey::new(7, 0x7b), Settings::default().hotkeys[0]] {
            let mut settings = Settings::default();
            settings.hotkeys[Action::RadialMenu.index()] = invalid;
            settings.normalize();
            assert_eq!(settings.hotkeys, Settings::default().hotkeys);
        }
    }

    #[test]
    fn eight_entry_custom_preferences_gain_only_an_unbound_radial_action() {
        let mut old = Settings::default().hotkeys[..8].to_vec();
        old[0] = Hotkey::new(0, 0x75);
        old[2] = Hotkey::unbound();
        let value = serde_json::json!({
            "primary_color": "#123456",
            "hotkeys": old,
            "hotkey_default_revision": HOTKEY_DEFAULT_REVISION,
        });
        let mut settings: Settings = serde_json::from_value(value).unwrap();
        settings.normalize();
        assert_eq!(&settings.hotkeys[..8], old.as_slice());
        assert_eq!(
            settings.hotkeys[Action::RadialMenu.index()],
            Hotkey::unbound()
        );
        assert_eq!(settings.primary_color, "#123456");
        assert!(settings.persistence_warning().is_none());
    }

    #[test]
    fn six_entry_custom_unbinding_does_not_disable_new_toolbar_or_quit_defaults() {
        let mut old = Settings::default().hotkeys[..6].to_vec();
        old[0] = Hotkey::unbound();
        let mut settings: Settings =
            serde_json::from_value(serde_json::json!({ "hotkeys": old })).unwrap();
        settings.normalize();
        assert_eq!(&settings.hotkeys[..6], old.as_slice());
        assert_eq!(settings.hotkeys[6..8], Settings::default().hotkeys[6..8]);
        assert_eq!(
            settings.hotkeys[Action::RadialMenu.index()],
            Hotkey::unbound()
        );
    }

    #[test]
    fn fresh_default_shortcuts_use_modified_function_keys_without_character_or_system_bindings() {
        let settings = Settings::default();
        assert_eq!(settings.hotkey_default_revision, HOTKEY_DEFAULT_REVISION);
        for (index, hotkey) in settings.hotkeys[..8].iter().enumerate() {
            assert_eq!(hotkey.modifiers, 7);
            assert_eq!(hotkey.key, 0x70 + index as u32);
            assert!(hotkey.validation_error().is_none());
        }
        assert_eq!(
            settings.hotkeys[Action::RadialMenu.index()],
            Hotkey::unbound()
        );
    }

    #[test]
    fn altgr_character_combinations_are_rejected_with_or_without_shift() {
        for modifiers in [3, 7] {
            for key in [
                b'Q' as u32,
                b'E' as u32,
                b'D' as u32,
                b'1' as u32,
                0x60,
                0xbb,
                0xdb,
            ] {
                let hotkey = Hotkey::new(modifiers, key);
                assert!(hotkey.validation_error().unwrap().contains("AltGr"));
                assert!(!hotkey.is_valid());
            }
        }
    }

    #[test]
    fn common_windows_combinations_and_unsafe_typing_keys_are_rejected() {
        let reserved = [
            Hotkey::new(8, b'D' as u32),
            Hotkey::new(15, 0x75),
            Hotkey::new(0, 0x7b),
            Hotkey::new(2, 0x7b),
            Hotkey::new(7, 0x7b),
            Hotkey::new(0, 0x1b),
            Hotkey::new(2, 0x1b),
            Hotkey::new(6, 0x1b),
            Hotkey::new(3, 0x2e),
            Hotkey::new(7, 0x2e),
            Hotkey::new(1, 0x09),
            Hotkey::new(3, 0x09),
            Hotkey::new(7, 0x09),
            Hotkey::new(1, 0x20),
            Hotkey::new(5, 0x20),
            Hotkey::new(1, 0x73),
            Hotkey::new(5, 0x73),
            Hotkey::new(0, 0x79),
            Hotkey::new(4, 0x79),
            Hotkey::new(0, b'A' as u32),
            Hotkey::new(1, b'A' as u32),
            Hotkey::new(0, b'3' as u32),
            Hotkey::new(1, 0xba),
        ];
        for hotkey in reserved {
            assert!(hotkey.validation_error().is_some(), "{}", hotkey.display());
            assert!(!hotkey.is_valid());
        }
        let settings = Settings::default();
        assert_eq!(
            settings
                .validate_hotkey(Action::TogglePen, Hotkey::new(7, 0x7b))
                .unwrap_err(),
            "F12 is reserved by Windows for debugging."
        );
    }

    #[test]
    fn valid_custom_function_keys_remain_available_including_modified_f4() {
        for hotkey in [
            Hotkey::new(0, 0x75),
            Hotkey::new(4, 0x75),
            Hotkey::new(2, 0x75),
            Hotkey::new(7, 0x73),
            Hotkey::new(3, 0x78),
            Hotkey::new(0, 0x7c),
            Hotkey::new(6, b'D' as u32),
        ] {
            assert!(hotkey.validation_error().is_none(), "{}", hotkey.display());
        }
    }

    #[test]
    fn shifted_printable_shortcuts_preserve_normal_uppercase_and_symbol_typing() {
        for hotkey in [
            Hotkey::new(4, b'A' as u32),
            Hotkey::new(4, 0xbb),
            Hotkey::new(5, b'A' as u32),
        ] {
            assert_eq!(
                hotkey.validation_error(),
                Some("Include Ctrl with a character key to protect normal typing.")
            );
        }
        assert!(Hotkey::new(6, b'K' as u32).is_valid());
        assert!(Hotkey::new(0, 0x75).is_valid());
    }

    #[test]
    fn exact_legacy_default_arrays_migrate_in_both_six_and_eight_entry_formats() {
        for count in [6, 8] {
            let legacy = serde_json::json!({
                "primary_color": "#123456",
                "secondary_color": "#654321",
                "hotkeys": &legacy_hotkeys()[..count],
                "toolbar_visible": false,
            });
            let mut settings: Settings = serde_json::from_value(legacy).unwrap();
            assert_eq!(settings.hotkey_default_revision, 0);
            settings.normalize();
            assert_eq!(settings.hotkeys, Settings::default().hotkeys);
            assert_eq!(settings.hotkey_default_revision, HOTKEY_DEFAULT_REVISION);
            assert_eq!(settings.primary_color, "#123456");
            assert_eq!(settings.secondary_color, "#654321");
            assert!(!settings.toolbar_visible);
            assert!(settings.persistence_warning().is_some());
        }
    }

    #[test]
    fn revision_one_and_old_eight_entry_custom_sets_keep_valid_user_changes() {
        for revision in [0, HOTKEY_DEFAULT_REVISION] {
            let mut custom = Settings::default().hotkeys;
            custom[0] = Hotkey::new(0, 0x75);
            custom[3] = Hotkey::new(6, b'K' as u32);
            let value = serde_json::json!({
                "hotkeys": custom,
                "hotkey_default_revision": revision,
            });
            let mut settings: Settings = serde_json::from_value(value).unwrap();
            settings.normalize();
            assert_eq!(settings.hotkeys, custom);
            assert!(settings.persistence_warning().is_none());
            assert_eq!(settings.hotkey_default_revision, HOTKEY_DEFAULT_REVISION);
        }
    }

    #[test]
    fn unsafe_custom_assignments_are_repaired_without_resetting_other_user_choices() {
        let mut settings = Settings::default();
        settings.hotkeys[0] = Hotkey::new(0, 0x75);
        settings.hotkeys[1] = Hotkey::new(7, 0x7b);
        settings.hotkeys[2] = Hotkey::new(7, b'Q' as u32);
        settings.hotkeys[3] = Hotkey::new(6, b'K' as u32);
        settings.normalize();
        assert_eq!(settings.hotkeys[0], Hotkey::new(0, 0x75));
        assert_eq!(settings.hotkeys[3], Hotkey::new(6, b'K' as u32));
        for action in Action::ALL {
            assert!(
                settings
                    .validate_hotkey(action, settings.hotkeys[action.index()])
                    .is_ok()
            );
        }
        assert!(settings.persistence_warning().is_some());
    }

    #[test]
    fn six_shortcut_migration_preserves_custom_assignments_and_other_preferences() {
        let old_hotkeys = [
            Hotkey::new(6, 0x75),
            Hotkey::new(2, b'Y' as u32),
            Hotkey::new(3, b'O' as u32),
            Hotkey::new(7, 0x79),
            Hotkey::new(0, 0x75),
            Hotkey::new(4, 0x77),
        ];
        let legacy = serde_json::json!({
            "primary_color": "#112233",
            "secondary_color": "#AABBCC",
            "pen_width": 7.0,
            "eraser_diameter": 75.0,
            "hotkeys": old_hotkeys,
            "check_for_updates": false,
        });
        let mut settings: Settings = serde_json::from_value(legacy).unwrap();
        settings.normalize();
        for index in [0, 1, 3, 4, 5] {
            assert_eq!(settings.hotkeys[index], old_hotkeys[index]);
        }
        assert_eq!(settings.hotkeys[2], Settings::default().hotkeys[2]);
        assert_eq!(settings.hotkeys[6..], Settings::default().hotkeys[6..]);
        assert_eq!(settings.hotkey_default_revision, HOTKEY_DEFAULT_REVISION);
        assert!(
            settings
                .persistence_warning()
                .unwrap()
                .contains("shortcuts")
        );
        for action in Action::ALL {
            assert!(
                settings
                    .validate_hotkey(action, settings.hotkeys[action.index()])
                    .is_ok()
            );
        }
        assert_eq!(settings.primary_color, "#112233");
        assert_eq!(settings.secondary_color, "#AABBCC");
        assert_eq!(settings.pen_width, 7.0);
        assert_eq!(settings.eraser_diameter, 75.0);
        assert!(!settings.check_for_updates);
        assert!(settings.toolbar_visible);
    }

    #[test]
    fn repairing_an_invalid_shortcut_does_not_steal_a_later_custom_assignment() {
        let mut settings = Settings::default();
        settings.hotkeys[0] = MISSING_HOTKEY;
        settings.hotkeys[1] = Hotkey::new(7, 0x70);
        settings.normalize();
        assert_eq!(settings.hotkeys[1], Hotkey::new(7, 0x70));
        assert_ne!(settings.hotkeys[0], settings.hotkeys[1]);
    }

    #[test]
    fn portable_settings_round_trip_every_preference_without_storing_paths() {
        let directory = TestDirectory::new();
        let portable = directory.portable();
        let fallback = directory.fallback();
        let mut settings = Settings::load_from_paths(portable.clone(), Some(fallback.clone()));
        settings.primary_color = "#102030".into();
        settings.secondary_color = "#708090".into();
        settings.pen_width = 9.0;
        settings.eraser_diameter = 84.0;
        settings.hotkeys[0] = Hotkey::new(4, 0x7a);
        settings.hotkeys[Action::RadialMenu.index()] = Hotkey::new(6, b'R' as u32);
        settings.check_for_updates = false;
        settings.github_repository = Some("example/study-pen".into());
        settings.toolbar_position = Some((-120, 240));
        settings.toolbar_visible = false;
        settings.toolbar_vertical = true;
        settings.desktop_shortcut_seen = true;
        settings.toolbar_theme = ToolbarTheme::LiquidAir;
        settings.toolbar_labels = true;
        settings.toolbar_buttons = vec![
            ToolbarButton::Fill,
            ToolbarButton::Settings,
            ToolbarButton::Ruler,
            ToolbarButton::Pen,
            ToolbarButton::Quit,
        ];
        settings.save().unwrap();
        settings.primary_color = "#203040".into();
        settings.save().unwrap();
        assert_eq!(settings.persistence_location(), Some(portable.clone()));
        assert!(settings.persistence_warning().is_none());
        assert!(!fallback.exists());
        let reloaded = Settings::load_from_paths(portable.clone(), Some(fallback));
        assert_eq!(reloaded.primary_color, "#203040");
        assert_eq!(reloaded.secondary_color, "#708090");
        assert_eq!(reloaded.pen_width, 9.0);
        assert_eq!(reloaded.eraser_diameter, 84.0);
        assert_eq!(reloaded.hotkeys, settings.hotkeys);
        assert_eq!(reloaded.github_repository, settings.github_repository);
        assert_eq!(reloaded.toolbar_position, Some((-120, 240)));
        assert!(!reloaded.check_for_updates);
        assert!(!reloaded.toolbar_visible);
        assert!(reloaded.toolbar_vertical);
        assert!(reloaded.desktop_shortcut_seen);
        assert_eq!(reloaded.toolbar_theme, ToolbarTheme::LiquidAir);
        assert!(reloaded.toolbar_labels);
        assert_eq!(reloaded.toolbar_buttons, settings.toolbar_buttons);
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&portable).unwrap()).unwrap();
        assert_eq!(json["hotkeys"].as_array().unwrap().len(), 9);
        assert_eq!(json["hotkey_default_revision"], HOTKEY_DEFAULT_REVISION);
        assert_eq!(json["toolbar_vertical"], true);
        assert_eq!(json["toolbar_theme"], "liquid_air");
        assert_eq!(json["toolbar_labels"], true);
        assert_eq!(
            json["toolbar_buttons"],
            serde_json::json!(["fill", "settings", "ruler", "pen", "quit"])
        );
        assert!(json.get("storage").is_none());
        assert!(json.get("hotkey_notice").is_none());
        assert!(suffixed_path(&portable, ".bak").exists());
        assert!(fs::read_dir(&directory.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")
        }));
    }

    #[test]
    fn legacy_preferences_migrate_once_and_newer_portable_changes_win() {
        let directory = TestDirectory::new();
        let portable = directory.portable();
        let fallback = directory.fallback();
        let legacy = Settings {
            primary_color: "#101010".into(),
            ..Settings::default()
        };
        directory.write(&fallback, &legacy, 1_000_000);
        let mut settings = Settings::load_from_paths(portable.clone(), Some(fallback.clone()));
        assert_eq!(settings.primary_color, "#101010");
        assert_eq!(settings.persistence_location(), Some(portable.clone()));
        assert!(portable.exists());
        settings.primary_color = "#202020".into();
        settings.save().unwrap();
        let reloaded = Settings::load_from_paths(portable.clone(), Some(fallback));
        assert_eq!(reloaded.primary_color, "#202020");
        assert_eq!(reloaded.persistence_location(), Some(portable));
    }

    #[test]
    fn valid_portable_preferences_win_even_when_shared_fallback_is_newer() {
        let directory = TestDirectory::new();
        let portable = directory.portable();
        let fallback = directory.fallback();
        directory.write(&portable, &Settings::default(), 1_000_000);
        let newer = Settings {
            secondary_color: "#123456".into(),
            ..Settings::default()
        };
        directory.write(&fallback, &newer, 2_000_000);
        let settings = Settings::load_from_paths(portable.clone(), Some(fallback));
        assert_eq!(
            settings.secondary_color,
            Settings::default().secondary_color
        );
        assert_eq!(settings.persistence_location(), Some(portable.clone()));
        let disk: Settings = serde_json::from_slice(&fs::read(portable).unwrap()).unwrap();
        assert_eq!(disk.secondary_color, Settings::default().secondary_color);
    }

    #[test]
    fn unwritable_portable_location_uses_visible_fallback_and_can_recover() {
        let directory = TestDirectory::new();
        let blocker = directory.0.join("blocked");
        fs::write(&blocker, b"not a directory").unwrap();
        let portable = blocker.join(PORTABLE_SETTINGS_NAME);
        let fallback = directory.fallback();
        let paired = directory.paired(&portable);
        let mut settings = Settings::load_from_paths(portable.clone(), Some(fallback.clone()));
        settings.primary_color = "#ABCDEF".into();
        settings.save().unwrap();
        assert_eq!(settings.persistence_location(), Some(paired));
        assert!(settings.persistence_warning().is_some());
        assert!(
            !settings
                .persistence_description()
                .contains(&directory.0.to_string_lossy().to_string())
        );
        let reloaded = Settings::load_from_paths(portable.clone(), Some(fallback));
        assert_eq!(reloaded.primary_color, "#ABCDEF");
        fs::remove_file(&blocker).unwrap();
        settings.save().unwrap();
        assert_eq!(settings.persistence_location(), Some(portable));
        assert!(settings.persistence_warning().is_none());
    }

    #[test]
    fn complete_persistence_failure_returns_an_error_instead_of_claiming_success() {
        let directory = TestDirectory::new();
        let blocker = directory.0.join("blocked");
        fs::write(&blocker, b"not a directory").unwrap();
        let settings = Settings::load_from_paths(blocker.join(PORTABLE_SETTINGS_NAME), None);
        assert!(settings.save().is_err());
        assert!(settings.persistence_location().is_none());
        assert!(settings.persistence_warning().is_some());
    }

    #[test]
    fn corrupt_settings_recover_backup_and_preserve_the_unreadable_file() {
        let directory = TestDirectory::new();
        let portable = directory.portable();
        let mut settings = Settings::load_from_paths(portable.clone(), None);
        settings.primary_color = "#123456".into();
        settings.save().unwrap();
        settings.primary_color = "#654321".into();
        settings.save().unwrap();
        fs::write(&portable, b"{unfinished").unwrap();
        let recovered = Settings::load_from_paths(portable.clone(), None);
        assert_eq!(recovered.primary_color, "#123456");
        assert!(
            recovered
                .persistence_warning()
                .unwrap()
                .contains("recovered")
        );
        assert_eq!(
            fs::read(suffixed_path(&portable, ".corrupt-0")).unwrap(),
            b"{unfinished"
        );
        assert!(serde_json::from_slice::<Settings>(&fs::read(portable).unwrap()).is_ok());
    }

    #[test]
    fn defaults_and_unit_test_runtime_loading_never_write_beside_test_executables() {
        let settings = Settings::default();
        assert!(settings.save().is_err());
        assert!(settings.persistence_location().is_none());
        assert!(Settings::load().persistence_location().is_none());
    }

    #[test]
    fn same_copy_paired_preferences_restore_and_migrate_back_to_writable_portable_storage() {
        let directory = TestDirectory::new();
        let portable = directory.portable();
        let paired = directory.paired(&portable);
        directory.write(&portable, &Settings::default(), 1_000_000);
        let newer = Settings {
            primary_color: "#123456".into(),
            toolbar_visible: false,
            ..Settings::default()
        };
        directory.write(&paired, &newer, 2_000_000);
        let settings = Settings::load_from_paths(portable.clone(), Some(directory.fallback()));
        assert_eq!(settings.primary_color, "#123456");
        assert!(!settings.toolbar_visible);
        assert_eq!(settings.persistence_location(), Some(portable.clone()));
        let reloaded = Settings::load_from_paths(portable.clone(), Some(directory.fallback()));
        assert_eq!(reloaded.primary_color, "#123456");
        assert_eq!(reloaded.persistence_location(), Some(portable));
        assert!(reloaded.persistence_warning().is_none());
    }

    #[test]
    fn paired_fallbacks_are_isolated_between_portable_copies() {
        let directory = TestDirectory::new();
        let first = directory.0.join("first").join(PORTABLE_SETTINGS_NAME);
        let second = directory.0.join("second").join(PORTABLE_SETTINGS_NAME);
        let first_paired = directory.paired(&first);
        let second_paired = directory.paired(&second);
        assert_ne!(first_paired, second_paired);
        directory.write(&first, &Settings::default(), 1_000_000);
        directory.write(&second, &Settings::default(), 1_000_000);
        let custom = Settings {
            primary_color: "#010203".into(),
            ..Settings::default()
        };
        directory.write(&first_paired, &custom, 2_000_000);
        directory.write(&directory.fallback(), &custom, 3_000_000);
        let first_loaded = Settings::load_from_paths(first, Some(directory.fallback()));
        let second_loaded = Settings::load_from_paths(second, Some(directory.fallback()));
        assert_eq!(first_loaded.primary_color, "#010203");
        assert_eq!(
            second_loaded.primary_color,
            Settings::default().primary_color
        );
        assert!(!second_paired.exists());
    }

    #[test]
    fn windows_fallback_namespace_is_stable_across_case_slashes_and_dot_segments() {
        let legacy = Path::new(r"C:\LocalData\StudyPen\settings.json");
        let first = Path::new(r"C:\Tools\Study Pen\.\unused\..\study-pen.settings.json");
        let second = Path::new("c:/tools/study pen/study-pen.settings.json");
        let extended = Path::new(r"\\?\C:\Tools\Study Pen\study-pen.settings.json");
        assert_eq!(
            paired_settings_path(first, legacy),
            paired_settings_path(second, legacy)
        );
        assert_eq!(
            paired_settings_path(first, legacy),
            paired_settings_path(extended, legacy)
        );
    }

    #[cfg(windows)]
    #[test]
    fn readonly_existing_portable_settings_keep_new_preferences_after_restart() {
        struct RestoreWritable(PathBuf, fs::Permissions);
        impl Drop for RestoreWritable {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, self.1.clone());
            }
        }
        let directory = TestDirectory::new();
        let portable = directory.portable();
        let paired = directory.paired(&portable);
        directory.write(&portable, &Settings::default(), 1_000_000);
        let mut settings = Settings::load_from_paths(portable.clone(), Some(directory.fallback()));
        let mut permissions = fs::metadata(&portable).unwrap().permissions();
        let restore = RestoreWritable(portable.clone(), permissions.clone());
        permissions.set_readonly(true);
        fs::set_permissions(&portable, permissions).unwrap();
        settings.primary_color = "#030405".into();
        settings.save().unwrap();
        assert_eq!(settings.persistence_location(), Some(paired.clone()));
        assert!(settings.persistence_warning().is_some());
        let reloaded = Settings::load_from_paths(portable.clone(), Some(directory.fallback()));
        assert_eq!(reloaded.primary_color, "#030405");
        assert_eq!(reloaded.persistence_location(), Some(paired));
        drop(restore);
        reloaded.save().unwrap();
        assert_eq!(reloaded.persistence_location(), Some(portable.clone()));
        assert!(reloaded.persistence_warning().is_none());
        let migrated = Settings::load_from_paths(portable.clone(), Some(directory.fallback()));
        assert_eq!(migrated.primary_color, "#030405");
        assert_eq!(migrated.persistence_location(), Some(portable));
    }
}
