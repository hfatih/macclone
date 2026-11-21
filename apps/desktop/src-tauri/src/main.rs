#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use mc_device::{Disk, Input, open_source};
use mc_engine::{Options, Progress, Report};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{AppHandle, Emitter, Manager, State};
type Api<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Clone, Copy, Serialize, Default)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    #[default]
    Idle,
    Running,
    Completed,
    Partial,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Image,
    Recovery,
    Verify,
}

#[derive(Clone, Serialize, Default)]
struct Status {
    id: u64,
    revision: u64,
    running: bool,
    outcome: Outcome,
    operation: Option<Operation>,
    progress: Progress,
    report: Option<Report>,
    message: Option<String>,
}
#[derive(Default)]
struct Jobs {
    record: Status,
    cancel: Arc<AtomicBool>,
}
#[derive(Deserialize)]
struct Request {
    input: Input,
    directory: PathBuf,
    options: Options,
    resume: bool,
    target_bytes: Option<u64>,
}

#[derive(Serialize)]
struct DesktopPlan {
    blockers: Vec<String>,
    gpt_fit: Option<GptFit>,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum GptFit {
    Fits {
        target_bytes: u64,
        minimum_bytes: u64,
    },
    Blocked {
        reason: String,
    },
}

fn check_gpt_fit(source: &Disk, target: &Disk) -> io::Result<mc_inspect::GptRelocation> {
    if source.logical != target.logical {
        return Err(mc_device::invalid(
            "Disklerin mantıksal sektör boyutları farklı.",
        ));
    }
    if source.offline != Some(true) {
        return Err(mc_device::invalid(
            "GPT denetimi için kaynak disk çevrimdışı olmalıdır.",
        ));
    }
    let id = source
        .id
        .clone()
        .ok_or_else(|| mc_device::invalid("Kaynak disk kimliği alınamadı."))?;
    let input = Input::Disk {
        number: source.number,
        id,
    };
    let mut opened = open_source(&input, true)?;
    if opened.stamp.bytes != source.bytes || opened.stamp.sector != source.logical {
        return Err(mc_device::invalid(
            "Kaynak geometrisi değişti. Diskleri yenileyip planı tekrar denetleyin.",
        ));
    }
    mc_inspect::GptRelocation::prepare(
        &mut opened.reader,
        opened.stamp.bytes,
        target.bytes,
        opened.stamp.sector,
    )
}
#[tauri::command]
async fn list_disks() -> Api<Vec<Disk>> {
    tauri::async_runtime::spawn_blocking(mc_device::disks)
        .await
        .map_err(err)?
        .map_err(err)
}

#[cfg(windows)]
fn run_storage_ps(script: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(err)?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Err(if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!(
                "PowerShell komutu başarısız oldu (kod {:?}).",
                out.status.code()
            )
        })
    }
}

#[cfg(not(windows))]
fn run_storage_ps(_script: &str) -> Result<(), String> {
    Err("Disk hazırlama işlemi yalnızca Windows'ta desteklenir.".into())
}

fn find_safe_disk(number: u32, expected_id: Option<&str>) -> Result<Disk, String> {
    let disks = mc_device::disks().map_err(err)?;
    let disk = disks
        .into_iter()
        .find(|d| d.number == number)
        .ok_or_else(|| format!("Disk {number} bulunamadı. Envanteri yenileyip tekrar deneyin."))?;
    if disk.id.as_deref() != expected_id {
        return Err("Disk kimliği değişti. Envanteri yenileyip yeniden seçin.".into());
    }
    if disk.boot != Some(false) || disk.system != Some(false) || disk.clustered != Some(false) {
        return Err(format!(
            "Güvenlik engeli: Disk {number} sistem, önyükleme veya küme diski olduğu için değiştirilemez."
        ));
    }
    Ok(disk)
}

