//! Native transport acceptance against volumes created by this test only.
use super::*;
use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Storage::{
        FileSystem::{BusTypeFileBackedVirtual, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH},
        Vhd::{
            ATTACH_VIRTUAL_DISK_FLAG_NO_DRIVE_LETTER, ATTACH_VIRTUAL_DISK_PARAMETERS,
            ATTACH_VIRTUAL_DISK_VERSION_1, AttachVirtualDisk, CREATE_VIRTUAL_DISK_PARAMETERS,
            CREATE_VIRTUAL_DISK_PARAMETERS_0, CREATE_VIRTUAL_DISK_PARAMETERS_0_1,
            CREATE_VIRTUAL_DISK_VERSION_2, CreateVirtualDisk, DETACH_VIRTUAL_DISK_FLAG_NONE,
            DetachVirtualDisk, GetVirtualDiskPhysicalPath, VIRTUAL_DISK_ACCESS_NONE,
            VIRTUAL_STORAGE_TYPE, VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
            VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
        },
    },
    System::Ioctl::{DISK_ATTRIBUTE_OFFLINE, IOCTL_DISK_SET_DISK_ATTRIBUTES, SET_DISK_ATTRIBUTES},
};

const CAPACITY: u64 = 16 * 1024 * 1024;

fn win_result(code: u32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

struct TestDisk(File);

impl TestDisk {
    fn new(path: &Path, sector: u32) -> io::Result<Self> {
        let name: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        let storage = VIRTUAL_STORAGE_TYPE {
            DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
            VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
        };
        let create = CREATE_VIRTUAL_DISK_PARAMETERS {
            Version: CREATE_VIRTUAL_DISK_VERSION_2,
            Anonymous: CREATE_VIRTUAL_DISK_PARAMETERS_0 {
                Version2: CREATE_VIRTUAL_DISK_PARAMETERS_0_1 {
                    MaximumSize: CAPACITY,
                    SectorSizeInBytes: sector,
                    PhysicalSectorSizeInBytes: sector,
                    ..Default::default()
                },
            },
        };
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: path is terminated and all input and output buffers live through this synchronous call.
        win_result(unsafe {
            CreateVirtualDisk(
                &storage,
                name.as_ptr(),
                VIRTUAL_DISK_ACCESS_NONE,
                std::ptr::null_mut(),
                0,
                0,
                &create,
                std::ptr::null(),
                &mut handle,
            )
        })?;
        // SAFETY: CreateVirtualDisk on success gives an owned Windows file handle.
        let disk = Self(unsafe { File::from_raw_handle(handle) });
        let attach = ATTACH_VIRTUAL_DISK_PARAMETERS {
            Version: ATTACH_VIRTUAL_DISK_VERSION_1,
            ..Default::default()
        };
        // SAFETY: handle and parameters are valid. The attachment is tied to the owned VHDX handle.
        win_result(unsafe {
            AttachVirtualDisk(
                handle,
                std::ptr::null_mut(),
                ATTACH_VIRTUAL_DISK_FLAG_NO_DRIVE_LETTER,
                0,
                &attach,
                std::ptr::null(),
            )
        })?;
        Ok(disk)
    }

    fn attached_path(&self) -> io::Result<String> {
        let mut path = [0u16; 256];
        let mut len = std::mem::size_of_val(&path) as u32;
        // SAFETY: live owned VHDX handle and a writable buffer large enough for the reported bytes.
        win_result(unsafe {
            GetVirtualDiskPhysicalPath(self.0.as_raw_handle(), &mut len, path.as_mut_ptr())
        })?;
        let len = path
            .iter()
            .position(|value| *value == 0)
            .ok_or_else(|| invalid("Invalid test disk path"))?;
        let path = String::from_utf16(&path[..len]).map_err(io::Error::other)?;
        let number = path
            .strip_prefix(r"\\.\PhysicalDrive")
            .and_then(|value| value.parse::<u32>().ok());
        if number.is_none() {
            return Err(invalid("Test disk attachment has no physical number"));
        }
        Ok(path)
    }

    fn open_raw(&self, writable: bool) -> io::Result<File> {
        let path = self.attached_path()?;
        let mut options = OpenOptions::new();
        let flags = FILE_FLAG_NO_BUFFERING | if writable { FILE_FLAG_WRITE_THROUGH } else { 0 };
        options
            .read(true)
            .write(writable)
            .share_mode(0)
            .custom_flags(flags);
        let file = options.open(&path)?;
        let descriptor = property(&file, StorageDeviceProperty)?;
        let bus = descriptor
            .get(28..32)
            .map(|bytes| i32::from_le_bytes(bytes.try_into().unwrap()));
        let size = u64::from_le_bytes(query(&file, IOCTL_DISK_GET_LENGTH_INFO)?);
        let actual_disk = query::<12>(&file, IOCTL_STORAGE_GET_DEVICE_NUMBER)?;
        let actual_number = u32::from_le_bytes(actual_disk[4..8].try_into().unwrap());
        if self.attached_path()? != path
            || path != format!(r"\\.\PhysicalDrive{actual_number}")
            || bus != Some(BusTypeFileBackedVirtual)
            || size != CAPACITY
        {
            return Err(invalid("The raw handle does not match the disposable VHDX"));
        }
        Ok(file)
    }
}

impl Drop for TestDisk {
    fn drop(&mut self) {
        // SAFETY: detach only the VHDX this test created; attachment also expires at handle close.
        unsafe { DetachVirtualDisk(self.0.as_raw_handle(), DETACH_VIRTUAL_DISK_FLAG_NONE, 0) };
    }
}

fn take_offline(file: &File) -> io::Result<()> {
    let flags = SET_DISK_ATTRIBUTES {
        Version: std::mem::size_of::<SET_DISK_ATTRIBUTES>() as u32,
        Attributes: DISK_ATTRIBUTE_OFFLINE,
        AttributesMask: DISK_ATTRIBUTE_OFFLINE,
        ..Default::default()
    };
    let mut returned = 0;
    // SAFETY: only a proven test VHDX's exclusive handle receives the offline flag.
    if unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            IOCTL_DISK_SET_DISK_ATTRIBUTES,
            (&flags as *const SET_DISK_ATTRIBUTES).cast(),
            flags.Version,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    raw_disk_attributes(file, true)
}

fn test_reader(file: File, sector: u32) -> SectorReader {
    SectorReader {
        buf: Aligned([0; 65536]),
        file,
        pos: 0,
        len: CAPACITY,
        sector: sector as usize,
        require_offline: true,
    }
}

fn test_target(file: File, sector: u32) -> PhysicalTarget {
    PhysicalTarget {
        disk: test_reader(file, sector),
        identity: "temporary Windows acceptance disk".into(),
        alignment: sector as usize,
    }
}

#[test]
#[ignore = "Requires an elevated isolated Windows VM with Virtual Disk support"]
fn disposable_vhdx_raw_roundtrip() -> io::Result<()> {
    use std::io::Write;
    for sector in [512, 4096] {
        let sandbox = tempfile::tempdir()?;
        let source = TestDisk::new(&sandbox.path().join("source.vhdx"), sector)?;
        let destination = TestDisk::new(&sandbox.path().join("destination.vhdx"), sector)?;
        if source.attached_path()? == destination.attached_path()? {
            return Err(invalid("Disposable VHDX endpoints alias each other"));
        }

        let seed = source.open_raw(true)?;
        take_offline(&seed)?;
        let mut seed = test_target(seed, sector);
        seed.write_all(&vec![0x68; sector as usize])?;
        let last = CAPACITY - u64::from(sector);
        seed.seek(SeekFrom::Start(last))?;
        seed.write_all(&vec![0x67; sector as usize])?;
        seed.sync()?;
        drop(seed);

        let target = destination.open_raw(true)?;
        take_offline(&target)?;
        let mut target = test_target(target, sector);
        let mut reader = test_reader(source.open_raw(false)?, sector);
        let mut buffer = vec![0u8; 1024 * 1024];
        while reader.pos < CAPACITY {
            reader.read_exact(&mut buffer)?;
            target.write_all(&buffer)?;
        }
        target.sync()?;
        drop(target);

        let mut reread = test_reader(destination.open_raw(false)?, sector);
        let mut result = vec![0u8; buffer.len()];
        reader.seek(SeekFrom::Start(0))?;
        while reader.pos < CAPACITY {
            reader.read_exact(&mut buffer)?;
            reread.read_exact(&mut result)?;
            assert_eq!(result, buffer);
        }
        assert_eq!(
            &result[result.len() - sector as usize..],
            &vec![0x67; sector as usize]
        );
    }
    Ok(())
}
