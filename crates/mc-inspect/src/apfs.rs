//! Read-only APFS container inspection, directory browsing, and file extraction.
use apfs_core::{ApfsContainer, dir, extent, inode::Inode, volume::ApfsVolume};
use serde::Serialize;
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

const MAX_EXTRACT_DEPTH: usize = 64;
const APFS_FS_UNENCRYPTED: u64 = 0x1;

#[derive(Debug, Clone, Serialize)]
pub struct ApfsVolumeInfo {
    pub index: usize,
    pub name: String,
    pub role: String,
    pub used_bytes: u64,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApfsEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ExtractSummary {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub skipped: u64,
}

struct PartitionSlice<R> {
    inner: R,
    base: u64,
    len: u64,
    pos: u64,
}

impl<R: Read + Seek> PartitionSlice<R> {
    fn new(inner: R, base: u64, len: u64) -> Self {
        Self {
            inner,
            base,
            len,
            pos: 0,
        }
    }
}

impl<R: Read + Seek> Read for PartitionSlice<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.len.saturating_sub(self.pos);
        if remaining == 0 {
            return Ok(0);
        }
        let max_read = buf.len().min(remaining.try_into().unwrap_or(usize::MAX));
        self.inner.seek(SeekFrom::Start(self.base + self.pos))?;
        let n = self.inner.read(&mut buf[..max_read])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for PartitionSlice<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
        }
        .filter(|n| *n <= self.len)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Seek outside APFS partition")
        })?;
        self.pos = next;
        Ok(next)
    }
}

fn apfs_err(err: apfs_core::ApfsError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

fn role_label(role: u16) -> String {
    match role {
        0 => "Standart",
        1 => "Sistem",
        2 => "Kullanıcı",
        4 => "Recovery",
        8 => "VM",
        16 => "Preboot",
        32 => "Installer",
        64 => "Veri (Data)",
        128 => "Baseband",
        192 => "Update",
        _ => "Diğer",
    }
    .into()
}

fn inode_kind(mode: u16) -> &'static str {
    match mode & 0o170000 {
        0o040000 => "dir",
        0o100000 => "file",
        0o120000 => "symlink",
        _ => "other",
    }
}

fn join_apfs_path(base: &str, name: &str) -> String {
    let trimmed = base.trim_matches('/');
    if trimmed.is_empty() {
        format!("/{name}")
    } else {
        format!("/{trimmed}/{name}")
    }
}

fn sanitize_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "_item".into()
    } else {
        trimmed.into()
    }
}

fn open_volume<R: Read + Seek>(
    reader: R,
    part_offset: u64,
    part_bytes: u64,
    volume_index: usize,
) -> io::Result<(PartitionSlice<R>, ApfsVolume, usize)> {
    let slice = PartitionSlice::new(reader, part_offset, part_bytes);
    let mut container = ApfsContainer::open(slice).map_err(apfs_err)?;
    let block_size = container.superblock().block_size as usize;
    let addrs = container.volume_superblock_addrs().map_err(apfs_err)?;
    let &paddr = addrs
        .get(volume_index)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APFS hacmi bulunamadı."))?;
    let mut slice = container.into_reader();
    let mut buf = vec![0u8; block_size];
    slice.seek(SeekFrom::Start(paddr.saturating_mul(block_size as u64)))?;
    slice.read_exact(&mut buf)?;
    let volume = ApfsVolume::parse(&buf).map_err(apfs_err)?;
    Ok((slice, volume, block_size))
}

pub fn list_volumes<R: Read + Seek>(
    reader: R,
    part_offset: u64,
    part_bytes: u64,
) -> io::Result<Vec<ApfsVolumeInfo>> {
    let slice = PartitionSlice::new(reader, part_offset, part_bytes);
    let mut container = ApfsContainer::open(slice).map_err(apfs_err)?;
    let block_size = container.superblock().block_size as usize;
    let addrs = container.volume_superblock_addrs().map_err(apfs_err)?;
    let mut slice = container.into_reader();
    let mut out = Vec::with_capacity(addrs.len());
    let mut buf = vec![0u8; block_size];
    for (index, paddr) in addrs.into_iter().enumerate() {
        slice.seek(SeekFrom::Start(paddr.saturating_mul(block_size as u64)))?;
        slice.read_exact(&mut buf)?;
        let vol = ApfsVolume::parse(&buf).map_err(apfs_err)?;
        // apfs_fs_alloc_count (u64 @88) and apfs_role (u16 @964) in apfs_superblock_t.
        let alloc_blocks = if buf.len() >= 96 {
            u64::from_le_bytes(buf[88..96].try_into().unwrap())
        } else {
            0
        };
        let role = if buf.len() >= 966 {
            u16::from_le_bytes(buf[964..966].try_into().unwrap())
        } else {
            0
        };
        out.push(ApfsVolumeInfo {
            index,
            name: vol.name().to_string(),
            role: role_label(role),
            used_bytes: alloc_blocks.saturating_mul(block_size as u64),
            encrypted: (vol.fs_flags() & APFS_FS_UNENCRYPTED) == 0,
        });
    }
    Ok(out)
}

