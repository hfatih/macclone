//! Regular-file acquisition, append-only checkpoint journal and independent destination reread.
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
        if self.block < sector || self.block > 8 * 1024 * 1024 || !self.block.is_multiple_of(sector)
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
fn regular(path: &Path, new: bool) -> io::Result<File> {
    if !new && !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(invalid("Expected a regular job file"));
    }
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create_new(new);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(1);
    }
    let f = opts.open(path)?;
    if !f.metadata()?.is_file() || file_id(&f)?.1 != 1 {
        return Err(invalid("Job file is not private to this directory"));
    }
    Ok(f)
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
fn read_manifest(f: &mut File) -> io::Result<Manifest> {
    if f.metadata()?.len() > 1024 * 1024 {
        return Err(invalid("Manifest exceeds limit"));
    }
    serde_json::from_reader(f).map_err(io::Error::other)
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
    run_with(input, source, dir, options, resume, cancel, progress)
}
fn run_with(
    input: &Input,
    mut source: Source,
    dir: &Path,
    options: Options,
    resume: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> io::Result<Report> {
    local_path(dir)?;
    options.validate(source.stamp.sector)?;
    checkpoint(cancel)?;
    if !resume {
        fs::create_dir(dir)?
    }
    if !fs::symlink_metadata(dir)?.is_dir() {
        return Err(invalid("Expected a local job directory"));
    }
    let mut manifest = regular(&dir.join("job.json"), !resume)?;
    manifest.try_lock().map_err(io::Error::other)?;
    let mut target = regular(&dir.join("image.raw"), !resume)?;
    let mut map = regular(&dir.join("map.jsonl"), !resume)?;
    let mut handles = vec![&manifest, &target, &map];
    if let Some(ref f) = source.backing_file {
        handles.push(f)
    }
    distinct(&handles)?;
    let actual = Manifest {
        version: 1,
        input: input.clone(),
        source: source.stamp.clone(),
        options,
        target_id: file_id(&target)?.0,
        map_id: file_id(&map)?.0,
    };
    if resume {
        if read_manifest(&mut manifest)? != actual {
            return Err(invalid("Job identity, geometry or options changed"));
        }
    } else {
        serde_json::to_writer(&mut manifest, &actual)?;
        manifest.sync_all()?;
        sync_dir(dir)?
    }
    let total = source.stamp.bytes;
    let sector = source.stamp.sector;
    let mut buffer = vec![0; options.block as usize];
    if target.metadata()?.len() > total {
        return Err(invalid("Target is longer than source"));
    }
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
        read_at(&mut target, offset, buf)?;
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
                read_at(
                    &mut *source.reader,
                    offset + from as u64,
                    &mut buf[from..to],
                )?
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
    map.set_len(valid_end)?;
    map.seek(SeekFrom::Start(valid_end))?;
    target.seek(SeekFrom::Start(offset))?;
    while offset < total {
        checkpoint(cancel)?;
        let n = (total - offset).min(options.block as u64) as usize;
        let buf = &mut buffer[..n];
        let holes = acquire(
            &mut *source.reader,
            offset,
            buf,
            sector,
            options.recovery,
            cancel,
        )?;
        let rec = Record {
            start: offset,
            end: offset + n as u64,
            sha256: digest(buf),
            missing: holes,
        };
        commit(&mut target, &mut map, buf, &rec)?;
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
    let before_verification = target.metadata()?;
    if before_verification.len() != total {
        return Err(invalid("Destination length differs from source"));
    }
    let target_modified = before_verification.modified()?;
    let actual_hash = hash_reader(&mut target, total, &mut buffer, cancel, |bytes| {
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
    let named_target = File::open(dir.join("image.raw"))?;
    if file_id(&named_target)? != (actual.target_id, 1)
        || named_target.metadata()?.len() != total
        || named_target.metadata()?.modified()? != target_modified
    {
        return Err(invalid("Destination file changed during verification"));
    }
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
    };
    // Publish only a complete report. A resume of a completed job checks the old report.
    let path = dir.join("report.json");
    if path.try_exists()? {
        let f = File::open(path)?;
        if f.metadata()?.len() > 1024 * 1024 || serde_json::from_reader::<_, Report>(f)? != report {
            return Err(invalid("Existing report differs from verified result"));
        }
    } else {
        let tmp = dir.join("report.pending");
        // A crash can leave an unpublished temp. Do not follow it or edit it.
        if tmp.try_exists()? {
            fs::remove_file(&tmp)?
        }
        let mut f = regular(&tmp, true)?;
        serde_json::to_writer_pretty(&mut f, &report)?;
        f.sync_all()?;
        drop(f);
        fs::rename(tmp, path)?;
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
        let named = File::open(path)?;
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
    progress: impl FnMut(u64),
) -> io::Result<String> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("Expected a SHA-256 hex digest"));
    }
    let mut src = open_source(input, true)?;
    let hash = hash_reader(
        &mut *src.reader,
        src.stamp.bytes,
        &mut vec![0; 1024 * 1024],
        cancel,
        progress,
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
