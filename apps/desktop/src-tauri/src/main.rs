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
}
#[tauri::command]
async fn list_disks() -> Api<Vec<Disk>> {
    tauri::async_runtime::spawn_blocking(mc_device::disks)
        .await
        .map_err(err)?
        .map_err(err)
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
) -> Api<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let report = mc_device::plan_disks(source, target).map_err(err)?;
        if report.source.id != source_id || report.target.id != target_id {
            return Err("Disk seçimi değişti. Envanteri yenileyip yeniden seçin.".into());
        }
        Ok(report.blockers)
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
        let r = mc_engine::run(
            &request.input,
            &request.directory,
            request.options,
            request.resume,
            c,
            p,
        )?;
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
fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(Jobs::default()))
        .invoke_handler(tauri::generate_handler![
            list_disks,
            inspect_disk,
            plan_clone,
            get_operation_status,
            cancel_operation,
            create_image,
            verify_image
        ])
        .run(tauri::generate_context!())
        .expect("MacClone desktop initialization");
}