#[tauri::command]
async fn set_disk_offline(number: u32, id: Option<String>, offline: bool) -> Api<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _ = find_safe_disk(number, id.as_deref())?;
        let flag = if offline { "$true" } else { "$false" };
        run_storage_ps(&format!(
            "$ErrorActionPreference='Stop'; Set-Disk -Number {number} -IsOffline {flag}"
        ))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn prepare_target_disk(number: u32, id: Option<String>) -> Api<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let disk = find_safe_disk(number, id.as_deref())?;
        if disk.readonly == Some(true) {
            return Err(format!(
                "Disk {number} salt okunur (read-only) durumda; sıfırlanamaz."
            ));
        }
        let script = format!(
            "$ErrorActionPreference='Stop'; \
             Set-Disk -Number {number} -IsOffline $false; \
             if ((Get-Disk -Number {number}).PartitionStyle -ne 'RAW') {{ \
               Clear-Disk -Number {number} -RemoveData -RemoveOEM -Confirm:$false \
             }}; \
             Set-Disk -Number {number} -IsOffline $true"
        );
        run_storage_ps(&script)
    })
    .await
    .map_err(err)?
}
#[tauri::command]
async fn inspect_disk(input: Input) -> Api<mc_inspect::Layout> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut s = open_source(&input, false)?;
        mc_inspect::inspect(&mut s.reader, s.stamp.bytes, s.stamp.sector)
    })
    .await
    .map_err(err)?
    .map_err(err)
}
#[tauri::command]
async fn plan_clone(
    source: u32,
    target: u32,
    source_id: Option<String>,
    target_id: Option<String>,
) -> Api<DesktopPlan> {
    tauri::async_runtime::spawn_blocking(move || {
        let report = mc_device::plan_disks(source, target).map_err(err)?;
        if report.source.id != source_id || report.target.id != target_id {
            return Err("Disk seçimi değişti. Envanteri yenileyip yeniden seçin.".into());
        }
        let gpt_fit = (report.source.bytes > report.target.bytes).then(|| {
            match check_gpt_fit(&report.source, &report.target) {
                Ok(gpt) => GptFit::Fits {
                    target_bytes: gpt.target_bytes(),
                    minimum_bytes: gpt.minimum_target_bytes(),
                },
                Err(error) => GptFit::Blocked {
                    reason: error.to_string(),
                },
            }
        });
        Ok(DesktopPlan {
            blockers: report.blockers,
            gpt_fit,
        })
    })
    .await
    .map_err(err)?
}
#[tauri::command]
fn get_operation_status(state: State<'_, Mutex<Jobs>>) -> Api<Status> {
    Ok(state.lock().map_err(err)?.record.clone())
}
#[tauri::command]
fn cancel_operation(state: State<'_, Mutex<Jobs>>, id: u64) -> Api<()> {
    let jobs = state.lock().map_err(err)?;
    if jobs.record.id != id || !jobs.record.running {
        return Err("Etkin işlem değişti".into());
    }
    jobs.cancel.store(true, Ordering::Relaxed);
    Ok(())
}
fn job<F>(app: AppHandle, operation: Operation, f: F) -> Api<u64>
where
    F: FnOnce(&AtomicBool, &dyn Fn(Progress)) -> io::Result<(Option<Report>, String)>
        + Send
        + 'static,
{
    let handle = app.state::<Mutex<Jobs>>();
    let mut jobs = handle.lock().map_err(err)?;
    if jobs.record.running {
        return Err("Bir işlem zaten çalışıyor".into());
    }
    let id = jobs.record.id + 1;
    let cancel = Arc::new(AtomicBool::new(false));
    jobs.cancel = cancel.clone();
    jobs.record = Status {
        id,
        running: true,
        outcome: Outcome::Running,
        operation: Some(operation),
        ..Status::default()
    };
    let initial = jobs.record.clone();
    drop(jobs);
    let _ = app.emit("operation", initial);
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<Jobs>>();
        let publish = Mutex::new(std::time::Instant::now());
        let progress = |p: Progress| {
            if let Ok(mut j) = state.lock() {
                j.record.revision += 1;
                j.record.progress = p;
                if let Ok(mut clock) = publish.lock()
                    && clock.elapsed().as_millis() >= 150
                {
                    *clock = std::time::Instant::now();
                    let _ = app.emit("operation", &j.record);
                }
            }
        };
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&cancel, &progress)));
        if let Ok(mut j) = state.lock() {
            j.record.revision += 1;
            j.record.running = false;
            match res {
                Ok(Ok((r, msg))) => {
                    j.record.outcome =
                        if r.as_ref().is_some_and(|report| report.outcome == "partial") {
                            Outcome::Partial
                        } else {
                            Outcome::Completed
                        };
                    j.record.report = r;
                    j.record.message = Some(msg);
                }
                Ok(Err(e))
                    if e.kind() == io::ErrorKind::Interrupted && cancel.load(Ordering::Relaxed) =>
                {
                    j.record.outcome = Outcome::Cancelled;
                    j.record.message = Some("İşlem güvenli bir noktada durduruldu.".into());
                }
                Ok(Err(e)) => {
                    j.record.outcome = Outcome::Failed;
                    j.record.message = Some(e.to_string());
                }
                Err(_) => {
                    j.record.outcome = Outcome::Failed;
                    j.record.message = Some("İşlem beklenmedik şekilde durdu.".into());
                }
            }
            let _ = app.emit("operation", &j.record);
        }
    });
    Ok(id)
}
#[tauri::command]
fn create_image(app: AppHandle, request: Request) -> Api<u64> {
    let operation = if request.options.recovery {
        Operation::Recovery
    } else {
        Operation::Image
    };
    job(app, operation, move |c, p| {
        let r = match request.target_bytes {
            Some(bytes) => mc_engine::run_relocated(
                &request.input,
                &request.directory,
                bytes,
                request.options,
                request.resume,
                c,
                p,
            )?,
            None => mc_engine::run(
                &request.input,
                &request.directory,
                request.options,
                request.resume,
                c,
                p,
            )?,
        };
        Ok((Some(r), "İmaj raporu iş klasörüne kaydedildi.".into()))
    })
}
#[tauri::command]
fn verify_image(app: AppHandle, input: Input, expected: String) -> Api<u64> {
    job(app, Operation::Verify, move |c, p| {
        let hash = mc_engine::verify_with_total(&input, &expected, c, |bytes, total| {
            p(Progress {
                phase: "verify".into(),
                bytes,
                total,
                missing_sectors: 0,
            })
        })?;
        Ok((None, format!("SHA-256 eşleşti · {hash}")))
    })
}
fn partition_bounds(
    start_lba: u64,
    end_lba: u64,
    sector: u32,
    total_bytes: u64,
) -> io::Result<(u64, u64)> {
    if end_lba < start_lba {
        return Err(mc_device::invalid("Geçersiz bölüm LBA aralığı."));
    }
    let s = u64::from(sector);
    let offset = start_lba
        .checked_mul(s)
        .ok_or_else(|| mc_device::invalid("Bölüm başlangıcı taşma yaptı."))?;
    let len = end_lba
        .saturating_sub(start_lba)
        .saturating_add(1)
        .checked_mul(s)
        .ok_or_else(|| mc_device::invalid("Bölüm boyutu taşma yaptı."))?;
    if offset.checked_add(len).is_none_or(|end| end > total_bytes) {
        return Err(mc_device::invalid("Bölüm disk sınırlarını aşıyor."));
    }
    Ok((offset, len))
}

