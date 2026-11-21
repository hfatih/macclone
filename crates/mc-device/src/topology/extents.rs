use std::io;

// VOLUME_DISK_EXTENTS and DISK_EXTENT layout for the Windows x64 ABI.
pub(super) const HEADER: usize = 8;
pub(super) const ENTRY: usize = 24;
const MAX_COUNT: usize = 4096;
// WinBase GetDriveType values for DRIVE_REMOTE and DRIVE_CDROM. Unknown types
// still get probed so an unmounted local hard disk volume cannot be skipped.
pub(super) const fn is_non_disk_volume(drive_type: u32) -> bool {
    matches!(drive_type, 4 | 5)
}

// WinBase GetDriveType value for DRIVE_REMOVABLE (e.g. SD/MMC card readers).
pub(super) const fn is_removable_volume(drive_type: u32) -> bool {
    drive_type == 2
}

const MAX_BYTES: usize = HEADER + ENTRY * MAX_COUNT;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Extent {
    pub(super) disk: u32,
    pub(super) start: u64,
    pub(super) length: u64,
}

fn malformed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid VOLUME_DISK_EXTENTS response",
    )
}

fn parse_with(bytes: &[u8], allow_empty: bool) -> io::Result<Vec<Extent>> {
    let count =
        u32::from_le_bytes(bytes.get(..4).ok_or_else(malformed)?.try_into().unwrap()) as usize;
    if count == 0 && allow_empty && bytes.len() >= HEADER {
        return Ok(Vec::new());
    }
    if !(1..=MAX_COUNT).contains(&count) {
        return Err(malformed());
    }
    let entries = bytes
        .get(HEADER..HEADER + count * ENTRY)
        .ok_or_else(malformed)?;
    if allow_empty && count == 1 && entries[8..24].iter().all(|&b| b == 0) {
        return Ok(Vec::new());
    }
    entries
        .as_chunks::<ENTRY>()
        .0
        .iter()
        .map(|entry| {
            let disk = u32::from_le_bytes(entry[..4].try_into().unwrap());
            let start = i64::from_le_bytes(entry[8..16].try_into().unwrap());
            let length = i64::from_le_bytes(entry[16..24].try_into().unwrap());
            if start < 0 || length <= 0 || start.checked_add(length).is_none() {
                return Err(malformed());
            }
            Ok(Extent {
                disk,
                start: start as u64,
                length: length as u64,
            })
        })
        .collect()
}

#[cfg(test)]
fn parse(bytes: &[u8]) -> io::Result<Vec<Extent>> {
    parse_with(bytes, false)
}

pub(super) enum Response {
    Complete(usize),
    NeedSpace,
}

pub(super) fn read_with(
    allow_empty: bool,
    mut query: impl FnMut(&mut [u8]) -> io::Result<Response>,
) -> io::Result<Vec<Extent>> {
    let mut buffer = vec![0; HEADER + ENTRY];
    loop {
        match query(&mut buffer)? {
            Response::Complete(returned) => {
                return parse_with(buffer.get(..returned).ok_or_else(malformed)?, allow_empty);
            }
            Response::NeedSpace if buffer.len() < MAX_BYTES => {
                buffer.resize((buffer.len() * 2).min(MAX_BYTES), 0);
            }
            Response::NeedSpace => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Volume has too many disk extents",
                ));
            }
        }
    }
}

#[cfg(test)]
pub(super) fn read(
    query: impl FnMut(&mut [u8]) -> io::Result<Response>,
) -> io::Result<Vec<Extent>> {
    read_with(false, query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_recognized_optical_and_remote_volumes_only() {
        for typ in [4, 5] {
            assert!(is_non_disk_volume(typ));
        }
        for typ in [0, 1, 2, 3, 6] {
            assert!(!is_non_disk_volume(typ));
        }
    }

    #[test]
    fn retries_and_parses_every_extent() {
        let mut attempts = 0;
        let result = read(|buffer| {
            attempts += 1;
            if buffer.len() < HEADER + 2 * ENTRY {
                return Ok(Response::NeedSpace);
            }
            buffer[..4].copy_from_slice(&2u32.to_le_bytes());
            for (i, disk) in [7u32, 19].into_iter().enumerate() {
                let at = HEADER + i * ENTRY;
                buffer[at..at + 4].copy_from_slice(&disk.to_le_bytes());
                buffer[at + 8..at + 16].copy_from_slice(&512i64.to_le_bytes());
                buffer[at + 16..at + 24].copy_from_slice(&1024i64.to_le_bytes());
            }
            Ok(Response::Complete(HEADER + 2 * ENTRY))
        })
        .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(
            result,
            [
                Extent {
                    disk: 7,
                    start: 512,
                    length: 1024
                },
                Extent {
                    disk: 19,
                    start: 512,
                    length: 1024
                },
            ]
        );
    }

    #[test]
    fn rejects_truncated_and_invalid_ranges() {
        let mut bytes = [0u8; HEADER + ENTRY];
        assert!(parse(&bytes).is_err());
        bytes[..4].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse(&bytes[..bytes.len() - 1]).is_err());
        assert!(parse(&bytes).is_err());
        bytes[16..24].copy_from_slice(&i64::MAX.to_le_bytes());
        bytes[24..32].copy_from_slice(&1i64.to_le_bytes());
        assert!(parse(&bytes).is_err());
        assert!(read(|buffer| Ok(Response::Complete(buffer.len() + 1))).is_err());
    }

    #[test]
    fn bounds_allocation_and_propagates_access_errors() {
        assert!(read(|_| Ok(Response::NeedSpace)).is_err());
        let error = read(|_| Err(io::ErrorKind::PermissionDenied.into())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn accepts_empty_extents_for_removable_no_media_only() {
        let empty = [0u8; HEADER];
        assert!(parse_with(&empty, false).is_err());
        assert_eq!(parse_with(&empty, true).unwrap(), Vec::new());
        let mut zero_len = [0u8; HEADER + ENTRY];
        zero_len[..4].copy_from_slice(&1u32.to_le_bytes());
        zero_len[8..12].copy_from_slice(&3u32.to_le_bytes());
        assert!(parse_with(&zero_len, false).is_err());
        assert_eq!(parse_with(&zero_len, true).unwrap(), Vec::new());
        assert!(is_removable_volume(2));
        assert!(!is_removable_volume(3));
    }
}
