//! Regular-file acquisition, append-only checkpoint journal and independent destination reread.
#[cfg(all(windows, feature = "windows-physical-clone"))]
mod physical;
#[cfg(all(windows, feature = "windows-physical-clone"))]
pub use physical::clone_physical;

use mc_device::{Input, Source, Stamp, file_id, invalid, local_path, open_source};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Options {
    pub block: u32,
    pub recovery: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            block: 1024 * 1024,
            recovery: false,
        }
    }
}
impl Options {
    pub fn validate(self, sector: u32) -> io::Result<()> {
        if sector == 0
            || self.block < sector
            || self.block > 8 * 1024 * 1024
            || !self.block.is_multiple_of(sector)
        {
            Err(invalid("Block must be a sector multiple, up to 8 MiB"))
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Manifest {
    version: u32,
    input: Input,
    source: Stamp,
    options: Options,
    target_id: String,
    map_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relocated_gpt_target_bytes: Option<u64>,
}
#[derive(Debug, Serialize, Deserialize)]
struct Frame<T> {
    record: T,
    checksum: String,
}
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    start: u64,
    end: u64,
    sha256: String,
    missing: Vec<u32>,
}
#[derive(Clone, Debug, Serialize, Default)]
pub struct Progress {
    pub phase: String,
    pub bytes: u64,
    pub total: u64,
    pub missing_sectors: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub outcome: String,
    pub bytes: u64,
    pub missing_sectors: u64,
    pub sha256: String,
    pub verified_bytes: u64,
    pub target_reread_bytes: u64,
    pub logical_sector: u32,
    pub schema_version: u32,
    pub source_identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_relocated_from_bytes: Option<u64>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Cancelled at a checkpoint")
}
fn checkpoint(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(interrupted())
    } else {
        Ok(())
    }
}
fn media_error(e: &io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(e.raw_os_error(), Some(23 | 27))
    }
    #[cfg(not(windows))]
    {
        e.raw_os_error() == Some(5)
    }
}
fn read_at<R: Read + Seek + ?Sized>(r: &mut R, start: u64, b: &mut [u8]) -> io::Result<()> {
    r.seek(SeekFrom::Start(start))?;
    r.read_exact(b)
}
fn acquire<R: Read + Seek + ?Sized>(
    r: &mut R,
    start: u64,
    b: &mut [u8],
    sector: u32,
    recover: bool,
    cancel: &AtomicBool,
) -> io::Result<Vec<u32>> {
    match read_at(r, start, b) {
        Ok(()) => return Ok(vec![]),
        Err(e) if recover && media_error(&e) => (),
        Err(e) => return Err(e),
    }
    let mut missing = vec![];
    for (i, chunk) in b.chunks_exact_mut(sector as usize).enumerate() {
        checkpoint(cancel)?;
        let offset = start + i as u64 * u64::from(sector);
        for attempt in 0..2 {
            match read_at(r, offset, chunk) {
                Ok(()) => break,
                Err(e) if media_error(&e) => {
                    if attempt == 1 {
                        chunk.fill(0);
                        missing.push(i as u32)
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(missing)
}
fn job_file(path: &Path, new: bool, write: bool) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(write).create_new(new);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(if write {
                FILE_SHARE_READ
            } else {
                // Allow reading our own write handle while it remains open.
                FILE_SHARE_READ | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
            });
    }
    let file = opts.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("Job path is a reparse point"));
        }
    }
    if !metadata.is_file() || file_id(&file)?.1 != 1 {
        return Err(invalid("Job file is not private to this directory"));
    }
    Ok(file)
}

fn load_report(path: &Path) -> io::Result<Option<Report>> {
    let file = match job_file(path, false, false) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    bounded_json(&file).map(Some)
}

