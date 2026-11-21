use mc_device::{Input, invalid, open_source};
use mc_engine::Options;
use std::{env, io, path::Path, sync::atomic::AtomicBool};
const USAGE: &str = "macclone list | inspect SOURCE | plan SOURCE_NUMBER TARGET_NUMBER | dry-run SOURCE | image SOURCE NEW_JOB_DIR [--recover] [--resume] [--block BYTES] | verify SOURCE SHA256\nSOURCE: image file path or disk:NUMBER:UNIQUE_ID. Use --sector 4096 for a 4Kn image. Keep recovery/block/sector settings identical on resume.";
fn result() -> io::Result<serde_json::Value> {
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
            let disks = mc_device::disks()?;
            let pick = |n: &str| -> io::Result<&mc_device::Disk> {
                let number = n.parse::<u32>().map_err(invalid_number)?;
                disks
                    .iter()
                    .find(|d| d.number == number)
                    .ok_or_else(|| invalid("Disk missing"))
            };
            let source = pick(a)?;
            let target = pick(b)?;
            Ok(
                serde_json::json!({"source":source,"target":target,"blockers":mc_device::plan(source,target),"physical_write_enabled":false}),
            )
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
