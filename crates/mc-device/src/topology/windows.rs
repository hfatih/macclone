use super::{Disk, Placement, Role, Topology, extents, locations::Locations};
use crate::win::powershell;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_NO_MORE_FILES, ERROR_SHARING_VIOLATION,
        HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
        GetDriveTypeW, GetFinalPathNameByHandleW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        VOLUME_NAME_GUID,
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::{DISK_EXTENT, VOLUME_DISK_EXTENTS},
        SystemInformation::GetSystemWindowsDirectoryW,
        WindowsProgramming::{DRIVE_CDROM, DRIVE_REMOTE},
    },
};

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const _: () = {
    assert!(std::mem::offset_of!(VOLUME_DISK_EXTENTS, Extents) == extents::HEADER);
    assert!(std::mem::size_of::<DISK_EXTENT>() == extents::ENTRY);
    assert!(std::mem::offset_of!(DISK_EXTENT, StartingOffset) == 8);
    assert!(std::mem::offset_of!(DISK_EXTENT, ExtentLength) == 16);
    assert!(extents::is_non_disk_volume(DRIVE_CDROM));
    assert!(extents::is_non_disk_volume(DRIVE_REMOTE));
};

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn volume_guid(path: &str) -> io::Result<String> {
    let prefix = path.strip_prefix(r"\\?\").unwrap_or_default();
    let component = prefix.split('\\').next().unwrap_or_default();
    let name = component.to_ascii_lowercase();
    // A normalized GUID root is also the key used for volumes without drive letters.
    let id = name
        .strip_prefix("volume{")
        .and_then(|s| s.strip_suffix('}'));
    if id.is_none_or(|id| {
        id.len() != 36
            || !id.bytes().enumerate().all(|(i, b)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
    }) {
        return Err(crate::invalid("Volume GUID could not be determined"));
    }
    Ok(format!("\\\\?\\{name}\\"))
}

struct VolumeSearch(HANDLE);
impl Drop for VolumeSearch {
    fn drop(&mut self) {
        // SAFETY: this search owns the live handle returned by FindFirstVolumeW.
        unsafe { FindVolumeClose(self.0) };
    }
}

fn volumes() -> io::Result<Vec<String>> {
    let mut buffer = [0u16; 100];
    // SAFETY: the output points to a writable UTF-16 buffer of the specified size.
    let first = unsafe { FindFirstVolumeW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if first == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let search = VolumeSearch(first);
    let mut names = BTreeSet::new();
    loop {
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .ok_or_else(|| crate::invalid("Unterminated volume name"))?;
        let path = String::from_utf16(&buffer[..end]).map_err(io::Error::other)?;
        names.insert(volume_guid(&path)?);
        buffer.fill(0);
        // SAFETY: the search handle is still live and the output buffer is writable.
        if unsafe { FindNextVolumeW(search.0, buffer.as_mut_ptr(), buffer.len() as u32) } == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                Ok(names.into_iter().collect())
            } else {
                Err(error)
            };
        }
    }
}

pub(in crate::topology) fn disk_extents(volume: &str) -> io::Result<Vec<extents::Extent>> {
    // CreateFile requires the GUID without its final separator for a volume handle.
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(SHARE_ALL)
        .open(volume.trim_end_matches('\\'))?;
    extents::read(|buffer| {
        let mut returned = 0;
        // SAFETY: the file is live and the synchronous call only uses the writable output and count.
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                std::ptr::null(),
                0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            return Ok(extents::Response::Complete(returned as usize));
        }
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(code)
            if code == ERROR_MORE_DATA as i32 || code == ERROR_INSUFFICIENT_BUFFER as i32)
        {
            Ok(extents::Response::NeedSpace)
        } else {
            Err(error)
        }
    })
}

fn windows_directory() -> io::Result<String> {
    let mut buffer = [0u16; 32768];
    // SAFETY: the output points to a writable UTF-16 buffer of the specified size.
    let len =
        unsafe { GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    if len >= buffer.len() {
        return Err(crate::invalid("Windows directory path is too long"));
    }
    String::from_utf16(&buffer[..len]).map_err(io::Error::other)
}

fn local_storage_path(raw: &str, windows: &str) -> io::Result<PathBuf> {
    let raw = raw.trim().replace('/', "\\");
    if raw.contains('\0') {
        return Err(crate::invalid("Invalid storage path"));
    }
    let upper = raw.to_ascii_uppercase();
    let expanded = if upper.starts_with("%SYSTEMROOT%\\") {
        format!("{windows}{}", &raw["%SYSTEMROOT%".len()..])
    } else if upper.starts_with(r"\SYSTEMROOT\") {
        format!("{windows}{}", &raw[r"\SYSTEMROOT".len()..])
    } else if upper.starts_with("%SYSTEMDRIVE%\\") {
        let drive = windows
            .get(..2)
            .filter(|s| s.ends_with(':'))
            .ok_or_else(|| crate::invalid("Windows drive could not be determined"))?;
        format!("{drive}{}", &raw["%SYSTEMDRIVE%".len()..])
    } else if let Some(native) = raw.strip_prefix(r"\??\") {
        format!(r"\\?\{native}")
    } else {
        raw
    };
    let path = PathBuf::from(&expanded);
    use std::path::{Component, Prefix};
    if expanded.contains('%')
        || !path.is_absolute()
        || !matches!(path.components().next(),
            Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) ||
                p.kind().is_verbatim() && volume_guid(&expanded).is_ok()
        )
    {
        return Err(crate::invalid("Unsupported local storage path"));
    }
    Ok(path)
}

