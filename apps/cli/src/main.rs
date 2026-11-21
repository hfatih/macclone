use mc_device::{Input, invalid, open_source};
use mc_engine::Options;
use std::{env, io, path::Path, sync::atomic::AtomicBool};
const USAGE: &str = "macclone list | inspect SOURCE | plan SOURCE_NUMBER TARGET_NUMBER | dry-run SOURCE [--target-bytes BYTES] | image SOURCE NEW_JOB_DIR [--recover] [--resume] [--block BYTES] [--target-bytes BYTES] | verify SOURCE SHA256\nSOURCE: image file path or disk:NUMBER:UNIQUE_ID. Use --sector 4096 for a 4Kn image. Keep recovery/block/sector/target size identical on resume.";
fn result() -> io::Result<serde_json::Value> {
    #[cfg(all(windows, feature = "windows-physical-clone"))]
    if env::args().nth(1).as_deref() == Some("clone") {
        return raw_clone();
    }
    let mut args: Vec<_> = env::args().skip(1).collect();
    let mut sector = 512;
    let mut options = Options::default();
    let mut sector_explicit = false;
    let mut block_explicit = false;
    let mut resume = false;
    let mut target_bytes = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--sector" | "--block" => {
                let flag = args.remove(i);
                if i >= args.len() {
                    return Err(invalid(USAGE));
                }
                let value = args.remove(i).parse::<u32>().map_err(invalid_number)?;
                if flag == "--sector" && !sector_explicit {
                    sector = value;
                    sector_explicit = true;
                } else if flag == "--block" && !block_explicit {
                    options.block = value;
                    block_explicit = true;
                } else {
                    return Err(invalid(format!("Duplicate {flag}")));
                }
            }
            "--target-bytes" if target_bytes.is_none() => {
                args.remove(i);
                if i >= args.len() {
                    return Err(invalid(USAGE));
                }
                target_bytes = Some(args.remove(i).parse::<u64>().map_err(invalid_number)?);
            }
            "--target-bytes" => return Err(invalid("Duplicate --target-bytes")),
            "--recover" if !options.recovery => {
                options.recovery = true;
                args.remove(i);
            }
            "--resume" if !resume => {
                resume = true;
                args.remove(i);
            }
            "--recover" | "--resume" => return Err(invalid(format!("Duplicate {}", args[i]))),
            _ => i += 1,
        }
    }
    let command = args.first().map(String::as_str).unwrap_or_default();
    if resume && command != "image" {
        return Err(invalid("--resume is only supported by image"));
    }
    if target_bytes.is_some() && !matches!(command, "image" | "dry-run") {
        return Err(invalid(
            "--target-bytes is only supported by image and dry-run",
        ));
    }
    if target_bytes.is_some() && options.recovery {
        return Err(invalid(
            "GPT relocation requires readable source metadata and data",
        ));
    }
    if (options.recovery || block_explicit) && !matches!(command, "image" | "dry-run") {
        return Err(invalid(
            "--recover and --block are only supported by image and dry-run",
        ));
    }
    if sector_explicit && !matches!(command, "image" | "dry-run" | "inspect" | "verify") {
        return Err(invalid(
            "--sector is only supported for image file operations",
        ));
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
            let mut item = open_source(&src(s)?, true)?;
            options.validate(item.stamp.sector)?;
            let mut report = serde_json::json!({"source":item.stamp,"options":options,"writes":0});
            if let Some(bytes) = target_bytes {
                let gpt = mc_inspect::GptRelocation::prepare(
                    &mut item.reader,
                    item.stamp.bytes,
                    bytes,
                    item.stamp.sector,
                )?;
                report["gpt"] = serde_json::json!({
                    "target_bytes": gpt.target_bytes(),
                    "minimum_target_bytes": gpt.minimum_target_bytes(),
                    "omitted_tail_bytes": item.stamp.bytes.saturating_sub(gpt.target_bytes()),
                });
            }
            Ok(report)
        }
        ["image", s, d] => {
            let mut last = std::time::Instant::now();
            let progress = |p: mc_engine::Progress| {
                if last.elapsed().as_secs() >= 1 {
                    eprintln!("{} {}/{} bytes", p.phase, p.bytes, p.total);
                    last = std::time::Instant::now()
                }
            };
            let input = src(s)?;
            let dir = Path::new(d);
            let r = match target_bytes {
                Some(bytes) => {
                    mc_engine::run_relocated(&input, dir, bytes, options, resume, &cancel, progress)
                }
                None => mc_engine::run(&input, dir, options, resume, &cancel, progress),
            }?;
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
        println!("macclone clone SOURCE_NUMBER TARGET_NUMBER JOB_DIR [--resume] [--relocate-gpt]");
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
    let [source, target, job, flags @ ..] = args.as_slice() else {
        return Err(invalid(
            "Usage: macclone clone SOURCE_NUMBER TARGET_NUMBER JOB_DIR [--resume] [--relocate-gpt]",
        ));
    };
    let mut resume = false;
    let mut layout = mc_device::CloneLayout::Exact;
    for flag in flags {
        match flag.as_str() {
            "--resume" if !resume => resume = true,
            "--relocate-gpt" if layout == mc_device::CloneLayout::Exact => {
                layout = mc_device::CloneLayout::RelocateGpt
            }
            _ => {
                return Err(invalid(format!(
                    "Unknown or duplicate clone option: {flag}"
                )));
            }
        }
    }
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(invalid(
            "Physical disk confirmation requires an interactive console",
        ));
    }
    let prepared = mc_device::PreparedClone::new(
        source.parse().map_err(invalid_number)?,
        target.parse().map_err(invalid_number)?,
        Path::new(job),
        layout,
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
    if layout == mc_device::CloneLayout::RelocateGpt {
        eprintln!(
            "Layout: relocate the backup GPT to the end of the destination and clear the added space. Partition boundaries are preserved."
        );
    }
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