fn bounded_json<T: serde::de::DeserializeOwned>(file: &File) -> io::Result<T> {
    const MAX_DOCUMENT: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DOCUMENT {
        return Err(invalid("Job JSON exceeds size limit"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn distinct(files: &[&File]) -> io::Result<()> {
    let ids = files
        .iter()
        .map(|f| same_file::Handle::from_file(f.try_clone()?))
        .collect::<io::Result<Vec<_>>>()?;
    if ids.iter().enumerate().any(|(i, x)| ids[..i].contains(x)) {
        return Err(invalid("A source or job file aliases another endpoint"));
    }
    Ok(())
}
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

pub fn run(
    input: &Input,
    dir: &Path,
    options: Options,
    resume: bool,
    cancel: &AtomicBool,
    progress: impl FnMut(Progress),
) -> io::Result<Report> {
    let source = open_source(input, true)?;
    write_image(input, source.into(), dir, options, resume, cancel, progress)
}
pub fn run_relocated(
    input: &Input,
    dir: &Path,
    target_bytes: u64,
    options: Options,
    resume: bool,
    cancel: &AtomicBool,
    progress: impl FnMut(Progress),
) -> io::Result<Report> {
    if options.recovery {
        return Err(invalid(
            "GPT relocation requires readable source metadata and data",
        ));
    }
    checkpoint(cancel)?;
    let mut source = open_source(input, true)?;
    let gpt = mc_inspect::GptRelocation::prepare(
        &mut source.reader,
        source.stamp.bytes,
        target_bytes,
        source.stamp.sector,
    )?;
    write_image(
        input,
        TransferSource {
            original: source,
            gpt: Some(gpt),
        },
        dir,
        options,
        resume,
        cancel,
        progress,
    )
}

#[cfg(test)]
fn run_with(
    input: &Input,
    source: Source,
    dir: &Path,
    options: Options,
    resume: bool,
    cancel: &AtomicBool,
    progress: impl FnMut(Progress),
) -> io::Result<Report> {
    write_image(input, source.into(), dir, options, resume, cancel, progress)
}

fn write_image(
    input: &Input,
    source: TransferSource,
    dir: &Path,
    options: Options,
    resume: bool,
    cancel: &AtomicBool,
    progress: impl FnMut(Progress),
) -> io::Result<Report> {
    local_path(dir)?;
    options.validate(source.original.stamp.sector)?;
    checkpoint(cancel)?;
    if !resume {
        fs::create_dir(dir)?
    }
    if !fs::symlink_metadata(dir)?.is_dir() {
        return Err(invalid("Expected a local job directory"));
    }
    let path = dir.join("image.raw");
    let mut target = ImageTarget {
        file: job_file(&path, !resume, true)?,
        path,
    };
    let job = Journal::open(dir, resume)?;
    if let Some(f) = source.original.backing_file.as_ref() {
        distinct(&[f, &target.file, &job.manifest, &job.map])?;
    } else {
        distinct(&[&target.file, &job.manifest, &job.map])?;
    }
    transfer(input, source, &mut target, job, options, cancel, progress)
}

struct Journal<'a> {
    dir: &'a Path,
    resume: bool,
    manifest: File,
    map: File,
}

impl<'a> Journal<'a> {
    fn open(dir: &'a Path, resume: bool) -> io::Result<Self> {
        let manifest = job_file(&dir.join("job.json"), !resume, true)?;
        manifest.try_lock().map_err(io::Error::other)?;
        let map = job_file(&dir.join("map.jsonl"), !resume, true)?;
        distinct(&[&manifest, &map])?;
        Ok(Self {
            dir,
            resume,
            manifest,
            map,
        })
    }
}

trait Target: Read + Seek + Durable {
    type Snapshot;
    const REPORT_DEVICE_IDENTITY: bool = false;
    fn identity(&self) -> io::Result<String>;
    fn length(&self) -> io::Result<u64>;
    fn snapshot(&self, total: u64) -> io::Result<Self::Snapshot>;
    fn check_snapshot(&self, total: u64, snapshot: Self::Snapshot) -> io::Result<()>;
}

struct ImageTarget {
    file: File,
    path: std::path::PathBuf,
}

impl Read for ImageTarget {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.file.read(bytes)
    }
}

impl Write for ImageTarget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for ImageTarget {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

impl Durable for ImageTarget {
    fn durable(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}

impl Target for ImageTarget {
    type Snapshot = std::time::SystemTime;
    fn identity(&self) -> io::Result<String> {
        Ok(file_id(&self.file)?.0)
    }
    fn length(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
    fn snapshot(&self, total: u64) -> io::Result<Self::Snapshot> {
        let metadata = self.file.metadata()?;
        if metadata.len() != total {
            return Err(invalid("Destination length differs from source"));
        }
        metadata.modified()
    }
    fn check_snapshot(&self, total: u64, modified: Self::Snapshot) -> io::Result<()> {
        let named = job_file(&self.path, false, false)?;
        let metadata = named.metadata()?;
        if file_id(&named)? != (self.identity()?, 1)
            || metadata.len() != total
            || metadata.modified()? != modified
        {
            return Err(invalid("Destination file changed during verification"));
        }
        Ok(())
    }
}

struct TransferSource {
    original: Source,
    gpt: Option<mc_inspect::GptRelocation>,
}

impl From<Source> for TransferSource {
    fn from(original: Source) -> Self {
        Self {
            original,
            gpt: None,
        }
    }
}

fn transfer<T: Target>(
    input: &Input,
    stream_source: TransferSource,
    target: &mut T,
    job: Journal<'_>,
    options: Options,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> io::Result<Report> {
    let Journal {
        dir,
        resume,
        mut manifest,
        mut map,
    } = job;
    let report_path = dir.join("report.json");
    let previous_report = load_report(&report_path)?;
    let TransferSource {
        original: mut source,
        gpt,
    } = stream_source;
    let relocated_bytes = gpt.as_ref().map(mc_inspect::GptRelocation::target_bytes);
    let actual = Manifest {
        version: 1,
        input: input.clone(),
        source: source.stamp.clone(),
        options,
        target_id: target.identity()?,
        map_id: file_id(&map)?.0,
        relocated_gpt_target_bytes: relocated_bytes,
    };
    if resume {
        if bounded_json::<Manifest>(&manifest)? != actual {
            return Err(invalid("Job identity, geometry or options changed"));
        }
    } else {
        serde_json::to_writer(&mut manifest, &actual)?;
        manifest.sync_all()?;
        sync_dir(dir)?
    }
    let total = relocated_bytes.unwrap_or(source.stamp.bytes);
    let sector = source.stamp.sector;
    let mut buffer = vec![0; options.block as usize];
    if target.length()? > total {
        return Err(invalid("Target is longer than source"));
    }
    let mut view: Box<dyn mc_device::Reader + '_> = match gpt.as_ref() {
        Some(gpt) => Box::new(gpt.reader(&mut *source.reader)),
        None => Box::new(&mut *source.reader),
    };
    let mut offset = 0;
    let mut missing = 0;
    let mut stream = Sha256::new();
    // Replay only complete lines. A complete malformed record is a hard failure.
    let mut reader = BufReader::new(map.try_clone()?);
    let mut valid_end = 0;
    let mut line = Vec::new();
    loop {
        checkpoint(cancel)?;
        line.clear();
        let cap = options.block as u64 / u64::from(sector) * 12 + 512;
        let count = reader.by_ref().take(cap).read_until(b'\n', &mut line)?;
        if count == 0 {
            break;
        }
        if count as u64 == cap {
            return Err(invalid("Oversized checkpoint"));
        }
        if line.last() != Some(&b'\n') {
            break;
        }
        let frame: Frame<Record> = serde_json::from_slice(&line)?;
        let rec = frame.record;
        if digest(&serde_json::to_vec(&rec)?) != frame.checksum {
            return Err(invalid("Checkpoint metadata checksum mismatch"));
        }
        let end = offset + (total - offset).min(options.block as u64);
        if rec.start != offset || rec.end != end || end <= offset {
            return Err(invalid("Invalid checkpoint sequence"));
        }
        let n = (end - offset) as usize;
        if rec
            .missing
            .iter()
            .any(|x| *x as usize >= n / sector as usize)
            || rec.missing.windows(2).any(|v| v[0] >= v[1])
            || (!options.recovery && !rec.missing.is_empty())
        {
            return Err(invalid("Invalid recovery map"));
        }
        let buf = &mut buffer[..n];
        read_at(target, offset, buf)?;
        if digest(buf) != rec.sha256 {
            return Err(invalid("Checkpoint target hash mismatch"));
        }
        // Reconfirm healthy source spans; already unreadable sectors are explicitly zero-filled.
        let mut from = 0;
        for hole in rec
            .missing
            .iter()
            .map(|x| *x as usize)
            .chain(std::iter::once(n / sector as usize))
        {
            let to = hole * sector as usize;
            if to > from {
                read_at(&mut *view, offset + from as u64, &mut buf[from..to])?
            }
            if to < n {
                buf[to..to + sector as usize].fill(0);
                from = to + sector as usize
            }
        }
        if digest(buf) != rec.sha256 {
            return Err(invalid("Checkpoint source hash mismatch"));
        }
        stream.update(&*buf);
        missing += rec.missing.len() as u64;
        offset = end;
        valid_end += count as u64;
        progress(Progress {
            phase: "resume_check".into(),
            bytes: offset,
            total,
            missing_sectors: missing,
        });
    }
    drop(reader);
    if previous_report.is_some() && (!resume || offset != total) {
        return Err(invalid(
            "Existing report conflicts with an incomplete job journal",
        ));
    }
    map.set_len(valid_end)?;
    map.seek(SeekFrom::Start(valid_end))?;
    target.seek(SeekFrom::Start(offset))?;
    while offset < total {
        checkpoint(cancel)?;
        let n = (total - offset).min(options.block as u64) as usize;
        let buf = &mut buffer[..n];
        let holes = acquire(&mut *view, offset, buf, sector, options.recovery, cancel)?;
        let rec = Record {
            start: offset,
            end: offset + n as u64,
            sha256: digest(buf),
            missing: holes,
        };
        commit(target, &mut map, buf, &rec)?;
        stream.update(buf);
        offset = rec.end;
        missing += rec.missing.len() as u64;
        progress(Progress {
            phase: "copy".into(),
            bytes: offset,
            total,
            missing_sectors: missing,
        });
    }
    let expected = format!("{:x}", stream.finalize());
    let target_snapshot = target.snapshot(total)?;
    let actual_hash = hash_reader(target, total, &mut buffer, cancel, |bytes| {
        progress(Progress {
            phase: "verify".into(),
            bytes,
            total,
            missing_sectors: missing,
        })
    })?;
    if actual_hash != expected {
        return Err(invalid("Destination SHA-256 mismatch"));
    }
    target.check_snapshot(total, target_snapshot)?;
    drop(view);
    check_source_file(input, &source)?;
    checkpoint(cancel)?;
    let report = Report {
        outcome: if missing == 0 { "verified" } else { "partial" }.into(),
        bytes: total,
        missing_sectors: missing,
        sha256: expected,
        verified_bytes: total - missing * u64::from(sector),
        target_reread_bytes: total,
        logical_sector: sector,
        schema_version: 1,
        source_identity: source.stamp.identity,
        destination_identity: T::REPORT_DEVICE_IDENTITY.then_some(actual.target_id),
        gpt_relocated_from_bytes: relocated_bytes.map(|_| source.stamp.bytes),
    };
    if let Some(previous) = previous_report {
        if previous != report || load_report(&report_path)?.as_ref() != Some(&report) {
            return Err(invalid("Existing report differs from verified result"));
        }
    } else {
        let mut temporary = tempfile::Builder::new()
            .prefix("report-")
            .tempfile_in(dir)?;
        serde_json::to_writer_pretty(&mut temporary, &report)?;
        temporary.as_file().sync_all()?;
        checkpoint(cancel)?;
        temporary
            .persist_noclobber(&report_path)
            .map_err(|error| error.error)?;
        sync_dir(dir)?;
    }
    Ok(report)
}
trait Durable: Write {
    fn durable(&mut self) -> io::Result<()>;
}
impl Durable for File {
    fn durable(&mut self) -> io::Result<()> {
        self.sync_all()
    }
}
fn commit(
    target: &mut impl Durable,
    map: &mut impl Durable,
    data: &[u8],
    rec: &Record,
) -> io::Result<()> {
    target.write_all(data)?;
    target.durable()?;
    let checksum = digest(&serde_json::to_vec(rec)?);
    serde_json::to_writer(
        &mut *map,
        &Frame {
            record: rec,
            checksum,
        },
    )?;
    map.write_all(b"\n")?;
    map.durable()
}
fn hash_reader<R: Read + Seek + ?Sized>(
    r: &mut R,
    total: u64,
    b: &mut [u8],
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> io::Result<String> {
    r.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut offset = 0;
    while offset < total {
        checkpoint(cancel)?;
        let n = (total - offset).min(b.len() as u64) as usize;
        r.read_exact(&mut b[..n])?;
        hash.update(&b[..n]);
        offset += n as u64;
        progress(offset)
    }
    Ok(format!("{:x}", hash.finalize()))
}
// A matching prefix digest alone cannot detect a renamed, replaced or growing file.
fn check_source_file(input: &Input, source: &Source) -> io::Result<()> {
    if let (Input::Image { path, .. }, Some(opened)) = (input, &source.backing_file) {
        let named = mc_device::open_image_file(path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Source file changed during the operation: {error}"),
            )
        })?;
        let metadata = opened.metadata()?;
        let modified = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos()
            .to_string();
        if file_id(opened)?.0 != file_id(&named)?.0
            || metadata.len() != source.stamp.bytes
            || Some(modified) != source.stamp.modified_ns
        {
            return Err(invalid("Source file changed during the operation"));
        }
    }
    Ok(())
}
pub fn verify(
    input: &Input,
    expected: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> io::Result<String> {
    verify_with_total(input, expected, cancel, |bytes, _| progress(bytes))
}

pub fn verify_with_total(
    input: &Input,
    expected: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> io::Result<String> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("Expected a SHA-256 hex digest"));
    }
    let mut src = open_source(input, true)?;
    let total = src.stamp.bytes;
    let hash = hash_reader(
        &mut *src.reader,
        total,
        &mut vec![0; 1024 * 1024],
        cancel,
        |bytes| progress(bytes, total),
    )?;
    check_source_file(input, &src)?;
    checkpoint(cancel)?;
    if !hash.eq_ignore_ascii_case(expected) {
        return Err(invalid("SHA-256 mismatch"));
    }
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;
    use tempfile::TempDir;
    struct TestJob {
        _temp: TempDir,
        src: Input,
        out: PathBuf,
    }
    impl TestJob {
        fn new() -> Self {
            let temp = TempDir::new().unwrap();
            let path = temp.path().join("source.raw");
            let out = temp.path().join("result");
            fs::write(
                &path,
                (0..8192).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
            )
            .unwrap();
            Self {
                _temp: temp,
                src: Input::Image { path, sector: 512 },
                out,
            }
        }
        fn run(
            &self,
            resume: bool,
            c: &AtomicBool,
            progress: impl FnMut(Progress),
        ) -> io::Result<Report> {
            run(
                &self.src,
                &self.out,
                Options {
                    block: 2048,
                    recovery: false,
                },
                resume,
                c,
                progress,
            )
        }
    }
    #[test]
    fn full_image_and_verify() {
        let j = TestJob::new();
        let c = AtomicBool::new(false);
        let r = j.run(false, &c, |_| {}).unwrap();
        assert_eq!(r.outcome, "verified");
        let i = Input::Image {
            path: j.out.join("image.raw"),
            sector: 512,
        };
        assert_eq!(verify(&i, &r.sha256, &c, |_| {}).unwrap(), r.sha256);
        assert!(verify(&i, &"0".repeat(64), &c, |_| {}).is_err());
        assert_eq!(j.run(true, &c, |_| {}).unwrap(), r)
    }
    #[test]
    fn report_created_while_verifying_is_preserved() {
        let j = TestJob::new();
        let report = j.out.join("report.json");
        let result = j.run(false, &AtomicBool::new(false), |progress| {
            if progress.phase == "verify" && progress.bytes == progress.total {
                fs::write(&report, b"external report").unwrap();
            }
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(report).unwrap(), b"external report");
    }

    #[test]
    fn completed_job_with_missing_checkpoints_refuses_to_resume() {
        let j = TestJob::new();
        let cancel = AtomicBool::new(false);
        let original = j.run(false, &cancel, |_| {}).unwrap();
        fs::write(j.out.join("map.jsonl"), b"").unwrap();
        let image = j.out.join("image.raw");
        let current = vec![0xa5; original.bytes as usize];
        fs::write(&image, &current).unwrap();
        let result = j.run(true, &cancel, |_| {});
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("incomplete job journal")
        );
        assert_eq!(fs::read(image).unwrap(), current);
    }

    #[cfg(unix)]
    #[test]
    fn completed_job_rejects_report_symlink_even_if_content_matches() {
        let j = TestJob::new();
        let cancel = AtomicBool::new(false);
        j.run(false, &cancel, |_| {}).unwrap();
        let report = j.out.join("report.json");
        let renamed = j.out.join("external.json");
        fs::rename(&report, &renamed).unwrap();
        std::os::unix::fs::symlink(&renamed, &report).unwrap();
        assert!(j.run(true, &cancel, |_| {}).is_err());
        assert!(renamed.exists());
    }

    #[cfg(unix)]
    #[test]
    fn source_fifo_replacement_returns_error_without_blocking() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let j = TestJob::new();
        let Input::Image { path, .. } = &j.src else {
            unreachable!()
        };
        let expected = digest(&fs::read(path).unwrap());
        let failure = verify(&j.src, &expected, &AtomicBool::new(false), |_| {
            fs::remove_file(path).unwrap();
            let name = CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: name refers to a test-owned pathname in a temporary directory.
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        })
        .unwrap_err();
        assert!(failure.to_string().contains("Source file changed"));
    }

    #[test]
    fn existing_dir_untouched() {
        let j = TestJob::new();
        fs::create_dir(&j.out).unwrap();
        fs::write(j.out.join("image.raw"), b"keep").unwrap();
        assert!(j.run(false, &AtomicBool::new(false), |_| {}).is_err());
        assert_eq!(fs::read(j.out.join("image.raw")).unwrap(), b"keep")
    }
    #[test]
    fn cancel_resume_torn_record() {
        let j = TestJob::new();
        let c = AtomicBool::new(false);
        let err = j
            .run(false, &c, |p| {
                if p.bytes == 2048 {
                    c.store(true, Ordering::Relaxed)
                }
            })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(!j.out.join("report.json").exists());
        OpenOptions::new()
            .append(true)
            .open(j.out.join("map.jsonl"))
            .unwrap()
            .write_all(b"{xx")
            .unwrap();
        c.store(false, Ordering::Relaxed);
        assert_eq!(j.run(true, &c, |_| {}).unwrap().verified_bytes, 8192)
    }
    #[test]
    fn resume_replaces_written_but_uncommitted_tail() {
        let j = TestJob::new();
        let c = AtomicBool::new(false);
        let error = j
            .run(false, &c, |progress| {
                if progress.phase == "copy" && progress.bytes == 2048 {
                    c.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        let target = j.out.join("image.raw");
        let mut tail = OpenOptions::new().append(true).open(&target).unwrap();
        tail.write_all(&[0xa5; 3072]).unwrap();
        tail.sync_all().unwrap();
        drop(tail);
        c.store(false, Ordering::Relaxed);
        let report = j.run(true, &c, |_| {}).unwrap();
        let Input::Image { path, .. } = &j.src else {
            unreachable!()
        };
        assert_eq!(report.verified_bytes, 8192);
        assert_eq!(fs::read(target).unwrap(), fs::read(path).unwrap());
    }
    #[test]
    fn final_cancel_prevents_publication() {
        let j = TestJob::new();
        let cancel = AtomicBool::new(false);
        let err = j
            .run(false, &cancel, |p| {
                if p.phase == "verify" && p.bytes == p.total {
                    cancel.store(true, Ordering::Relaxed)
                }
            })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(!j.out.join("report.json").exists());
    }
    // Windows file sharing already excludes external writers to these open files.
    #[cfg(unix)]
    #[test]
    fn target_growth_during_reread_is_not_verified() {
        let j = TestJob::new();
        let err = j
            .run(false, &AtomicBool::new(false), |p| {
                if p.phase == "verify" && p.bytes == p.total {
                    OpenOptions::new()
                        .append(true)
                        .open(j.out.join("image.raw"))
                        .unwrap()
                        .write_all(&[7; 512])
                        .unwrap();
                }
            })
            .unwrap_err();
        assert!(err.to_string().contains("Destination file changed"));
        assert!(!j.out.join("report.json").exists());
    }
    #[cfg(unix)]
    #[test]
    fn standalone_verify_rejects_replaced_path() {
        let j = TestJob::new();
        let Input::Image { path, .. } = &j.src else {
            unreachable!()
        };
        let expected = digest(&fs::read(path).unwrap());
        let replacement = path.with_extension("replacement");
        fs::write(&replacement, [0u8; 8192]).unwrap();
        let err = verify(&j.src, &expected, &AtomicBool::new(false), |_| {
            fs::rename(&replacement, path).unwrap();
        })
        .unwrap_err();
        assert!(err.to_string().contains("Source file changed"));
    }
    #[test]
    fn detects_changed_checkpoint() {
        let j = TestJob::new();
        let c = AtomicBool::new(false);
        j.run(false, &c, |_| {}).unwrap();
        OpenOptions::new()
            .write(true)
            .open(j.out.join("image.raw"))
            .unwrap()
            .write_all(b"x")
            .unwrap();
        assert!(j.run(true, &c, |_| {}).is_err())
    }
    #[test]
    fn wrong_source_cannot_resume() {
        let j = TestJob::new();
        let k = TestJob::new();
        let c = AtomicBool::new(false);
        j.run(false, &c, |_| {}).unwrap();
        assert!(
            run(
                &k.src,
                &j.out,
                Options {
                    block: 2048,
                    recovery: false
                },
                true,
                &c,
                |_| {}
            )
            .is_err()
        )
    }
    #[test]
    fn concurrent_job_is_blocked() {
        let j = TestJob::new();
        let c = AtomicBool::new(false);
        j.run(false, &c, |_| {}).unwrap();
        let lock = File::open(j.out.join("job.json")).unwrap();
        lock.lock().unwrap();
        assert!(j.run(true, &c, |_| {}).is_err())
    }
    #[test]
    fn hard_link_cannot_receive_writes() {
        let j = TestJob::new();
        let c = AtomicBool::new(false);
        j.run(false, &c, |_| {}).unwrap();
        fs::remove_file(j.out.join("image.raw")).unwrap();
        let Input::Image { path, .. } = &j.src else {
            unreachable!()
        };
        let before = fs::read(path).unwrap();
        fs::hard_link(path, j.out.join("image.raw")).unwrap();
        assert!(j.run(true, &c, |_| {}).is_err());
        assert_eq!(fs::read(path).unwrap(), before)
    }
    #[test]
    fn rejects_edit_to_checkpoint_metadata() {
        let j = TestJob::new();
        let c = AtomicBool::new(false);
        j.run(false, &c, |_| {}).unwrap();
        let map = j.out.join("map.jsonl");
        let text = fs::read_to_string(&map).unwrap();
        let altered = text.replacen("\"missing\":[]", "\"missing\":[0]", 1);
        assert_ne!(altered, text);
        fs::write(map, altered).unwrap();
        assert!(
            j.run(true, &c, |_| {})
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
    }
    struct Fault {
        inner: Cursor<Vec<u8>>,
        sector: u64,
    }
    impl Seek for Fault {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.inner.seek(p)
        }
    }
    impl Read for Fault {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let at = self.inner.position();
            if at < self.sector + 512 && at + b.len() as u64 > self.sector {
                return Err(io::Error::from_raw_os_error(if cfg!(windows) {
                    23
                } else {
                    5
                }));
            }
            self.inner.read(b)
        }
    }
    #[test]
    fn media_error_stops_strict_and_marks_recovery() {
        let j = TestJob::new();
        let c = AtomicBool::new(false);
        let fake = || Source {
            reader: Box::new(Fault {
                inner: Cursor::new(vec![12; 8192]),
                sector: 512,
            }),
            backing_file: None,
            stamp: Stamp {
                bytes: 8192,
                sector: 512,
                modified_ns: None,
                identity: "fake".into(),
            },
        };
        assert!(
            run_with(
                &j.src,
                fake(),
                &j.out,
                Options::default(),
                false,
                &c,
                |_| {}
            )
            .is_err()
        );
        let out = j.out.with_extension("recover");
        let r = run_with(
            &j.src,
            fake(),
            &out,
            Options {
                recovery: true,
                ..Default::default()
            },
            false,
            &c,
            |_| {},
        )
        .unwrap();
        assert_eq!(
            (r.outcome.as_str(), r.missing_sectors, r.verified_bytes),
            ("partial", 1, 7680)
        );
        let raw = fs::read(out.join("image.raw")).unwrap();
        assert_eq!(raw[512..1024], [0; 512]);
        assert_eq!(raw[1024], 12)
    }
    #[derive(Default)]
    struct Sink {
        data: Vec<u8>,
        fail_write: bool,
        fail_flush: bool,
    }
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                return Err(io::Error::other("write fault"));
            }
            self.data.write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Durable for Sink {
        fn durable(&mut self) -> io::Result<()> {
            if self.fail_flush {
                Err(io::Error::other("flush fault"))
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn failed_target_never_commits() {
        for (fail_write, fail_flush) in [(true, false), (false, true)] {
            let mut target = Sink {
                fail_write,
                fail_flush,
                ..Default::default()
            };
            let mut map = Sink::default();
            assert!(
                commit(
                    &mut target,
                    &mut map,
                    b"x",
                    &Record {
                        start: 0,
                        end: 1,
                        missing: vec![],
                        sha256: digest(b"x")
                    }
                )
                .is_err()
            );
            assert!(map.data.is_empty())
        }
    }
    #[test]
    fn journal_failure_is_terminal() {
        let mut map = Sink {
            fail_flush: true,
            ..Default::default()
        };
        assert!(
            commit(
                &mut Sink::default(),
                &mut map,
                b"x",
                &Record {
                    start: 0,
                    end: 1,
                    missing: vec![],
                    sha256: digest(b"x")
                }
            )
            .is_err()
        )
    }
    #[test]
    fn dry_check_opens_only_reader() {
        let j = TestJob::new();
        let s = open_source(&j.src, true).unwrap();
        Options::default().validate(s.stamp.sector).unwrap();
        assert!(!j.out.exists())
    }
}

#[cfg(test)]
mod destination_tests {
    use super::*;
    use std::{cell::Cell, io::Cursor};

    struct FixedTarget {
        device: Cursor<Vec<u8>>,
        id: String,
        write_until: Option<u64>,
        verifying: Cell<bool>,
        corrupt_reread: bool,
    }

    impl Read for FixedTarget {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let n = self.device.read(bytes)?;
            if self.corrupt_reread && self.verifying.get() && n > 0 {
                bytes[0] ^= 1;
            }
            Ok(n)
        }
    }
    impl Seek for FixedTarget {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.device.seek(position)
        }
    }
    impl Write for FixedTarget {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(limit) = self.write_until {
                let count = limit
                    .saturating_sub(self.device.position())
                    .min(bytes.len() as u64) as usize;
                if count == 0 {
                    return Err(io::Error::other("Injected partial write"));
                }
                return self.device.write(&bytes[..count]);
            }
            self.device.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Durable for FixedTarget {
        fn durable(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Target for FixedTarget {
        type Snapshot = ();
        const REPORT_DEVICE_IDENTITY: bool = true;
        fn identity(&self) -> io::Result<String> {
            Ok(self.id.clone())
        }
        fn length(&self) -> io::Result<u64> {
            Ok(self.device.get_ref().len() as u64)
        }
        fn snapshot(&self, _: u64) -> io::Result<()> {
            self.verifying.set(true);
            Ok(())
        }
        fn check_snapshot(&self, _: u64, (): ()) -> io::Result<()> {
            Ok(())
        }
    }

    fn fixture() -> (tempfile::TempDir, Input, Vec<u8>, FixedTarget) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.raw");
        let payload: Vec<_> = (0..2048).map(|value| (value % 251) as u8).collect();
        fs::write(&source, &payload).unwrap();
        (
            dir,
            Input::Image {
                path: source,
                sector: 512,
            },
            payload,
            FixedTarget {
                device: Cursor::new(vec![0xD0; 2048]),
                id: "original-target".into(),
                write_until: None,
                verifying: Cell::new(false),
                corrupt_reread: false,
            },
        )
    }

    fn copy(
        dir: &Path,
        source: &Input,
        target: &mut FixedTarget,
        resume: bool,
    ) -> io::Result<Report> {
        transfer(
            source,
            open_source(source, true)?.into(),
            target,
            Journal::open(dir, resume)?,
            Options {
                block: 512,
                recovery: false,
            },
            &AtomicBool::new(false),
            |_| (),
        )
    }

    #[test]
    fn partial_write_resumes_only_on_same_fixed_destination() {
        let (dir, source, bytes, mut target) = fixture();
        target.write_until = Some(512 + 128);
        assert!(copy(dir.path(), &source, &mut target, false).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("map.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert!(!dir.path().join("report.json").exists());
        target.write_until = None;
        target.id = "replacement-target".into();
        let after_failure = target.device.get_ref().clone();
        assert!(copy(dir.path(), &source, &mut target, true).is_err());
        assert_eq!(target.device.get_ref(), &after_failure);
        target.id = "original-target".into();
        let report = copy(dir.path(), &source, &mut target, true).unwrap();
        assert_eq!(target.device.get_ref(), &bytes);
        assert_eq!(report.sha256, digest(&bytes));
        assert_eq!(
            report.destination_identity.as_deref(),
            Some("original-target")
        );
    }

    #[test]
    fn failed_destination_reread_never_creates_a_report() {
        let (dir, source, _, mut target) = fixture();
        target.corrupt_reread = true;
        assert!(
            copy(dir.path(), &source, &mut target, false)
                .unwrap_err()
                .to_string()
                .contains("SHA-256")
        );
        assert!(!dir.path().join("report.json").exists());
    }
}