fn volume_for_path(path: &Path, allow_missing: bool) -> io::Result<String> {
    for candidate in path.ancestors() {
        // Resolve junctions through the actual open handle. A dump file may not exist yet.
        let file = match OpenOptions::new()
            .access_mode(0)
            .share_mode(SHARE_ALL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(candidate)
        {
            Ok(file) => file,
            Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(candidate) {
                    Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => {
                        continue;
                    }
                    _ => return Err(error),
                }
            }
            Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) => {
                // In-use pagefiles may deny an open; walk up only for an ordinary file.
                let metadata = fs::symlink_metadata(candidate)?;
                if metadata.is_file()
                    && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
                {
                    continue;
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        return volume_for_handle(&file);
    }
    Err(crate::invalid("No existing local storage ancestor"))
}

fn volume_for_handle(file: &fs::File) -> io::Result<String> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: file owns the handle and the output buffer is writable.
    let len = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_GUID,
        )
    } as usize;
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    if len >= buffer.len() {
        return Err(crate::invalid("Resolved volume path is too long"));
    }
    volume_guid(&String::from_utf16(&buffer[..len]).map_err(io::Error::other)?)
}

fn place(
    topology: &mut Topology,
    volumes: &BTreeMap<String, Vec<u32>>,
    role: Role,
    location: &str,
    windows: &str,
) {
    let mut resolved_volume = None;
    let locations = local_storage_path(location, windows)
        .and_then(|path| volume_for_path(&path, role == Role::CrashDump))
        .and_then(|volume| {
            let disks = volumes.get(&volume).cloned();
            resolved_volume = Some(volume);
            disks.ok_or_else(|| crate::invalid("Volume extents are unavailable"))
        });
    match locations {
        Ok(disks) => topology.protected.push(Placement {
            role,
            volume: resolved_volume,
            disks,
        }),
        Err(error) => topology.report_error(
            "protected_location_unresolved",
            format!("{} konumu çözülemedi ({error}).", role.label()),
            resolved_volume,
        ),
    }
}

pub(super) fn observe(disks: &[Disk]) -> Topology {
    let mut topology = Topology::default();
    // IsSystem covers the active Windows boot partition even when EFI has no volume GUID.
    let system_disks: Vec<_> = disks
        .iter()
        .filter(|d| d.system == Some(true))
        .map(|d| d.number)
        .collect();
    if system_disks.is_empty() {
        topology.report_error(
            "system_location_unresolved",
            "Windows önyükleme bölümünün fiziksel diski belirlenemedi.".into(),
            None,
        );
    } else {
        topology.protected.push(Placement {
            role: Role::System,
            volume: None,
            disks: system_disks,
        });
    }
    let mut volume_disks = BTreeMap::<String, Vec<u32>>::new();
    let volume_names = volumes();
    match volume_names {
        Ok(names) => {
            for volume in names {
                let root = wide(Path::new(&volume));
                // SAFETY: root is a NUL-terminated volume GUID path with a trailing separator.
                let typ = unsafe { GetDriveTypeW(root.as_ptr()) };
                if extents::is_non_disk_volume(typ) {
                    continue;
                }
                match disk_extents(&volume) {
                    Ok(extents)
                        if extents.iter().all(|e| {
                            disks
                                .iter()
                                .any(|d| d.number == e.disk && e.start + e.length <= d.bytes)
                        }) =>
                    {
                        let numbers: BTreeSet<_> = extents.iter().map(|e| e.disk).collect();
                        volume_disks.insert(volume, numbers.into_iter().collect());
                    }
                    result => {
                        let detail = result.err().map_or_else(
                            || "disk kayboldu veya extent disk sınırını aşıyor".into(),
                            |e| e.to_string(),
                        );
                        topology.report_error(
                            "volume_extents_unavailable",
                            format!("{volume} fiziksel disklerle eşleştirilemedi ({detail})."),
                            Some(volume),
                        );
                    }
                }
            }
        }
        Err(error) => topology.report_error(
            "volume_inventory_unavailable",
            format!("Windows volume listesi alınamadı ({error})."),
            None,
        ),
    }

    topology.attached = volume_disks
        .iter()
        .map(|(guid, numbers)| (guid.clone(), numbers.clone()))
        .collect();
    let windows = match windows_directory() {
        Ok(path) => path,
        Err(error) => {
            topology.report_error(
                "windows_location_unavailable",
                format!("Çalışan Windows dizini belirlenemedi ({error})."),
                None,
            );
            return topology;
        }
    };
    place(
        &mut topology,
        &volume_disks,
        Role::Windows,
        &windows,
        &windows,
    );

    let locations: Locations = match powershell(include_str!("locations.ps1")) {
        Ok(locations) => locations,
        Err(error) => {
            topology.report_error(
                "storage_locations_unavailable",
                format!("Sayfa dosyası ve çökme dökümü bilgileri alınamadı ({error})."),
                None,
            );
            return topology;
        }
    };
    locations.observe(&mut topology, |topology, role, path| {
        place(topology, &volume_disks, role, path, &windows);
    });
    topology
}

#[cfg(feature = "windows-physical-clone")]
pub(in crate::topology) fn journal_disks(file: &fs::File) -> io::Result<Vec<u32>> {
    Ok(disk_extents(&volume_for_handle(file)?)?
        .into_iter()
        .map(|extent| extent.disk)
        .collect())
}

#[cfg(feature = "windows-physical-clone")]
pub(in crate::topology) fn hold_directory(path: &Path) -> io::Result<fs::File> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(crate::invalid(
            "Journal directory must be a local, non-reparse directory",
        ));
    }
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(crate::invalid("Expected a journal directory handle"));
    }
    Ok(file)
}