pub fn list_dir<R: Read + Seek>(
    reader: R,
    part_offset: u64,
    part_bytes: u64,
    volume_index: usize,
    path: &str,
) -> io::Result<Vec<ApfsEntry>> {
    let (mut slice, volume, block_size) =
        open_volume(reader, part_offset, part_bytes, volume_index)?;
    let dir_inode = if path.trim_matches('/').is_empty() {
        dir::load_inode(&mut slice, &volume, dir::ROOT_DIR_INO_NUM, block_size).map_err(apfs_err)?
    } else {
        dir::open_path(&mut slice, &volume, path, block_size).map_err(apfs_err)?
    };
    let entries =
        dir::list_dir(&mut slice, &volume, dir_inode.oid, block_size).map_err(apfs_err)?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let (kind, size) = match dir::load_inode(&mut slice, &volume, entry.file_id, block_size) {
            Ok(ino) => (inode_kind(ino.mode).to_string(), ino.size.unwrap_or(0)),
            Err(_) => ("other".into(), 0),
        };
        out.push(ApfsEntry {
            path: join_apfs_path(path, &entry.name),
            name: entry.name,
            kind,
            size,
            inode: entry.file_id,
        });
    }
    out.sort_by(|a, b| {
        let a_dir = a.kind == "dir";
        let b_dir = b.kind == "dir";
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

fn extract_node<R: Read + Seek>(
    slice: &mut PartitionSlice<R>,
    volume: &ApfsVolume,
    block_size: usize,
    inode: &Inode,
    target_path: &Path,
    depth: usize,
    summary: &mut ExtractSummary,
) -> io::Result<()> {
    if depth > MAX_EXTRACT_DEPTH {
        summary.skipped += 1;
        return Ok(());
    }
    match inode_kind(inode.mode) {
        "dir" => {
            fs::create_dir_all(target_path)?;
            summary.dirs += 1;
            let entries = match dir::list_dir(slice, volume, inode.oid, block_size) {
                Ok(e) => e,
                Err(_) => {
                    summary.skipped += 1;
                    return Ok(());
                }
            };
            for entry in entries {
                let child = match dir::load_inode(slice, volume, entry.file_id, block_size) {
                    Ok(c) => c,
                    Err(_) => {
                        summary.skipped += 1;
                        continue;
                    }
                };
                let child_path = target_path.join(sanitize_component(&entry.name));
                extract_node(
                    slice,
                    volume,
                    block_size,
                    &child,
                    &child_path,
                    depth + 1,
                    summary,
                )?;
            }
        }
        "file" => match extent::read_data(slice, volume, inode, block_size) {
            Ok(data) => {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(target_path, &data)?;
                summary.files += 1;
                summary.bytes += data.len() as u64;
            }
            Err(_) => {
                summary.skipped += 1;
            }
        },
        _ => {
            summary.skipped += 1;
        }
    }
    Ok(())
}

pub fn extract_path<R: Read + Seek>(
    reader: R,
    part_offset: u64,
    part_bytes: u64,
    volume_index: usize,
    apfs_path: &str,
    dest_dir: &Path,
) -> io::Result<ExtractSummary> {
    let (mut slice, volume, block_size) =
        open_volume(reader, part_offset, part_bytes, volume_index)?;
    let trimmed = apfs_path.trim_matches('/');
    let inode = if trimmed.is_empty() {
        dir::load_inode(&mut slice, &volume, dir::ROOT_DIR_INO_NUM, block_size).map_err(apfs_err)?
    } else {
        dir::open_path(&mut slice, &volume, apfs_path, block_size).map_err(apfs_err)?
    };
    fs::create_dir_all(dest_dir)?;
    let target = if trimmed.is_empty() {
        dest_dir.join(sanitize_component(volume.name()))
    } else {
        let leaf = trimmed.rsplit('/').next().unwrap_or("item");
        dest_dir.join(sanitize_component(leaf))
    };
    let mut summary = ExtractSummary::default();
    extract_node(
        &mut slice,
        &volume,
        block_size,
        &inode,
        &target,
        0,
        &mut summary,
    )?;
    Ok(summary)
}
