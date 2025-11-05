use fs_extra::dir::{CopyOptions, TransitProcess, copy_with_progress};
use human_bytes::human_bytes;
use ml_progress::progress;
use std::process::Command;

// ============================================================================
// DATA
// ============================================================================

enum Outcome {
    Ok(u64),
    NoDevice,
    Err(String),
}

// ============================================================================
// CORE
// ============================================================================

fn find_opz(df: &str) -> Option<String> {
    let col = if cfg!(target_os = "macos") { 8 } else { 5 };
    df.lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().nth(col))
        .find(|p| p.contains("OP-Z"))
        .map(String::from)
}

fn backup_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    format!("{}/opz-backups/{}", home, ts)
}

fn copy(src: &str, dst: &str) -> Result<u64, String> {
    let bar = progress!(100).unwrap();
    let mut pct = 0u8;
    let mut total = 0u64;

    let cb = |info: TransitProcess| {
        total = info.copied_bytes;
        let p = if info.total_bytes == 0 {
            0
        } else {
            ((info.copied_bytes as f64 / info.total_bytes as f64) * 100.0) as u8
        };
        if p > pct {
            bar.inc((p - pct) as u64);
            pct = p;
        }
        bar.message(info.file_name);
        fs_extra::dir::TransitProcessResult::ContinueOrAbort
    };

    copy_with_progress(src, dst, &CopyOptions::new().content_only(true), cb)
        .map_err(|e| e.to_string())?;

    bar.finish();
    Ok(total)
}

// ============================================================================
// MAIN
// ============================================================================

fn main() {
    match run() {
        Outcome::Ok(b) => println!("✓ {} copied", human_bytes(b as f64)),
        Outcome::NoDevice => println!("✗ No OP-Z (turn off, press I, turn on, plug USB)"),
        Outcome::Err(e) => eprintln!("✗ {}", e),
    }
}

fn run() -> Outcome {
    let df = Command::new("df")
        .arg("-h")
        .output()
        .map_err(|e| e.to_string())
        .and_then(|o| String::from_utf8(o.stdout).map_err(|e| e.to_string()));

    let df = match df {
        Ok(d) => d,
        Err(e) => return Outcome::Err(e),
    };

    let src = match find_opz(&df) {
        Some(p) => p,
        None => return Outcome::NoDevice,
    };

    let dst = backup_dir();
    println!("→ {}", dst);

    if let Err(e) = std::fs::create_dir_all(&dst) {
        return Outcome::Err(e.to_string());
    }

    match copy(&src, &dst) {
        Ok(b) => Outcome::Ok(b),
        Err(e) => Outcome::Err(e),
    }
}
