//! Bounded, read-only probes. Unrecognized data can still be imaged.
use serde::Serialize;
use std::io::{self, Read, Seek, SeekFrom};
fn bad(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn at<R: Read + Seek>(r: &mut R, offset: u64, b: &mut [u8]) -> io::Result<()> {
    r.seek(SeekFrom::Start(offset))?;
    r.read_exact(b)
}
#[derive(Debug, Serialize, PartialEq)]
pub struct Partition {
    pub start_lba: u64,
    pub end_lba: u64,
    pub kind: String,
    pub label: String,
    pub filesystem: String,
}
#[derive(Debug, Serialize)]
pub struct Layout {
    pub table: String,
    pub partitions: Vec<Partition>,
    pub filesystem: String,
    pub warnings: Vec<String>,
}
fn fs<R: Read + Seek>(r: &mut R, base: u64, size: u64) -> io::Result<String> {
    let mut b = [0; 1536];
    let n = size.min(b.len() as u64) as usize;
    at(r, base, &mut b[..n])?;
    Ok(if n >= 40 && &b[32..36] == b"NXSB" {
        "APFS signature"
    } else if n >= 1028 && &b[1024..1028] == b"H+\0\x04" {
        "HFS+ signature"
    } else if n >= 1028 && &b[1024..1028] == b"HX\0\x05" {
        "HFSX signature"
    } else {
        "Unknown"
    }
    .into())
}
fn guid(b: &[u8]) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{}",
        u32le(b, 0),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8],
        b[9],
        b[10..16]
            .iter()
            .map(|n| format!("{n:02x}"))
            .collect::<String>()
    )
}
mod relocate;
pub use relocate::{GptRelocation, RelocatedReader};

