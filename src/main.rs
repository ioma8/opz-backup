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

fn run() -> Result<u64, String> {
    let output = Command::new("df").arg("-P").output().map_err(|e| e.to_string())?;
    let df = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;

    let src = find_opz(&df).ok_or("No OP-Z (turn off, press I, turn on, plug USB)")?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
}
