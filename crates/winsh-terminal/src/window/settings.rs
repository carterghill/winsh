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

pub(super) trait Config: Default + Clone + PartialEq {
    const STARTER: &'static str;
    fn parse(json: &str) -> io::Result<Self>;
}

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
    terminal: TerminalSettings,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct TerminalSettings {
    #[serde(flatten)]
    pub font: FontSettings,
    pub new_tab_directory: NewTabDirectory,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(super) enum NewTabDirectory {
    #[default]
    Current,
    Home,
    Custom {
        path: PathBuf,
    },
}

impl Config for TerminalSettings {
    const STARTER: &'static str = include_str!("../../assets/settings.json");

    fn parse(json: &str) -> io::Result<Self> {
        let Settings { terminal } = serde_json::from_str(json)?;
        // GDI's LF_FACESIZE includes the terminating UTF-16 NUL.
        if terminal.font.font_family.trim().is_empty()
            || terminal.font.font_family.contains('\0')
            || terminal.font.font_family.encode_utf16().count() >= 32
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "font_family must be a Windows font name of 1 to 31 UTF-16 units",
            ));
        }
        if !(1.0..=256.0).contains(&terminal.font.font_size_px) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "font_size_px must be between 1 and 256",
            ));
        }
        if let NewTabDirectory::Custom { path } = &terminal.new_tab_directory
            && (!path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&0))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "new_tab_directory.path must be an absolute folder path",
            ));
        }
        Ok(terminal)
    }
}

fn read<T: Config>(path: &Path) -> io::Result<T> {
    match fs::read_to_string(path) {
        Ok(json) => T::parse(json.trim_start_matches('\u{feff}')),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error),
    }
}

fn seed(path: &Path, starter: &str) -> io::Result<()> {
    if path.try_exists()? {
        return Ok(());
    }
    let directory = path.parent().expect("config file has a parent directory");
    fs::create_dir_all(directory)?;
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    temp.write_all(starter.as_bytes())?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error),
    }
}

pub(super) struct ConfigFile<T> {
    path: PathBuf,
    changes: Option<DirectoryChanges>,
    next_check: Instant,
    pub current: Result<T, String>,
}

impl<T: Config> ConfigFile<T> {
    pub fn open(path: PathBuf) -> Self {
        let seeded = seed(&path, T::STARTER);
        // Arm the directory notification before reading to also catch save-by-rename.
        let changes =
            DirectoryChanges::open(path.parent().expect("config file has a parent directory"));
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

    pub fn poll(&mut self) -> Option<Result<T, String>> {
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
