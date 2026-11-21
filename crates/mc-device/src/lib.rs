//! Physical devices export a reader. Only regular job files can be sinks.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}
pub trait Reader: Read + Seek + Send {}
impl<T: Read + Seek + Send> Reader for T {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Disk {
    pub number: u32,
    pub model: String,
    pub serial: Option<String>,
    pub id: Option<String>,
    pub bytes: u64,
    pub logical: u32,
    pub physical: u32,
    pub bus: String,
    pub boot: Option<bool>,
    pub system: Option<bool>,
    pub offline: Option<bool>,
    pub readonly: Option<bool>,
    pub clustered: Option<bool>,
    pub health: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Input {
    Image { path: PathBuf, sector: u32 },
    Disk { number: u32, id: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stamp {
    pub identity: String,
    pub bytes: u64,
    pub sector: u32,
    pub modified_ns: Option<String>,
}
pub struct Source {
    pub reader: Box<dyn Reader>,
    pub stamp: Stamp,
    pub backing_file: Option<File>,
}

pub fn geometry(bytes: u64, sector: u32) -> io::Result<()> {
    if !matches!(sector, 512 | 4096) || bytes == 0 || !bytes.is_multiple_of(sector as u64) {
        return Err(invalid(
            "Expected a non-empty, sector-aligned 512 or 4096 byte geometry",
        ));
    }
    Ok(())
}

/// Reject DOS device namespaces, alternate streams, reserved basenames and UNC destinations.
pub fn local_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(invalid("Empty path"));
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        for part in path.components() {
            match part {
                Component::Prefix(p) if !matches!(p.kind(), Prefix::Disk(_)) => {
                    return Err(invalid("Use a local drive-letter path"));
                }
                Component::Normal(n) => {
                    let s = n.to_str().ok_or_else(|| invalid("Unsupported file name"))?;
                    let base = s.split('.').next().unwrap_or("").trim_end().to_uppercase();
                    if s.contains(':')
                        || s.ends_with(['.', ' '])
                        || matches!(
                            base.as_str(),
                            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
                        )
                        || (base.starts_with("COM") || base.starts_with("LPT"))
                            && base.chars().skip(3).count() == 1
                    {
                        return Err(invalid("Reserved Windows path"));
                    }
                }
                _ => (),
            }
        }
    }
    Ok(())
}

pub fn open_source(input: &Input, consistent: bool) -> io::Result<Source> {
    match input {
        Input::Image { path, sector } => {
            local_path(path)?;
            if !fs::metadata(path)?.is_file() {
                return Err(invalid("Source must be a regular image file"));
            }
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(1);
            }
            let f = options.open(path)?;
            let m = f.metadata()?;
            if !m.is_file() {
                return Err(invalid("Opened source is not a regular file"));
            }
            geometry(m.len(), *sector)?;
            let stamp = Stamp {
                identity: format!("{}#{}", fs::canonicalize(path)?.display(), file_id(&f)?.0),
                bytes: m.len(),
                sector: *sector,
                modified_ns: Some(
                    m.modified()?
                        .duration_since(UNIX_EPOCH)
                        .map_err(io::Error::other)?
                        .as_nanos()
                        .to_string(),
                ),
            };
            Ok(Source {
                backing_file: Some(f.try_clone()?),
                reader: Box::new(f),
                stamp,
            })
        }
        Input::Disk { number, id } => open_disk(*number, id, consistent),
    }
}

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::disks;
#[cfg(windows)]
use win::open_disk;
#[cfg(not(windows))]
pub fn disks() -> io::Result<Vec<Disk>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Physical disk inventory requires Windows",
    ))
}
#[cfg(not(windows))]
fn open_disk(_: u32, _: &str, _: bool) -> io::Result<Source> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Physical devices require Windows",
    ))
}

mod topology;
#[cfg(all(windows, feature = "windows-physical-clone"))]
pub use topology::approval::{ApprovedClone, PreparedClone};
pub use topology::{CloneLayout, ClonePlan, TopologyIssue, plan_disks};
#[cfg(all(windows, feature = "windows-physical-clone"))]
pub use win::PhysicalTarget;

