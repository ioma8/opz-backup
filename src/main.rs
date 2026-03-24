use fs_extra::dir::{CopyOptions, TransitProcess, copy_with_progress};
use human_bytes::human_bytes;
use ml_progress::progress;
use std::process::Command;

fn find_opz(df: &str) -> Option<&str> {
    df.lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().nth(5))
        .find(|p| p.contains("OP-Z"))
}

fn pct(copied: u64, total: u64) -> u8 {
    if total == 0 { 0 } else { (copied * 100 / total) as u8 }
}

fn backup_copy(src: &str, dst: &str) -> Result<u64, String> {
    let bar = progress!(100).map_err(|e| e.to_string())?;
    let mut prev = 0u8;

    let cb = |info: TransitProcess| {
        let p = pct(info.copied_bytes, info.total_bytes);
        if p > prev {
            bar.inc((p - prev) as u64);
            prev = p;
            bar.message(info.file_name);
        }
        fs_extra::dir::TransitProcessResult::ContinueOrAbort
    };

    let bytes = copy_with_progress(src, dst, &CopyOptions::new().content_only(true), cb)
        .map_err(|e| e.to_string())?;
    bar.finish();
    Ok(bytes)
}

fn backup_root() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/opz-backups", home)
}

fn run() -> Result<u64, String> {
    let output = Command::new("df").arg("-P").output().map_err(|e| e.to_string())?;
    let df = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;

    let src = find_opz(&df).ok_or("No OP-Z (turn off, press I, turn on, plug USB)")?;

    let dst = format!("{}/{}", backup_root(), chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    println!("→ {}", dst);

    std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    backup_copy(src, &dst)
}

fn list_backups() -> Result<(), String> {
    let root = backup_root();

    let mut entries: Vec<(String, u64)> = std::fs::read_dir(&root)
        .map_err(|_| format!("no backups found ({})", root))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let size = fs_extra::dir::get_size(e.path()).ok()?;
            Some((name, size))
        })
        .collect();

    if entries.is_empty() {
        println!("no backups in {}", root);
        return Ok(());
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0)); // oldest first

    let total: u64 = entries.iter().map(|(_, s)| s).sum();
    let n = entries.len();
    println!("root   {}", root);
    println!("total  {}  ({} backup{})\n", human_bytes(total as f64), n, if n == 1 { "" } else { "s" });

    for (name, size) in &entries {
        let date = chrono::NaiveDateTime::parse_from_str(name, "%Y-%m-%d_%H-%M-%S")
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|_| name.clone());
        println!("  {}   {}   {}", name, date, human_bytes(*size as f64));
    }

    Ok(())
}

