use super::*;
use std::{
    io::{Read, Seek, SeekFrom},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt},
    process::Command,
};
use windows_sys::Win32::System::{
    IO::DeviceIoControl,
    Ioctl::{
        IOCTL_DISK_GET_LENGTH_INFO, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY,
        PropertyStandardQuery, STORAGE_PROPERTY_QUERY, StorageAccessAlignmentProperty,
        StorageDeviceProperty,
    },
    SystemInformation::GetSystemDirectoryW,
};

pub fn disks() -> io::Result<Vec<Disk>> {
    powershell(include_str!("inventory.ps1"))
}

pub(super) fn powershell<T: serde::de::DeserializeOwned>(script: &str) -> io::Result<T> {
    let mut path = [0u16; 512];
    // SAFETY: the buffer is writable for its stated number of UTF-16 code units.
    let n = unsafe { GetSystemDirectoryW(path.as_mut_ptr(), path.len() as u32) } as usize;
    if n == 0 || n >= path.len() {
        return Err(io::Error::last_os_error());
    }
    let exe = PathBuf::from(String::from_utf16_lossy(&path[..n]))
        .join("WindowsPowerShell/v1.0/powershell.exe");
    let result = Command::new(exe)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .creation_flags(0x08000000)
        .output()?;
    if !result.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&result.stderr).trim().to_string(),
        ));
    }
    serde_json::from_slice(&result.stdout).map_err(io::Error::other)
}

