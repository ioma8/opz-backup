use fs_extra::dir::{CopyOptions, TransitProcess, copy_with_progress};
use human_bytes::human_bytes;
use ml_progress::progress;
use std::process::Command;

fn backup_copy(src: &str, dst: &str) -> Result<u64, String> {
    let bar = progress!(100).map_err(|e| e.to_string())?;
    let mut pct = 0u8;

    let cb = |info: TransitProcess| {
        let p = if info.total_bytes == 0 { 0 } else { (info.copied_bytes * 100 / info.total_bytes) as u8 };
        if p > pct {
            bar.inc((p - pct) as u64);
            pct = p;
            bar.message(info.file_name);
        }
        fs_extra::dir::TransitProcessResult::ContinueOrAbort
    };

    let bytes = copy_with_progress(src, dst, &CopyOptions::new().content_only(true), cb)
        .map_err(|e| e.to_string())?;
    bar.finish();
    Ok(bytes)
}

fn run() -> Result<u64, String> {
    let output = Command::new("df").arg("-P").output().map_err(|e| e.to_string())?;
    let df = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;

    let src = df.lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().nth(5))
        .find(|p| p.contains("OP-Z"))
        .ok_or("No OP-Z (turn off, press I, turn on, plug USB)")?;

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dst = format!("{}/opz-backups/{}", home, chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    println!("→ {}", dst);

    std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    backup_copy(src, &dst)
}

fn main() {
    match run() {
        Ok(b) => println!("✓ {} copied", human_bytes(b as f64)),
        Err(e) => eprintln!("✗ {}", e),
    }
}