fn main() {
    let result: Result<(), String> = match std::env::args().nth(1).as_deref() {
        Some("list") => list_backups(),
        _ => run().map(|b| println!("✓ {} copied", human_bytes(b as f64))),
    };
    if let Err(e) = result {
        eprintln!("✗ {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    // Serialize tests that mutate HOME to avoid races with parallel test runner
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    // Realistic df -P output (POSIX: 6 cols, mount point always at index 5)
    const DF_WITH_OPZ: &str = "\
Filesystem     1024-blocks      Used  Available Capacity Mounted on
/dev/disk1s1   244277232   94712304  148564528      39% /
devfs                396        396          0     100% /dev
/dev/disk2s1     1953520    1953520          0     100% /Volumes/OP-Z";

    const DF_WITHOUT_OPZ: &str = "\
Filesystem     1024-blocks      Used  Available Capacity Mounted on
/dev/disk1s1   244277232   94712304  148564528      39% /
devfs                396        396          0     100% /dev";

    // ── find_opz ────────────────────────────────────────────────────────────

    #[test]
    fn find_opz_returns_mount_path() {
        assert_eq!(find_opz(DF_WITH_OPZ), Some("/Volumes/OP-Z"));
    }

    #[test]
    fn find_opz_returns_none_when_not_mounted() {
        assert_eq!(find_opz(DF_WITHOUT_OPZ), None);
    }

    #[test]
    fn find_opz_skips_header_row() {
        // Header line has "OP-Z" injected at mount column — must be ignored
        let df = "Filesystem 1 2 3 4 /Volumes/OP-Z\n\
                  /dev/disk1 100 50 50 50% /normal";
        assert_eq!(find_opz(df), None);
    }

    #[test]
    fn find_opz_matches_on_mount_column_not_filesystem() {
        // "OP-Z" appears in the filesystem column (0), not the mount column (5)
        let df = "Filesystem     1024-blocks      Used  Available Capacity Mounted on\n\
                  /dev/OP-Z/s1   244277232   94712304  148564528      39% /other";
        assert_eq!(find_opz(df), None);
    }

    #[test]
    fn find_opz_returns_first_match_when_multiple() {
        let df = "Filesystem     1024-blocks      Used  Available Capacity Mounted on\n\
                  /dev/disk2s1     1953520    1953520          0     100% /Volumes/OP-Z\n\
                  /dev/disk3s1     1953520    1953520          0     100% /Volumes/OP-Z-2";
        assert_eq!(find_opz(df), Some("/Volumes/OP-Z"));
    }

    #[test]
    fn find_opz_empty_input() {
        assert_eq!(find_opz(""), None);
    }

    #[test]
    fn find_opz_header_only() {
        assert_eq!(find_opz("Filesystem 1024-blocks Used Available Capacity Mounted on"), None);
    }

    // ── pct ─────────────────────────────────────────────────────────────────

    #[test]
    fn pct_zero_total_returns_zero() {
        assert_eq!(pct(0, 0), 0);
        assert_eq!(pct(99, 0), 0);
    }

    #[test]
    fn pct_zero_copied_returns_zero() {
        assert_eq!(pct(0, 1000), 0);
    }

    #[test]
    fn pct_half_returns_50() {
        assert_eq!(pct(50, 100), 50);
        assert_eq!(pct(512, 1024), 50);
    }

    #[test]
    fn pct_complete_returns_100() {
        assert_eq!(pct(100, 100), 100);
        assert_eq!(pct(1_000_000, 1_000_000), 100);
    }

    #[test]
    fn pct_truncates_not_rounds() {
        // 1/3 = 33.33... should truncate to 33
        assert_eq!(pct(1, 3), 33);
        // 2/3 = 66.66... should truncate to 66
        assert_eq!(pct(2, 3), 66);
    }

    #[test]
    fn pct_large_values_no_overflow() {
        // copied * 100 could overflow u64 at ~184 PB — well outside real use
        let gb = 1024 * 1024 * 1024u64;
        assert_eq!(pct(gb, gb * 2), 50);
        assert_eq!(pct(gb * 2, gb * 2), 100);
    }

    // ── backup_copy ─────────────────────────────────────────────────────────

    #[test]
    fn backup_copy_copies_files_and_returns_byte_count() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        fs::write(src.path().join("a.txt"), b"hello world").unwrap();
        fs::write(src.path().join("b.txt"), b"foo bar baz").unwrap();

        let bytes = backup_copy(
            src.path().to_str().unwrap(),
            dst.path().to_str().unwrap(),
        ).unwrap();

        assert_eq!(bytes, 22); // 11 + 11
        assert_eq!(fs::read(dst.path().join("a.txt")).unwrap(), b"hello world");
        assert_eq!(fs::read(dst.path().join("b.txt")).unwrap(), b"foo bar baz");
    }

    #[test]
    fn backup_copy_empty_dir_returns_zero_bytes() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        let bytes = backup_copy(
            src.path().to_str().unwrap(),
            dst.path().to_str().unwrap(),
        ).unwrap();

        assert_eq!(bytes, 0);
    }

    #[test]
    fn backup_copy_preserves_subdirectories() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        fs::create_dir(src.path().join("subdir")).unwrap();
        fs::write(src.path().join("subdir").join("nested.txt"), b"nested").unwrap();

        backup_copy(
            src.path().to_str().unwrap(),
            dst.path().to_str().unwrap(),
        ).unwrap();

        assert_eq!(
            fs::read(dst.path().join("subdir").join("nested.txt")).unwrap(),
            b"nested"
        );
    }

    #[test]
    fn backup_copy_errors_on_missing_src() {
        let dst = tempfile::tempdir().unwrap();
        let result = backup_copy("/nonexistent/path/opz", dst.path().to_str().unwrap());
        assert!(result.is_err());
    }

    // ── list_backups ─────────────────────────────────────────────────────────

    #[test]
    fn list_backups_errors_when_root_missing() {
        let _lock = HOME_LOCK.lock().unwrap();
        unsafe { std::env::set_var("HOME", "/nonexistent/path/that/does/not/exist") };
        let result = list_backups();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no backups found"));
    }

    #[test]
    fn list_backups_ok_with_valid_dirs() {
        let _lock = HOME_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", root.path()) };

        // Create two fake timestamped backup dirs with a file each
        let b1 = root.path().join("opz-backups").join("2026-03-20_10-00-00");
        let b2 = root.path().join("opz-backups").join("2026-03-24_14-30-00");
        fs::create_dir_all(&b1).unwrap();
        fs::create_dir_all(&b2).unwrap();
        fs::write(b1.join("data.bin"), vec![0u8; 100]).unwrap();
        fs::write(b2.join("data.bin"), vec![0u8; 200]).unwrap();

        assert!(list_backups().is_ok());
    }

    #[test]
    fn list_backups_ok_when_dir_empty() {
        let _lock = HOME_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", root.path()) };
        fs::create_dir_all(root.path().join("opz-backups")).unwrap();

        assert!(list_backups().is_ok());
    }

    // ── backup_root ──────────────────────────────────────────────────────────

    #[test]
    fn backup_root_uses_home() {
        let _lock = HOME_LOCK.lock().unwrap();
        unsafe { std::env::set_var("HOME", "/tmp/testhome") };
        assert_eq!(backup_root(), "/tmp/testhome/opz-backups");
    }

    #[test]
    fn backup_root_falls_back_to_dot() {
        let _lock = HOME_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("HOME") };
        assert_eq!(backup_root(), "./opz-backups");
    }
}
