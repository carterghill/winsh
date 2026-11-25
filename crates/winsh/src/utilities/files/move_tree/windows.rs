use super::*;
use std::os::windows::{
    fs::{MetadataExt, OpenOptionsExt},
    io::AsRawHandle,
};
use std::{ffi::OsString, os::windows::ffi::OsStringExt};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::{
    Foundation::{
        ERROR_HANDLE_EOF, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_FUNCTION,
        ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_NOT_SUPPORTED,
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::{FSCTL_GET_REPARSE_POINT, FSCTL_SET_REPARSE_POINT},
        SystemServices::{FILE_NAMED_STREAMS, IO_REPARSE_TAG_MOUNT_POINT},
    },
};

#[allow(unsafe_code)]
pub(super) fn file_id(
    file: &File,
    legacy: &winapi_util::file::Information,
) -> io::Result<(u64, u128)> {
    // ReFS file identifiers are 128 bits. Truncating them could alias two
    // different files while rebuilding a hard-link group.
    let mut info = FILE_ID_INFO::default();
    // SAFETY: the live handle and sized output match the FileIdInfo class.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut info).cast(),
            size_of_val(&info) as u32,
        )
    } != 0
    {
        return Ok((
            info.VolumeSerialNumber,
            u128::from_le_bytes(info.FileId.Identifier),
        ));
    }
    let error = io::Error::last_os_error();
    if matches!(
        error.raw_os_error().map(|e| e as u32),
        Some(ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED | ERROR_INVALID_PARAMETER)
    ) {
        Ok((legacy.volume_serial_number(), legacy.file_index().into()))
    } else {
        Err(error)
    }
}

#[allow(unsafe_code)]
pub(super) fn named_streams(file: &File) -> io::Result<Vec<(OsString, u64)>> {
    // FILE_STREAM_INFO is a chain of 8-byte-aligned variable-length records.
    let mut buffer = vec![0u64; 64];
    loop {
        let size = u32::try_from(size_of_val(&buffer[..])).map_err(io::Error::other)?;
        // SAFETY: the handle is live and buffer is aligned and writable for size bytes.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileStreamInfo,
                buffer.as_mut_ptr().cast(),
                size,
            )
        } != 0
        {
            break;
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|e| e as u32) {
            Some(ERROR_HANDLE_EOF) => return Ok(Vec::new()),
            Some(ERROR_INSUFFICIENT_BUFFER | ERROR_MORE_DATA) => buffer.resize(buffer.len() * 2, 0),
            Some(ERROR_INVALID_FUNCTION | ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED) => {
                let mut flags = 0;
                // SAFETY: all optional outputs except the flags are null.
                if unsafe {
                    GetVolumeInformationByHandleW(
                        file.as_raw_handle(),
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &mut flags,
                        std::ptr::null_mut(),
                        0,
                    )
                } != 0
                    && flags & FILE_NAMED_STREAMS == 0
                {
                    return Ok(Vec::new());
                }
                return Err(error);
            }
            _ => return Err(error),
        }
    }
    let mut result = Vec::new();
    // SAFETY: the initialized integer buffer can be viewed as bytes.
    let mut data = unsafe {
        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), size_of_val(&buffer[..]))
    };
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid filesystem stream record",
        )
    };
    loop {
        const HEADER: usize = std::mem::offset_of!(FILE_STREAM_INFO, StreamName);
        if data.len() < HEADER {
            return Err(invalid());
        }
        let next = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let size = u64::from_le_bytes(data[8..16].try_into().unwrap());
        let name = data[HEADER..].get(..len).ok_or_else(invalid)?;
        let (units, remainder) = name.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(invalid());
        }
        let units: Vec<_> = units.iter().copied().map(u16::from_le_bytes).collect();
        let name = OsString::from_wide(&units);
        if len != 0 && name != "::$DATA" {
            let bytes = name.as_encoded_bytes();
            if !bytes.starts_with(b":")
                || !bytes.ends_with(b":$DATA")
                || bytes.iter().any(|b| matches!(b, b'/' | b'\\' | 0))
            {
                return Err(invalid());
            }
            result.push((name, size));
        }
        if next == 0 {
            break;
        }
        if next < HEADER + len {
            return Err(invalid());
        }
        data = data.get(next..).ok_or_else(invalid)?;
    }
    result.sort_unstable();
    Ok(result)
}

pub(super) fn transfer_streams(entry: &Entry, dest: &Path) -> io::Result<()> {
    for (stream, expected_size) in &entry.streams {
        let mut target = dest.as_os_str().to_owned();
        target.push(stream);
        let actual = if entry.meta.is_dir() {
            let mut source = entry.path.as_os_str().to_owned();
            source.push(stream);
            io::copy(&mut File::open(source)?, &mut File::create_new(target)?)?
        } else {
            // CopyFileEx copied file streams. Check the destination filesystem
            // actually retained each one before removing the original.
            File::open(target)?.metadata()?.len()
        };
        if actual != *expected_size {
            return Err(io::Error::other("incomplete named data stream"));
        }
    }
    Ok(())
}

#[allow(unsafe_code)]
pub(super) fn copy_reparse_link(entry: &Entry, dest: &Path) -> io::Result<()> {
    if !entry.streams.is_empty() {
        return Err(io::Error::other(
            "cannot recreate named data streams on a link",
        ));
    }
    let file = open_entry(&entry.path, false)?;
    let mut data = vec![0u8; MAXIMUM_REPARSE_DATA_BUFFER_SIZE as usize];
    let mut used = 0;
    // SAFETY: both ioctl calls use synchronous, live handles and byte buffers
    // of the indicated lengths. Only a mount-point buffer is passed to SET.
    if unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_GET_REPARSE_POINT,
            std::ptr::null(),
            0,
            data.as_mut_ptr().cast(),
            data.len() as u32,
            &mut used,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if !(8..=MAXIMUM_REPARSE_DATA_BUFFER_SIZE).contains(&used) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    if u32::from_le_bytes(data[..4].try_into().unwrap()) != IO_REPARSE_TAG_MOUNT_POINT {
        return copy_link(&entry.path, dest, &entry.meta);
    }
    fs::create_dir(dest)?;
    let target = File::options()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(dest)?;
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            target.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            data.as_ptr().cast(),
            used,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn open_entry(path: &Path, deleting: bool) -> io::Result<File> {
    let mut options = File::options();
    options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    if deleting {
        options
            .access_mode(FILE_READ_ATTRIBUTES | DELETE)
            .share_mode(FILE_SHARE_READ);
    } else {
        options.access_mode(FILE_READ_ATTRIBUTES);
    }
    options.open(path)
}

pub(super) fn is_reparse_point(meta: &Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub(super) fn open_attributes(path: &Path) -> io::Result<File> {
    File::options()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[allow(unsafe_code)]
pub(super) fn unlink(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: the live handle has DELETE access, and the selected information
    // class matches the initialized buffer and its length.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const info).cast(),
            size_of_val(&info) as u32,
        )
    } != 0
    {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // Older filesystems can support only the classic disposition request.
    if !matches!(
        error.raw_os_error().map(|e| e as u32),
        Some(ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED | ERROR_INVALID_PARAMETER)
    ) {
        return Err(error);
    }
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: same handle and lifetime, using the classic structure.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&raw const info).cast(),
            size_of_val(&info) as u32,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
