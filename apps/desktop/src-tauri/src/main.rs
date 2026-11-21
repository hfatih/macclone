#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use mc_device::{Disk, Input, open_source};
use mc_engine::{Options, Progress, Report};
use serde::{Deserialize, Serialize};
use std::{
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
#[derive(Clone, Serialize, Default)]
struct Status {
    id: u64,
    running: bool,
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
async fn plan_clone(source: u32, target: u32) -> Api<Vec<String>> {
    let disks = list_disks().await?;
    let pick = |n| {
        disks
            .iter()
            .find(|d| d.number == n)
            .ok_or("Disk selection changed".to_owned())
    };
    let mut reasons = mc_device::plan(pick(source)?, pick(target)?);
    reasons.push("Fiziksel hedefe yazma bu sürümde kapalı. Kaynaktan RAW imaj alınabilir.".into());
    Ok(reasons)
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
fn job<F>(app: AppHandle, f: F) -> Api<u64>
where
    F: FnOnce(&AtomicBool, &dyn Fn(Progress)) -> Api<(Option<Report>, String)> + Send + 'static,
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
        ..Status::default()
    };
    drop(jobs);
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<Jobs>>();
        let publish = Mutex::new(std::time::Instant::now());
        let progress = |p: Progress| {
            if let Ok(mut j) = state.lock() {
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
            j.record.running = false;
            match res {
                Ok(Ok((r, msg))) => {
                    j.record.report = r;
                    j.record.message = Some(msg)
                }
                Ok(Err(e)) => j.record.message = Some(e),
                Err(_) => j.record.message = Some("Worker stopped unexpectedly".into()),
            }
            let _ = app.emit("operation", &j.record);
        }
    });
    Ok(id)
}
#[tauri::command]
fn create_image(app: AppHandle, request: Request) -> Api<u64> {
    job(app, move |c, p| {
        let r = mc_engine::run(
            &request.input,
            &request.directory,
            request.options,
            request.resume,
            c,
            p,
        )
        .map_err(err)?;
        Ok((Some(r), "İmaj raporu iş klasörüne kaydedildi.".into()))
    })
}
#[tauri::command]
fn verify_image(app: AppHandle, input: Input, expected: String) -> Api<u64> {
    job(app, move |c, p| {
        let total = open_source(&input, true).map_err(err)?.stamp.bytes;
        let hash = mc_engine::verify(&input, &expected, c, |bytes| {
            p(Progress {
                phase: "verify".into(),
                bytes,
                total,
                missing_sectors: 0,
            })
        })
        .map_err(err)?;
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