fn query<const N: usize>(f: &File, code: u32) -> io::Result<[u8; N]> {
    let mut out = [0u8; N];
    let mut count = 0;
    // SAFETY: live File owns the handle; this synchronous ioctl uses the bounded output buffer only.
    let ok = unsafe {
        DeviceIoControl(
            f.as_raw_handle(),
            code,
            std::ptr::null(),
            0,
            out.as_mut_ptr().cast(),
            N as u32,
            &mut count,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if count as usize != N {
        return Err(invalid("Unexpected device response length"));
    }
    Ok(out)
}

fn property(f: &File, id: i32) -> io::Result<Vec<u8>> {
    let q = STORAGE_PROPERTY_QUERY {
        PropertyId: id,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut out = vec![0u8; 16384];
    let mut count = 0;
    // SAFETY: the SDK query and writable result live until this synchronous call returns.
    let ok = unsafe {
        DeviceIoControl(
            f.as_raw_handle(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&q as *const STORAGE_PROPERTY_QUERY).cast(),
            std::mem::size_of_val(&q) as u32,
            out.as_mut_ptr().cast(),
            out.len() as u32,
            &mut count,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    out.truncate(count as usize);
    if out.len() < 8 {
        return Err(invalid("Truncated storage descriptor"));
    }
    let declared = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
    if !(8..=out.len()).contains(&declared) {
        return Err(invalid("Invalid storage descriptor length"));
    }
    out.truncate(declared);
    Ok(out)
}

fn bind_handle(file: &File, d: &Disk, check_bus: bool) -> io::Result<()> {
    let alignment = property(file, StorageAccessAlignmentProperty)?;
    if alignment.len() < 28
        || u32::from_le_bytes(alignment[16..20].try_into().unwrap()) != d.logical
        || u32::from_le_bytes(alignment[20..24].try_into().unwrap()) != d.physical
        || (check_bus && u32::from_le_bytes(alignment[24..28].try_into().unwrap()) != 0)
        || d.physical > 4096
        || !d.physical.is_power_of_two()
        || d.physical < d.logical
    {
        return Err(invalid("Unconfirmed or unsupported drive geometry"));
    }
    let descriptor = property(file, StorageDeviceProperty)?;
    if descriptor.len() < 28 {
        return Err(invalid("Missing device descriptor"));
    }
    if check_bus {
        use windows_sys::Win32::Storage::FileSystem::{
            BusTypeAta as BUS_ATA, BusTypeNvme as BUS_NVME, BusTypeSata as BUS_SATA,
            BusTypeUsb as BUS_USB,
        };
        let reported = descriptor
            .get(28..32)
            .map(|bytes| i32::from_le_bytes(bytes.try_into().unwrap()));
        let matches_inventory = matches!(
            (reported, d.bus.to_ascii_uppercase().as_str()),
            (Some(BUS_ATA), "ATA")
                | (Some(BUS_NVME), "NVME")
                | (Some(BUS_SATA), "SATA")
                | (Some(BUS_USB), "USB")
        );
        if !matches_inventory {
            return Err(invalid(
                "Raw handle transport does not match the approved inventory",
            ));
        }
    }
    let at = u32::from_le_bytes(descriptor[24..28].try_into().unwrap()) as usize;
    let serial = descriptor
        .get(at..)
        .filter(|_| at >= 36)
        .and_then(|tail| tail.split(|c| *c == 0).next())
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
        .trim();
    if serial.is_empty() || !serial.eq_ignore_ascii_case(d.serial.as_deref().unwrap_or("").trim()) {
        return Err(invalid(
            "Serial number from drive handle does not match inventory",
        ));
    }
    Ok(())
}

pub(super) fn open_disk(number: u32, id: &str, consistent: bool) -> io::Result<Source> {
    let select = || -> io::Result<Disk> {
        let all = disks()?;
        if id.trim().is_empty() || all.iter().filter(|d| d.id.as_deref() == Some(id)).count() != 1 {
            return Err(invalid("Missing or ambiguous device ID"));
        }
        all.into_iter()
            .find(|d| d.number == number && d.id.as_deref() == Some(id))
            .ok_or_else(|| invalid("Disk selection changed"))
    };
    let d = select()?;
    geometry(d.bytes, d.logical)?;
    if consistent
        && (d.offline != Some(true)
            || d.system != Some(false)
            || d.boot != Some(false)
            || d.clustered != Some(false))
    {
        return Err(invalid(
            "Acquisition requires a previously offlined, non-system, non-clustered source",
        ));
    }
    let path = format!(r"\\.\PhysicalDrive{number}");
    let file = OpenOptions::new()
        .read(true)
        .share_mode(if consistent { 1 } else { 3 })
        .open(path)?;
    let len = u64::from_le_bytes(query(&file, IOCTL_DISK_GET_LENGTH_INFO)?);
    if consistent {
        bind_handle(&file, &d, false)?;
        raw_disk_attributes(&file, false)?;
    }
    let dev = query::<12>(&file, IOCTL_STORAGE_GET_DEVICE_NUMBER)?;
    if u32::from_le_bytes(dev[4..8].try_into().unwrap()) != number
        || len != d.bytes
        || select()? != d
    {
        return Err(invalid("Disk changed while opening"));
    }
    let stamp = Stamp {
        identity: format!(
            "disk:{}:{}:{}",
            id,
            d.model,
            d.serial.as_deref().unwrap_or("")
        ),
        bytes: len,
        sector: d.logical,
        modified_ns: None,
    };
    Ok(Source {
        stamp,
        backing_file: None,
        reader: Box::new(SectorReader {
            file,
            pos: 0,
            require_offline: consistent,
            len,
            sector: d.logical as usize,
            buf: Aligned([0u8; 65536]),
        }),
    })
}

// Some storage drivers reject sub-sector I/O, including small metadata probes.
#[repr(align(4096))]
struct Aligned([u8; 65536]);
struct SectorReader {
    buf: Aligned,
    file: File,
    pos: u64,
    len: u64,
    sector: usize,
    require_offline: bool,
}
impl Read for SectorReader {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let max = (self.len - self.pos).min(b.len() as u64) as usize;
        if max == 0 {
            return Ok(0);
        }
        if self.require_offline {
            raw_disk_attributes(&self.file, false)?;
        }
        let sector = self.sector as u64;
        let base = self.pos / self.sector as u64 * sector;
        self.file.seek(SeekFrom::Start(base))?;
        let skip = (self.pos - base) as usize;
        let n = max.min(self.buf.0.len() - skip);
        let read_len = (n + skip).div_ceil(self.sector) * self.sector;
        if self.require_offline {
            if self.file.read(&mut self.buf.0[..read_len])? != read_len {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Short aligned raw disk read",
                ));
            }
        } else {
            self.file.read_exact(&mut self.buf.0[..read_len])?;
        }
        b[..n].copy_from_slice(&self.buf.0[skip..skip + n]);
        self.pos += n as u64;
        Ok(n)
    }
}
impl Seek for SectorReader {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        if matches!(p, SeekFrom::Current(0)) && self.require_offline {
            raw_disk_attributes(&self.file, false)?;
        }
        let v = match p {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.pos as i128 + n as i128,
            SeekFrom::End(n) => self.len as i128 + n as i128,
        };
        if !(0..=self.len as i128).contains(&v) {
            return Err(invalid("Seek outside disk"));
        }
        self.pos = v as u64;
        Ok(self.pos)
    }
}

fn raw_disk_attributes(file: &File, writable: bool) -> io::Result<()> {
    use windows_sys::Win32::System::Ioctl::{
        DISK_ATTRIBUTE_OFFLINE, DISK_ATTRIBUTE_READ_ONLY, GET_DISK_ATTRIBUTES,
        IOCTL_DISK_GET_DISK_ATTRIBUTES,
    };
    const SIZE: usize = std::mem::size_of::<GET_DISK_ATTRIBUTES>();
    let bytes = query::<SIZE>(file, IOCTL_DISK_GET_DISK_ATTRIBUTES).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Cannot confirm the disk offline state: {error}"),
        )
    })?;
    let version = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let attributes = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    if version != SIZE
        || attributes & DISK_ATTRIBUTE_OFFLINE == 0
        || (writable && attributes & DISK_ATTRIBUTE_READ_ONLY != 0)
    {
        return Err(invalid(
            "Clone device is no longer in its approved offline state",
        ));
    }
    Ok(())
}

