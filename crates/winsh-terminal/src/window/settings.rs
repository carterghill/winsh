use serde::Deserialize;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
    },
    System::Threading::WaitForSingleObject,
};

const STARTER: &str = include_str!("../../assets/settings.json");

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct FontSettings {
    pub font_family: String,
    pub font_size_px: f32,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            font_family: "Cascadia Mono".into(),
            font_size_px: 14.0,
        }
    }
}

#[derive(Deserialize)]
struct Settings {
    #[serde(default)]
    terminal: FontSettings,
}

fn read(path: &Path) -> io::Result<FontSettings> {
    let json = match fs::read_to_string(path) {
        Ok(json) => json,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(FontSettings::default()),
        Err(error) => return Err(error),
    };
    let Settings { terminal } = serde_json::from_str(json.trim_start_matches('\u{feff}'))?;
    // GDI's LF_FACESIZE includes the terminating UTF-16 NUL.
    if terminal.font_family.trim().is_empty()
        || terminal.font_family.contains('\0')
        || terminal.font_family.encode_utf16().count() >= 32
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "font_family must be a Windows font name of 1 to 31 UTF-16 units",
        ));
    }
    if !(1.0..=256.0).contains(&terminal.font_size_px) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "font_size_px must be between 1 and 256",
        ));
    }
    Ok(terminal)
}

fn seed(path: &Path) -> io::Result<()> {
    if path.try_exists()? {
        return Ok(());
    }
    let directory = path.parent().expect("settings.json has a config directory");
    fs::create_dir_all(directory)?;
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    temp.write_all(STARTER.as_bytes())?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error),
    }
}

pub(super) struct SettingsFile {
    path: PathBuf,
    changes: Option<DirectoryChanges>,
    next_check: Instant,
    pub current: Result<FontSettings, String>,
}

impl SettingsFile {
    pub fn open(path: PathBuf) -> Self {
        let seeded = seed(&path);
        // Arm the directory notification before reading to also catch save-by-rename.
        let changes =
            DirectoryChanges::open(path.parent().expect("settings.json has a config directory"));
        let current = seeded
            .and_then(|()| read(&path))
            .map_err(|err| format!("{}\n\n{err}", path.display()));
        Self {
            path,
            changes,
            next_check: Instant::now(),
            current,
        }
    }

    pub fn poll(&mut self) -> Option<Result<FontSettings, String>> {
        let now = Instant::now();
        if now < self.next_check {
            return None;
        }
        self.next_check = now + Duration::from_millis(250);
        match self.changes.as_ref().and_then(DirectoryChanges::changed) {
            Some(false) if self.current.is_ok() => return None,
            // An editor may still have an exclusive handle when its save
            // notification arrives. Retry even if there is no further event.
            Some(false) => self.next_check = now + Duration::from_secs(1),
            Some(true) => {}
            None => {
                // Some roaming filesystems do not provide notifications. Reopen
                // by path as well if the original directory handle became invalid.
                self.changes = DirectoryChanges::open(self.path.parent()?);
                self.next_check = now + Duration::from_secs(1);
            }
        }
        let current = read(&self.path).map_err(|err| format!("{}\n\n{err}", self.path.display()));
        if current == self.current {
            return None;
        }
        self.current = current.clone();
        Some(current)
    }
}

struct DirectoryChanges(HANDLE);

impl DirectoryChanges {
    fn open(directory: &Path) -> Option<Self> {
        use std::os::windows::ffi::OsStrExt;
        let path: Vec<_> = directory.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: path is NUL-terminated and the returned handle is owned below.
        let handle = unsafe {
            FindFirstChangeNotificationW(
                path.as_ptr(),
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(handle))
        }
    }

    fn changed(&self) -> Option<bool> {
        // SAFETY: the handle is live and the zero timeout never blocks the window.
        unsafe {
            match WaitForSingleObject(self.0, 0) {
                WAIT_TIMEOUT => Some(false),
                WAIT_OBJECT_0 if FindNextChangeNotification(self.0) != 0 => Some(true),
                _ => None,
            }
        }
    }
}

impl Drop for DirectoryChanges {
    fn drop(&mut self) {
        // SAFETY: release this notification handle exactly once.
        unsafe {
            FindCloseChangeNotification(self.0);
        }
    }
}
