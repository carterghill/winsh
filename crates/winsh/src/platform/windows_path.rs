//! Keep installer additions visible in the shell that launched the installer.
#![allow(unsafe_code)]

use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
    ptr,
};
use windows_sys::{
    Win32::{
        Security::{TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY},
        System::{
            Environment::ExpandEnvironmentStringsForUserW,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    w,
};

use crate::Shell;

pub(crate) fn registered() -> Option<OsString> {
    let mut token = ptr::null_mut();
    // SAFETY: token is a writable output handle for the current process.
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
            &mut token,
        )
    } == 0
    {
        return None;
    }
    // SAFETY: OpenProcessToken returned a new owned handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    // Windows limits one environment variable to 32,767 UTF-16 units.
    let mut value = vec![0; 32768];
    // SAFETY: the token stays open and the buffer length is in UTF-16 units.
    if unsafe {
        ExpandEnvironmentStringsForUserW(
            token.as_raw_handle(),
            w!("%PATH%"),
            value.as_mut_ptr(),
            value.len() as u32,
        )
    } == 0
    {
        return None;
    }
    let end = value.iter().position(|&c| c == 0)?;
    Some(OsString::from_wide(&value[..end]))
}

pub(crate) fn refresh(shell: &mut Shell, before: Option<&OsStr>) {
    if let (Some(path), Some(registered)) = (shell.var("PATH"), registered())
        && let Some(merged) = append_additions(path, before, &registered)
    {
        shell.set_var("PATH", merged);
    }
}

fn append_additions(path: &OsStr, before: Option<&OsStr>, registered: &OsStr) -> Option<OsString> {
    // Existing session entries keep their order. After an install, also skip
    // previously registered directories the user omitted from the session.
    let mut seen: HashSet<_> = env::split_paths(path)
        .chain(before.into_iter().flat_map(env::split_paths))
        .map(|dir| PathBuf::from(dir.as_os_str().to_ascii_lowercase()))
        .collect();
    let mut result = path.to_owned();
    for dir in env::split_paths(registered).filter(|p| p.is_absolute()) {
        if !seen.insert(PathBuf::from(dir.as_os_str().to_ascii_lowercase())) {
            continue;
        }
        let entry = env::join_paths([&dir]).ok()?;
        result.push(";");
        result.push(entry);
    }
    (result != path).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_registered_paths_at_startup_and_after_install() {
        let session = OsStr::new(r"C:\session;D:\tools");
        let before = OsStr::new(r"d:/TOOLS/;C:\old");
        let after = OsStr::new(r#"d:/TOOLS/;C:\old;;relative;"C:\new;tools""#);
        assert_eq!(
            append_additions(session, None, before).unwrap(),
            r"C:\session;D:\tools;C:\old"
        );
        let merged = append_additions(session, Some(before), after).unwrap();
        assert_eq!(merged, r#"C:\session;D:\tools;"C:\new;tools""#);
        assert_eq!(append_additions(&merged, Some(before), after), None);
    }
}