struct Gpt {
    header: Vec<u8>,
    entries_at: u64,
    first: u64,
    last: u64,
    other: u64,
    id: String,
    entries: Vec<u8>,
    stride: usize,
}
fn gpt<R: Read + Seek>(r: &mut R, at_lba: u64, total: u64, s: u64) -> io::Result<Gpt> {
    let mut h = vec![0; s as usize];
    at(r, at_lba * s, &mut h)?;
    if &h[..8] != b"EFI PART" || u32le(&h, 8) != 0x10000 {
        return Err(bad("GPT signature/revision"));
    }
    let len = u32le(&h, 12) as usize;
    if !(92..=h.len()).contains(&len) {
        return Err(bad("GPT header length"));
    }
    let crc = u32le(&h, 16);
    h[16..20].fill(0);
    if crc32fast::hash(&h[..len]) != crc || u64le(&h, 24) != at_lba || u32le(&h, 20) != 0 {
        return Err(bad("GPT header CRC/location"));
    }
    let first = u64le(&h, 40);
    let last = u64le(&h, 48);
    let other = u64le(&h, 32);
    let entries_at = u64le(&h, 72);
    let count = u32le(&h, 80) as usize;
    let stride = u32le(&h, 84) as usize;
    let size = count
        .checked_mul(stride)
        .filter(|x| *x > 0 && *x <= 16 * 1024 * 1024)
        .ok_or_else(|| bad("GPT entry array outside supported bounds"))?;
    let end = entries_at
        .checked_add((size as u64).div_ceil(s))
        .ok_or_else(|| bad("GPT overflow"))?;
    if first < 2
        || first > last
        || last >= total - 1
        || other >= total
        || other == at_lba
        || other == 0
        || !(128..=4096).contains(&stride)
        || !stride.is_power_of_two()
        || count > 16384
        || entries_at < 2
        || end > total
        || (at_lba == 1 && (end > first || other != total - 1))
        || (at_lba == total - 1 && (entries_at <= last || end > at_lba || other != 1))
        || (entries_at..end).contains(&at_lba)
        || (entries_at..end).contains(&other)
    {
        return Err(bad("Invalid GPT bounds"));
    }
    let mut entries = vec![0; size];
    at(r, entries_at * s, &mut entries)?;
    if crc32fast::hash(&entries) != u32le(&h, 88) {
        return Err(bad("GPT entry array CRC"));
    }
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    Ok(Gpt {
        entries_at,
        first,
        last,
        other,
        id: guid(&h[56..72]),
        entries,
        stride,
        header: h,
    })
}
pub fn inspect<R: Read + Seek>(r: &mut R, bytes: u64, sector: u32) -> io::Result<Layout> {
    if !matches!(sector, 512 | 4096)
        || bytes < u64::from(sector)
        || !bytes.is_multiple_of(sector.into())
    {
        return Err(bad("Invalid image geometry"));
    }
    let s = u64::from(sector);
    let total = bytes / s;
    let mut m = [0; 512];
    let mbr_read = at(r, 0, &mut m);
    let mut o = Layout {
        table: "Unknown".into(),
        partitions: vec![],
        filesystem: "Unknown".into(),
        warnings: vec![],
    };
    let mut tag = [0; 8];
    let tag_read = if total > 1 {
        at(r, s, &mut tag)
    } else {
        Ok(())
    };
    let primary_signature = tag_read.is_ok() && &tag == b"EFI PART";
    let protective = mbr_read.is_ok()
        && m[510..] == [0x55, 0xaa]
        && m[446..510].chunks_exact(16).any(|e| e[4] == 0xee);
    let backup_hint = if total > 1 && !primary_signature && !protective {
        // A valid backup can identify GPT even after losing both records at the front.
        gpt(r, total - 1, total, s).ok()
    } else {
        None
    };
    let is_gpt = primary_signature || protective || backup_hint.is_some();
    let mbr_unreadable = match mbr_read {
        Err(error) if !is_gpt => return Err(error),
        Err(error) => Some(error),
        Ok(()) => None,
    };
    let mut apm_entry = [0u8; 2];
    if !is_gpt {
        tag_read?;
        if &m[..2] == b"ER" && bytes >= 514 {
            at(r, 512, &mut apm_entry)?;
        }
    }
    if is_gpt {
        let primary = gpt(r, 1, total, s);
        let backup = backup_hint.map_or_else(|| gpt(r, total - 1, total, s), Ok);
        let chosen = match (primary, backup) {
            (Ok(a), Ok(b)) => {
                if a.other != total - 1
                    || b.other != 1
                    || a.id != b.id
                    || a.entries != b.entries
                    || a.first != b.first
                    || a.last != b.last
                    || a.stride != b.stride
                {
                    o.warnings.push("GPT copies disagree".into())
                }
                a
            }
            (Ok(a), Err(e)) => {
                o.warnings.push(format!("Backup GPT at final LBA: {e}"));
                a
            }
            (Err(e), Ok(b)) => {
                o.warnings.push(format!("Using backup GPT; primary: {e}"));
                b
            }
            (Err(a), Err(b)) => return Err(bad(&format!("Both GPT copies invalid: {a}, {b}"))),
        };
        o.table = "GPT".into();
        if let Some(error) = mbr_unreadable {
            o.warnings
                .push(format!("Protective MBR unreadable: {error}"));
        } else if !protective {
            o.warnings.push("Protective MBR missing".into());
        }
        for e in chosen
            .entries
            .chunks_exact(chosen.stride)
            .filter(|e| e[..16] != [0; 16])
        {
            let start = u64le(e, 32);
            let end = u64le(e, 40);
            if start > end || start < chosen.first || end > chosen.last {
                return Err(bad("GPT partition outside usable space"));
            }
            let id = guid(&e[..16]);
            let kind = if id == "c12a7328-f81f-11d2-ba4b-00a0c93ec93b" {
                "EFI System".into()
            } else {
                id
            };
            let name: Vec<_> = e[56..128]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|v| *v != 0)
                .collect();
            o.partitions.push(Partition {
                start_lba: start,
                end_lba: end,
                kind,
                label: String::from_utf16_lossy(&name),
                filesystem: fs(r, start * s, (end - start + 1) * s)?,
            });
        }
    } else if &m[..2] == b"ER" && &apm_entry == b"PM" {
        o.table = "APM signature".into();
        o.warnings
            .push("Apple Partition Map entries have not been decoded".into())
    } else if m[510..] == [0x55, 0xaa] && m[446..510].chunks_exact(16).any(|e| e[4] != 0) {
        o.table = "MBR".into();
        for e in m[446..510].chunks_exact(16).filter(|e| e[4] != 0) {
            let start = u64::from(u32le(e, 8));
            let n = u64::from(u32le(e, 12));
            if start == 0 || n == 0 || start + n > total {
                return Err(bad("MBR partition out of bounds"));
            }
            let kind = if e[4] == 0xef {
                "EFI System".into()
            } else {
                format!("MBR {:02x}", e[4])
            };
            if matches!(e[4], 0x05 | 0x0f | 0x85) {
                o.warnings
                    .push("Extended partition; EBR chain not decoded".into())
            }
            o.partitions.push(Partition {
                start_lba: start,
                end_lba: start + n - 1,
                kind,
                label: String::new(),
                filesystem: fs(r, start * s, n * s)?,
            });
        }
    } else {
        o.filesystem = fs(r, 0, bytes)?
    }
    o.partitions.sort_by_key(|p| p.start_lba);
    if o.partitions
        .windows(2)
        .any(|p| p[0].end_lba >= p[1].start_lba)
    {
        return Err(bad("Overlapping partitions"));
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn probe(mut b: Vec<u8>) -> io::Result<Layout> {
        let size = b.len() as u64;
        inspect(&mut Cursor::new(&mut b), size, 512)
    }
    fn mbr() -> Vec<u8> {
        let mut b = vec![0; 8192];
        b[510..512].copy_from_slice(&[0x55, 0xaa]);
        b[450] = 0xef;
        b[454] = 1;
        b[458] = 8;
        b
    }
    #[test]
    fn unknown() {
        assert_eq!(probe(vec![0; 4096]).unwrap().filesystem, "Unknown")
    }
    #[test]
    fn gpt_backup_provides_layout_when_front_sectors_are_unreadable() {
        struct UnreadableFront(Cursor<Vec<u8>>);
        impl Read for UnreadableFront {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                if self.0.position() < 1024 {
                    return Err(io::Error::other("simulated unreadable sector"));
                }
                self.0.read(b)
            }
        }
        impl Seek for UnreadableFront {
            fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
                self.0.seek(seek)
            }
        }
        let mut data = vec![0_u8; 8 * 512];
        let table = [0_u8; 128];
        let header = &mut data[7 * 512..8 * 512];
        header[..8].copy_from_slice(b"EFI PART");
        for (offset, value) in [
            (8, 0x10000_u32),
            (12, 92),
            (80, 1),
            (84, 128),
            (88, crc32fast::hash(&table)),
        ] {
            header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [(24, 7_u64), (32, 1), (40, 3), (48, 5), (72, 6)] {
            header[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        let crc = crc32fast::hash(&header[..92]);
        header[16..20].copy_from_slice(&crc.to_le_bytes());
        let len = data.len() as u64;
        let result = inspect(&mut UnreadableFront(Cursor::new(data)), len, 512).unwrap();
        assert_eq!(result.table, "GPT");
        assert!(
            result
                .warnings
                .iter()
                .any(|warn| warn.contains("Using backup GPT"))
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|warn| warn.contains("Protective MBR unreadable"))
        );
    }

    #[test]
    fn mbr_efi() {
        let p = probe(mbr()).unwrap();
        assert_eq!(p.table, "MBR");
        assert_eq!(p.partitions[0].kind, "EFI System")
    }
    #[test]
    fn overlaps() {
        let mut b = mbr();
        b.copy_within(446..462, 462);
        assert!(probe(b).is_err())
    }
    #[test]
    fn bad_bounds() {
        let mut b = mbr();
        b[458] = 17;
        assert!(probe(b).is_err())
    }
    #[test]
    fn apfs_and_hfs() {
        for (offset, tag, want) in [
            (32, &b"NXSB"[..], "APFS signature"),
            (1024, &b"H+\0\x04"[..], "HFS+ signature"),
            (1024, &b"HX\0\x05"[..], "HFSX signature"),
        ] {
            let mut b = vec![0; 4096];
            b[offset..offset + 4].copy_from_slice(tag);
            assert_eq!(probe(b).unwrap().filesystem, want)
        }
    }
    #[test]
    fn apm_signature() {
        let mut b = vec![0; 4096];
        b[512..514].copy_from_slice(b"PM");
        assert_eq!(probe(b.clone()).unwrap().table, "Unknown");
        b[..2].copy_from_slice(b"ER");
        assert_eq!(probe(b).unwrap().table, "APM signature")
    }
    #[test]
    fn broken_protective_mbr() {
        let mut b = mbr();
        b[450] = 0xee;
        assert!(probe(b).is_err())
    }
}