#[cfg(feature = "windows-physical-clone")]
fn confirmed_raw_handle(disk: &Disk, writable: bool) -> io::Result<SectorReader> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DEVICE_DISK, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH,
    };
    let mut options = OpenOptions::new();
    let flags = FILE_FLAG_NO_BUFFERING | if writable { FILE_FLAG_WRITE_THROUGH } else { 0 };
    options
        .read(true)
        .write(writable)
        .share_mode(0)
        .custom_flags(flags);
    let path = format!(r"\\.\PhysicalDrive{}", disk.number);
    let file = options.open(path)?;
    let number = query::<12>(&file, IOCTL_STORAGE_GET_DEVICE_NUMBER)?;
    if u32::from_le_bytes(number[..4].try_into().unwrap()) != FILE_DEVICE_DISK
        || u32::from_le_bytes(number[4..8].try_into().unwrap()) != disk.number
        || u64::from_le_bytes(query(&file, IOCTL_DISK_GET_LENGTH_INFO)?) != disk.bytes
    {
        return Err(invalid("Raw device size or disk number changed"));
    }
    bind_handle(&file, disk, true)?;
    raw_disk_attributes(&file, writable)?;
    Ok(SectorReader {
        buf: Aligned([0; 65536]),
        file,
        pos: 0,
        len: disk.bytes,
        sector: disk.logical as usize,
        require_offline: true,
    })
}

#[cfg(feature = "windows-physical-clone")]
fn disk_identity(disk: &Disk) -> io::Result<String> {
    // JSON framing avoids collisions between model, ID and serial delimiters.
    Ok(format!(
        "physical:{}",
        serde_json::to_string(&(
            &disk.id,
            &disk.serial,
            &disk.model,
            &disk.bus,
            disk.bytes,
            disk.logical,
            disk.physical,
        ))?
    ))
}

#[cfg(feature = "windows-physical-clone")]
pub(super) fn open_clone_handles(
    source: &Disk,
    target: &Disk,
) -> io::Result<(Source, PhysicalTarget)> {
    let source_reader = confirmed_raw_handle(source, false)?;
    let target_writer = confirmed_raw_handle(target, true)?;
    Ok((
        Source {
            reader: Box::new(source_reader),
            stamp: Stamp {
                identity: disk_identity(source)?,
                bytes: source.bytes,
                sector: source.logical,
                modified_ns: None,
            },
            backing_file: None,
        },
        PhysicalTarget {
            disk: target_writer,
            identity: disk_identity(target)?,
            alignment: target.physical as usize,
        },
    ))
}

#[cfg(feature = "windows-physical-clone")]
pub struct PhysicalTarget {
    disk: SectorReader,
    identity: String,
    alignment: usize,
}

#[cfg(feature = "windows-physical-clone")]
impl PhysicalTarget {
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn capacity(&self) -> u64 {
        self.disk.len
    }
    pub fn reported_length(&self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(query(
            &self.disk.file,
            IOCTL_DISK_GET_LENGTH_INFO,
        )?))
    }
    pub fn assert_writable_offline(&self) -> io::Result<()> {
        raw_disk_attributes(&self.disk.file, true)
    }
    pub fn sync(&self) -> io::Result<()> {
        self.disk.file.sync_all()?;
        self.assert_writable_offline()
    }
}

#[cfg(feature = "windows-physical-clone")]
impl Read for PhysicalTarget {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.disk.read(b)
    }
}

#[cfg(feature = "windows-physical-clone")]
impl Seek for PhysicalTarget {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.disk.seek(position)
    }
}

#[cfg(feature = "windows-physical-clone")]
impl std::io::Write for PhysicalTarget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.disk.pos.is_multiple_of(self.alignment as u64)
            || !bytes.len().is_multiple_of(self.alignment)
            || bytes.len() as u64 > self.disk.len - self.disk.pos
        {
            return Err(invalid("Physical write is unaligned or outside the disk"));
        }
        self.assert_writable_offline()?;
        let count = bytes.len().min(self.disk.buf.0.len());
        self.disk.file.seek(SeekFrom::Start(self.disk.pos))?;
        self.disk.buf.0[..count].copy_from_slice(&bytes[..count]);
        // A short write error leaves the checkpoint uncommitted for a retry.
        if self.disk.file.write(&self.disk.buf.0[..count])? != count {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "Partial physical disk write",
            ));
        }
        self.disk.pos += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sync()
    }
}

#[cfg(all(test, feature = "windows-physical-clone"))]
mod vhdx_test;
