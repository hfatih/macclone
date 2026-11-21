use super::{at, bad, gpt, u32le, u64le};
use std::io::{self, Read, Seek, SeekFrom};

struct Patch {
    start: u64,
    bytes: Vec<u8>,
}

/// Prepares a larger GPT address space with unchanged partition entries and partition data.
pub struct GptRelocation {
    original_bytes: u64,
    target_bytes: u64,
    patches: Vec<Patch>,
}

impl GptRelocation {
    pub fn prepare<R: Read + Seek>(
        reader: &mut R,
        original_bytes: u64,
        target_bytes: u64,
        sector: u32,
    ) -> io::Result<Self> {
        let s = u64::from(sector);
        if !matches!(sector, 512 | 4096)
            || original_bytes < 6 * s
            || target_bytes <= original_bytes
            || !original_bytes.is_multiple_of(s)
            || !target_bytes.is_multiple_of(s)
        {
            return Err(bad(
                "GPT relocation requires a larger sector-aligned destination",
            ));
        }
        let original = original_bytes / s;
        let target = target_bytes / s;
        let mut mbr = vec![0; s as usize];
        at(reader, 0, &mut mbr)?;
        let protective: Vec<_> = mbr[446..510]
            .chunks_exact(16)
            .enumerate()
            .filter(|(_, e)| e.iter().any(|byte| *byte != 0))
            .collect();
        let [(entry_index, entry)] = protective.as_slice() else {
            return Err(bad("Expected a single protective MBR entry"));
        };
        if mbr[510..512] != [0x55, 0xaa]
            || entry[0] != 0
            || entry[4] != 0xee
            || u32le(entry, 8) != 1
            || u32le(entry, 12) != (original - 1).min(u64::from(u32::MAX)) as u32
        {
            return Err(bad(
                "Protective MBR does not describe the entire source disk",
            ));
        }

        let primary = gpt(reader, 1, original, s)?;
        let secondary = gpt(reader, original - 1, original, s)?;
        let sectors = (primary.entries.len() as u64).div_ceil(s);
        if primary.other != original - 1
            || secondary.other != 1
            || primary.id != secondary.id
            || primary.header[56..72] == [0; 16]
            || primary.entries != secondary.entries
            || primary.stride != secondary.stride
            || primary.first != secondary.first
            || primary.last != secondary.last
            || primary.entries_at + sectors > primary.first
            || secondary.entries_at <= primary.last
            || secondary.entries_at + sectors != original - 1
            || u32le(&primary.header, 12) != 92
            || u32le(&secondary.header, 12) != 92
        {
            return Err(bad("Source GPT copies or metadata placement disagree"));
        }

        let mut ranges = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for entry in primary
            .entries
            .chunks_exact(primary.stride)
            .filter(|e| e[..16] != [0; 16])
        {
            let id: [u8; 16] = entry[16..32].try_into().unwrap();
            let start = u64le(entry, 32);
            let end = u64le(entry, 40);
            if id == [0; 16]
                || !ids.insert(id)
                || start > end
                || start < primary.first
                || end > primary.last
            {
                return Err(bad("Invalid GPT partition entry"));
            }
            ranges.push((start, end));
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[0].1 >= pair[1].0) {
            return Err(bad("GPT partitions overlap"));
        }

        let delta = target - original;
        let new_last = primary.last + delta;
        let entry_lba = secondary.entries_at + delta;
        let obsolete_start = secondary.entries_at * s;
        let obsolete_end = original_bytes.min(entry_lba * s);
        let mbr_count = 446 + entry_index * 16 + 12;
        mbr[mbr_count..mbr_count + 4]
            .copy_from_slice(&((target - 1).min(u64::from(u32::MAX)) as u32).to_le_bytes());
        let primary_entries = primary.entries_at;
        let mut entries = secondary.entries;
        entries.resize((sectors * s) as usize, 0);
        let patches = vec![
            Patch {
                start: 0,
                bytes: mbr,
            },
            Patch {
                start: s,
                bytes: header(primary.header, 1, target - 1, new_last, primary_entries),
            },
            Patch {
                start: obsolete_start,
                bytes: vec![0; (obsolete_end - obsolete_start) as usize],
            },
            Patch {
                start: entry_lba * s,
                bytes: entries,
            },
            Patch {
                start: (target - 1) * s,
                bytes: header(secondary.header, target - 1, 1, new_last, entry_lba),
            },
        ];
        Ok(Self {
            original_bytes,
            target_bytes,
            patches,
        })
    }

    pub fn target_bytes(&self) -> u64 {
        self.target_bytes
    }

    pub fn reader<R: Read + Seek>(&self, source: R) -> RelocatedReader<'_, R> {
        RelocatedReader {
            layout: self,
            source,
            position: 0,
        }
    }
}

fn header(mut bytes: Vec<u8>, location: u64, alternate: u64, last: u64, entries: u64) -> Vec<u8> {
    for (offset, value) in [(24, location), (32, alternate), (48, last), (72, entries)] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes[16..20].fill(0);
    let checksum = crc32fast::hash(&bytes[..92]);
    bytes[16..20].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

pub struct RelocatedReader<'a, R> {
    layout: &'a GptRelocation,
    source: R,
    position: u64,
}

impl<R: Read + Seek> Read for RelocatedReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let n = output.len().min(
            (self.layout.target_bytes - self.position)
                .try_into()
                .unwrap_or(usize::MAX),
        );
        let existing = n.min(
            self.layout
                .original_bytes
                .saturating_sub(self.position)
                .try_into()
                .unwrap_or(usize::MAX),
        );
        if existing != 0 {
            at(&mut self.source, self.position, &mut output[..existing])?;
        }
        output[existing..n].fill(0);
        let end = self.position + n as u64;
        for patch in &self.layout.patches {
            let first = self.position.max(patch.start);
            let last = end.min(patch.start + patch.bytes.len() as u64);
            if first < last {
                output[(first - self.position) as usize..(last - self.position) as usize]
                    .copy_from_slice(
                        &patch.bytes[(first - patch.start) as usize..(last - patch.start) as usize],
                    );
            }
        }
        self.position = end;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for RelocatedReader<'_, R> {
    fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
        let next = match seek {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.layout.target_bytes.checked_add_signed(delta),
        }
        .filter(|n| *n <= self.layout.target_bytes)
        .ok_or_else(|| bad("Seek outside GPT destination"))?;
        self.position = next;
        Ok(next)
    }
}