fn plan(source: &Disk, target: &Disk) -> Vec<String> {
    let mut reasons = Vec::new();
    let id = |s: &Option<String>| s.as_deref().unwrap_or("").trim().to_ascii_uppercase();
    if source.number == target.number
        || id(&source.id) == id(&target.id)
        || id(&source.serial) == id(&target.serial)
    {
        reasons.push("Same or ambiguous physical identity".into())
    }
    for d in [source, target] {
        if id(&d.id).is_empty() || id(&d.serial).is_empty() || d.model.trim().is_empty() {
            reasons.push(format!("Disk {} has incomplete identity", d.number))
        }
        if d.boot != Some(false)
            || d.system != Some(false)
            || d.clustered != Some(false)
            || d.offline != Some(true)
        {
            reasons.push(format!(
                "Disk {} must be offline, non-system, non-boot and non-clustered",
                d.number
            ))
        }
        if geometry(d.bytes, d.logical).is_err()
            || !d.physical.is_power_of_two()
            || d.physical > 4096
            || d.physical < d.logical
            || !d.bytes.is_multiple_of(d.physical.max(1) as u64)
        {
            reasons.push(format!("Disk {} has unsupported geometry", d.number))
        }
        if !matches!(
            d.bus.to_ascii_uppercase().as_str(),
            "USB" | "SATA" | "ATA" | "NVME"
        ) {
            reasons.push(format!("Disk {} has unsupported topology", d.number))
        }
    }
    if target.readonly != Some(false) {
        reasons.push("Target is read-only or write state is unknown".into())
    }
    if source.bytes > target.bytes {
        reasons.push("Destination is smaller than the source disk".into())
    }
    if source.logical != target.logical {
        reasons.push("Logical sector sizes differ".into())
    }
    reasons
}

/// Regular file identity from the *open handle*. Link count is checked before resuming writes.
pub fn file_id(file: &File) -> io::Result<(String, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        Ok((format!("{:x}-{:x}", m.dev(), m.ino()), m.nlink()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: File keeps a valid handle, Windows initializes info on success.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let i = unsafe { info.assume_init() };
        Ok((
            format!(
                "{:x}-{:08x}{:08x}",
                i.dwVolumeSerialNumber, i.nFileIndexHigh, i.nFileIndexLow
            ),
            u64::from(i.nNumberOfLinks),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn disk(n: u32) -> Disk {
        Disk {
            number: n,
            model: "Synthetic".into(),
            serial: Some(format!("SN{n}")),
            id: Some(format!("id{n}")),
            bytes: 8192,
            logical: 512,
            physical: 4096,
            bus: "USB".into(),
            boot: Some(false),
            system: Some(false),
            offline: Some(true),
            readonly: Some(false),
            clustered: Some(false),
            health: None,
        }
    }
    #[test]
    fn distinct_compatible() {
        assert!(plan(&disk(1), &disk(2)).is_empty())
    }
    #[test]
    fn identity_and_aliases() {
        let a = disk(1);
        assert!(!plan(&a, &a).is_empty());
        let mut b = disk(2);
        b.serial = a.serial.clone();
        assert!(!plan(&a, &b).is_empty());
        b.serial = None;
        assert!(!plan(&a, &b).is_empty())
    }
    #[test]
    fn capacity_and_tail() {
        let mut smaller = disk(2);
        smaller.bytes = 4096;
        assert!(!plan(&disk(1), &smaller).is_empty());
        let mut larger = disk(2);
        larger.bytes = 16384;
        assert!(plan(&disk(1), &larger).is_empty());
    }
    #[test]
    fn hdd_and_ssd_models_can_be_source_or_target() {
        let mut hdd = disk(1);
        let mut ssd = disk(2);
        hdd.model = "HDD".into();
        ssd.model = "SSD".into();
        assert!(plan(&hdd, &ssd).is_empty());
        assert!(plan(&ssd, &hdd).is_empty());
        hdd.bytes *= 2;
        assert!(plan(&ssd, &hdd).is_empty());
        assert_eq!(
            CloneLayout::for_sizes(ssd.bytes, hdd.bytes),
            CloneLayout::RelocateGpt
        );
        assert!(!plan(&hdd, &ssd).is_empty());
    }

    #[test]
    fn sector_mismatch() {
        let mut b = disk(2);
        b.logical = 4096;
        assert!(!plan(&disk(1), &b).is_empty())
    }
    #[test]
    fn boot_and_system_protected() {
        for v in [None, Some(true)] {
            let mut b = disk(2);
            b.boot = v;
            assert!(!plan(&disk(1), &b).is_empty());
            b = disk(2);
            b.system = v;
            assert!(!plan(&disk(1), &b).is_empty())
        }
    }
    #[test]
    fn online_and_unknown_rejected() {
        let mut b = disk(2);
        b.offline = None;
        assert!(!plan(&disk(1), &b).is_empty());
        b = disk(2);
        b.clustered = Some(true);
        assert!(!plan(&disk(1), &b).is_empty())
    }
}