#[tauri::command]
async fn apfs_volumes(
    input: Input,
    start_lba: u64,
    end_lba: u64,
) -> Api<Vec<mc_inspect::apfs::ApfsVolumeInfo>> {
    tauri::async_runtime::spawn_blocking(move || {
        let opened = open_source(&input, false)?;
        let (offset, len) =
            partition_bounds(start_lba, end_lba, opened.stamp.sector, opened.stamp.bytes)?;
        mc_inspect::apfs::list_volumes(opened.reader, offset, len)
    })
    .await
    .map_err(err)?
    .map_err(err)
}

#[tauri::command]
async fn apfs_list_dir(
    input: Input,
    start_lba: u64,
    end_lba: u64,
    volume_index: usize,
    path: String,
) -> Api<Vec<mc_inspect::apfs::ApfsEntry>> {
    tauri::async_runtime::spawn_blocking(move || {
        let opened = open_source(&input, false)?;
        let (offset, len) =
            partition_bounds(start_lba, end_lba, opened.stamp.sector, opened.stamp.bytes)?;
        mc_inspect::apfs::list_dir(opened.reader, offset, len, volume_index, &path)
    })
    .await
    .map_err(err)?
    .map_err(err)
}

#[tauri::command]
async fn apfs_extract(
    input: Input,
    start_lba: u64,
    end_lba: u64,
    volume_index: usize,
    path: String,
    destination: PathBuf,
) -> Api<mc_inspect::apfs::ExtractSummary> {
    tauri::async_runtime::spawn_blocking(move || {
        mc_device::local_path(&destination)?;
        let opened = open_source(&input, false)?;
        let (offset, len) =
            partition_bounds(start_lba, end_lba, opened.stamp.sector, opened.stamp.bytes)?;
        mc_inspect::apfs::extract_path(
            opened.reader,
            offset,
            len,
            volume_index,
            &path,
            &destination,
        )
    })
    .await
    .map_err(err)?
    .map_err(err)
}

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(Jobs::default()))
        .invoke_handler(tauri::generate_handler![
            list_disks,
            inspect_disk,
            apfs_volumes,
            apfs_list_dir,
            apfs_extract,
            plan_clone,
            set_disk_offline,
            prepare_target_disk,
            get_operation_status,
            cancel_operation,
            create_image,
            verify_image
        ])
        .run(tauri::generate_context!())
        .expect("MacClone desktop initialization");
}
