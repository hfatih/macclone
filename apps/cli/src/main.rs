use mc_device::{Input, invalid, open_source};
use mc_engine::Options;
use std::{env, io, path::Path, sync::atomic::AtomicBool};
const USAGE: &str = "macclone list | inspect SOURCE | plan SOURCE_NUMBER TARGET_NUMBER | dry-run SOURCE | image SOURCE NEW_JOB_DIR [--recover] [--resume] [--block BYTES] | verify SOURCE SHA256\nSOURCE: image file path or disk:NUMBER:UNIQUE_ID. Use --sector 4096 for a 4Kn image. Keep recovery/block/sector settings identical on resume.";
fn result() -> io::Result<serde_json::Value> {
    #[cfg(all(windows, feature = "windows-physical-clone"))]
    if env::args().nth(1).as_deref() == Some("clone") {
        return raw_clone();
    }
    let mut args: Vec<_> = env::args().skip(1).collect();
    let mut sector = 512;
    let mut options = Options::default();
    let mut resume = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--sector" | "--block" => {
                let flag = args.remove(i);
                if i >= args.len() {
                    return Err(invalid(USAGE));
                }
                let value = args.remove(i).parse::<u32>().map_err(invalid_number)?;
                if flag == "--sector" {
                    sector = value
                } else {
                    options.block = value
                }
            }
            "--recover" => {
                options.recovery = true;
                args.remove(i);
            }
            "--resume" => {
                resume = true;
                args.remove(i);
            }
            _ => i += 1,
        }
    }
    let src = |s: &str| -> io::Result<Input> {
        if let Some(d) = s.strip_prefix("disk:") {
            let (n, id) = d.split_once(':').ok_or_else(|| invalid(USAGE))?;
            Ok(Input::Disk {
                number: n.parse().map_err(invalid_number)?,
                id: id.into(),
            })
        } else {
            Ok(Input::Image {
                path: s.into(),
                sector,
            })
        }
    };
    let cancel = AtomicBool::new(false);
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["list"] => Ok(serde_json::to_value(mc_device::disks()?)?),
        ["inspect", s] => {
            let mut item = open_source(&src(s)?, false)?;
            Ok(serde_json::to_value(mc_inspect::inspect(
                &mut item.reader,
                item.stamp.bytes,
                item.stamp.sector,
            )?)?)
        }
        ["plan", a, b] => {
            let source = a.parse().map_err(invalid_number)?;
            let target = b.parse().map_err(invalid_number)?;
            let mut plan = mc_device::plan_disks(source, target)?;
            plan.physical_write_enabled =
                cfg!(all(windows, feature = "windows-physical-clone")) && plan.blockers.is_empty();
            Ok(serde_json::to_value(plan)?)
        }
        ["dry-run", s] => {
            let item = open_source(&src(s)?, true)?;
            options.validate(item.stamp.sector)?;
            Ok(serde_json::json!({"source":item.stamp,"options":options,"writes":0}))
        }
        ["image", s, d] => {
            let mut last = std::time::Instant::now();
            let r = mc_engine::run(&src(s)?, Path::new(d), options, resume, &cancel, |p| {
                if last.elapsed().as_secs() >= 1 {
                    eprintln!("{} {}/{} bytes", p.phase, p.bytes, p.total);
                    last = std::time::Instant::now()
                }
            })?;
            Ok(serde_json::to_value(r)?)
        }
        ["verify", s, digest] => Ok(
            serde_json::json!({"sha256":mc_engine::verify(&src(s)?,digest,&cancel,|_|{})?,"outcome":"verified"}),
        ),
        _ => Err(invalid(USAGE)),
    }
}
fn invalid_number(e: std::num::ParseIntError) -> io::Error {
    invalid(e.to_string())
}
fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    if env::args().nth(1).as_deref() == Some("--help") {
        println!("{USAGE}");
        #[cfg(all(windows, feature = "windows-physical-clone"))]
        println!("macclone clone SOURCE_NUMBER TARGET_NUMBER JOB_DIR [--resume]");
        return;
    }
    match result() {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            if v.get("outcome").and_then(|s| s.as_str()) == Some("partial") {
                std::process::exit(2)
            }
        }
        Err(e) => {
            tracing::error!("{}", e);
            std::process::exit(1)
        }
    }
}

#[cfg(all(windows, feature = "windows-physical-clone"))]
fn raw_clone() -> io::Result<serde_json::Value> {
    use std::io::{BufRead, IsTerminal, Write};
    let args = env::args().skip(2).collect::<Vec<_>>();
    let (selection, resume) = match args.as_slice() {
        [a, b, dir] => ((a, b, dir), false),
        [a, b, dir, flag] if flag == "--resume" => ((a, b, dir), true),
        _ => {
            return Err(invalid(
                "Usage: macclone clone SOURCE_NUMBER TARGET_NUMBER JOB_DIR [--resume]",
            ));
        }
    };
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(invalid(
            "Physical disk confirmation requires an interactive console",
        ));
    }
    let (source, target, job) = selection;
    let prepared = mc_device::PreparedClone::new(
        source.parse().map_err(invalid_number)?,
        target.parse().map_err(invalid_number)?,
        Path::new(job),
    )?;
    let plan = prepared.plan();
    let disk_description = |disk: &mc_device::Disk| {
        format!(
            "disk {}, model {:?}, serial {:?}, ID {:?}, {} bytes, {} B logical / {} B physical",
            disk.number,
            disk.model,
            disk.serial.as_deref().unwrap_or_default(),
            disk.id.as_deref().unwrap_or_default(),
            disk.bytes,
            disk.logical,
            disk.physical,
        )
    };
    eprintln!("Source      {}", disk_description(&plan.source));
    eprintln!("Destination {}", disk_description(&plan.target));
    eprintln!("Journal {job:?}");
    eprintln!("All sectors of the destination will be overwritten. Type exactly:");
    eprintln!("{}", prepared.confirmation());
    io::stderr().flush()?;
    let mut entered = String::new();
    io::stdin().lock().read_line(&mut entered)?;
    let approved = prepared.approve(entered.trim_end_matches(['\r', '\n']))?;
    let _handler = install_clone_cancel()?;
    let mut last = std::time::Instant::now();
    let report =
        mc_engine::clone_physical(approved, Options::default(), resume, &CLONE_CANCEL, |p| {
            if last.elapsed().as_secs() >= 1 || p.bytes == p.total {
                eprintln!("{} {}/{} bytes", p.phase, p.bytes, p.total);
                last = std::time::Instant::now();
            }
        })?;
    Ok(serde_json::to_value(report)?)
}

#[cfg(all(windows, feature = "windows-physical-clone"))]
static CLONE_CANCEL: AtomicBool = AtomicBool::new(false);

#[cfg(all(windows, feature = "windows-physical-clone"))]
unsafe extern "system" fn clone_ctrl_event(event: u32) -> i32 {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        CLONE_CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
        return 1;
    }
    0
}

#[cfg(all(windows, feature = "windows-physical-clone"))]
struct CloneConsoleHandler;

#[cfg(all(windows, feature = "windows-physical-clone"))]
fn install_clone_cancel() -> io::Result<CloneConsoleHandler> {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    // SAFETY: the registered callback has process lifetime and only accesses an atomic.
    if unsafe { SetConsoleCtrlHandler(Some(clone_ctrl_event), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(CloneConsoleHandler)
}

#[cfg(all(windows, feature = "windows-physical-clone"))]
impl Drop for CloneConsoleHandler {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        // SAFETY: unregisters this process's callback installed above.
        unsafe { SetConsoleCtrlHandler(Some(clone_ctrl_event), 0) };
    }
}
