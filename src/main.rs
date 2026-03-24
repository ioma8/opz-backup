use clap::{Parser, Subcommand};
use fs_extra::dir::{CopyOptions, TransitProcess, copy_with_progress};
use human_bytes::human_bytes;
use ml_progress::progress;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    version,
    about = "Back up and restore your OP-Z (no subcommand = run backup)",
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List all existing backups with their sizes
    List,
    /// Interactively select and restore a backup to the OP-Z
    Restore,
    /// Show what changed between the two most recent backups
    Diff,
    /// Show OP-Z connection state and last backup info
    Status,
    /// Open a backup folder in Finder / file manager
    Open,
    /// Watch for OP-Z connection and auto-backup on plug-in
    Watch,
}

fn hb(n: u64) -> String { human_bytes(n as f64) }

fn find_opz(df: &str) -> Option<&str> {
    df.lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().nth(5))
        .find(|p| p.contains("OP-Z"))
}

fn pct(copied: u64, total: u64) -> u8 {
    if total == 0 { 0 } else { (copied * 100 / total) as u8 }
}

fn backup_copy(src: &str, dst: &str, overwrite: bool) -> Result<u64, String> {
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

    let bytes = copy_with_progress(src, dst, &CopyOptions::new().content_only(true).overwrite(overwrite), cb)
        .map_err(|e| e.to_string())?;
    bar.finish();
    Ok(bytes)
}

fn backup_root() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/opz-backups", home)
}

fn backup_names() -> Result<Vec<String>, String> {
    let root = backup_root();
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .map_err(|_| format!("no backups in {}", root))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    Ok(names)
}

fn load_backups() -> Result<Vec<(String, u64)>, String> {
    let root = backup_root();
    let mut entries: Vec<(String, u64)> = std::fs::read_dir(&root)
        .map_err(|_| format!("no backups in {}", root))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let size = fs_extra::dir::get_size(e.path()).ok()?;
            Some((name, size))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

fn opz_mount() -> Result<String, String> {
    let output = Command::new("df").arg("-P").output().map_err(|e| e.to_string())?;
    let df = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    find_opz(&df)
        .map(String::from)
        .ok_or_else(|| "No OP-Z (turn off, press I, turn on, plug USB)".to_string())
}

// Returns relative-path → size for every file under root (iterative, no recursion)
fn walk_dir(root: &Path) -> BTreeMap<String, u64> {
    let mut map = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root).ok()
                    .and_then(|p| p.to_str())
                    .map(str::to_string)
                    .unwrap_or_default();
                map.insert(rel, meta.len());
            }
        }
    }
    map
}

fn run() -> Result<u64, String> {
    let src = opz_mount()?;
    let dst = format!("{}/{}", backup_root(), chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    println!("→ {}", dst);
    std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    backup_copy(&src, &dst, false)
}

fn list_backups() -> Result<(), String> {
    let root = backup_root();
    let entries = load_backups()?;

    if entries.is_empty() {
        println!("no backups in {}", root);
        return Ok(());
    }

    let total: u64 = entries.iter().map(|(_, s)| s).sum();
    let n = entries.len();
    println!("root   {}", root);
    println!("total  {}  ({} backup{})\n", hb(total), n, if n == 1 { "" } else { "s" });

    for (name, size) in &entries {
        println!("  {}   {}", name, hb(*size));
    }

    Ok(())
}

fn diff_backups() -> Result<(), String> {
    let names = backup_names()?;
    if names.len() < 2 {
        return Err("need at least 2 backups to diff".to_string());
    }

    let root = backup_root();
    let a_name = &names[names.len() - 2];
    let b_name = &names[names.len() - 1];
    let a = walk_dir(Path::new(&format!("{}/{}", root, a_name)));
    let b = walk_dir(Path::new(&format!("{}/{}", root, b_name)));

    println!("{} → {}\n", a_name, b_name);

    let mut new_count = 0u32;
    let mut del_count = 0u32;
    let mut mod_count = 0u32;

    for (path, &size) in &a {
        if !b.contains_key(path) {
            println!("  - {}  {}", path, hb(size));
            del_count += 1;
        }
    }
    for (path, &size) in &b {
        match a.get(path) {
            None => { println!("  + {}  {}", path, hb(size)); new_count += 1; }
            Some(&old) if old != size => { println!("  ~ {}  {} → {}", path, hb(old), hb(size)); mod_count += 1; }
            _ => {}
        }
    }

    if new_count + del_count + mod_count == 0 {
        println!("  no changes");
    } else {
        println!("\n  {} change{}  ({} new, {} modified, {} deleted)",
            new_count + del_count + mod_count,
            if new_count + del_count + mod_count == 1 { "" } else { "s" },
            new_count, mod_count, del_count);
    }

    Ok(())
}

fn status() -> Result<(), String> {
    match opz_mount() {
        Ok(path) => println!("OP-Z    connected   {}", path),
        Err(_)   => println!("OP-Z    not connected"),
    }

    let root = backup_root();
    let entries = load_backups().unwrap_or_default();
    if entries.is_empty() {
        println!("backup  no backups yet");
        println!("root    {}", root);
    } else {
        let last = entries.last().unwrap();
        let total: u64 = entries.iter().map(|(_, s)| s).sum();
        let n = entries.len();
        println!("backup  {} backup{}   last: {}  ({})",
            n, if n == 1 { "" } else { "s" }, last.0, hb(last.1));
        println!("root    {}   ({} total)", root, hb(total));
    }

    Ok(())
}

fn open_backup() -> Result<(), String> {
    let names = backup_names()?;
    if names.is_empty() {
        return Err(format!("no backups in {}", backup_root()));
    }

    let theme = dialoguer::theme::ColorfulTheme::default();
    let idx = dialoguer::Select::with_theme(&theme)
        .with_prompt("Select backup to open")
        .items(&names)
        .default(names.len() - 1)
        .interact()
        .map_err(|e| e.to_string())?;

    let path = format!("{}/{}", backup_root(), names[idx]);
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Command::new(opener).arg(&path).status().map_err(|e| e.to_string())?;
    Ok(())
}

fn restore() -> Result<(), String> {
    let names = backup_names()?;
    if names.is_empty() {
        return Err(format!("no backups in {}", backup_root()));
    }

    let theme = dialoguer::theme::ColorfulTheme::default();

    let idx = dialoguer::Select::with_theme(&theme)
        .with_prompt("Select backup to restore")
        .items(&names)
        .default(names.len() - 1)
        .interact()
        .map_err(|e| e.to_string())?;

    let name = &names[idx];
    let src = format!("{}/{}", backup_root(), name);
    let dst = opz_mount()?;

    let confirmed = dialoguer::Confirm::with_theme(&theme)
        .with_prompt(format!("Restore {} → {}?", name, dst))
        .default(false)
        .interact()
        .map_err(|e| e.to_string())?;

    if !confirmed {
        println!("cancelled");
        return Ok(());
    }

    println!("→ restoring {} to {}", name, dst);
    let bytes = backup_copy(&src, &dst, true)?;
    println!("✓ {} restored", hb(bytes));
    Ok(())
}

fn watch() -> Result<(), String> {
    // Initialise to current state so we only trigger on new plug-ins
    let mut was_connected = opz_mount().is_ok();
    if was_connected {
        println!("OP-Z already connected — will back up on next reconnect");
    } else {
        println!("Watching for OP-Z... (Ctrl+C to stop)");
    }

    loop {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let connected = opz_mount().is_ok();

        if connected && !was_connected {
            println!("OP-Z connected — backing up...");
            match run() {
                Ok(b)  => println!("✓ {} copied — watching...", hb(b)),
                Err(e) => eprintln!("✗ {} — watching...", e),
            }
        } else if !connected && was_connected {
            println!("OP-Z disconnected — watching...");
        }

        was_connected = connected;
    }
}

fn main() {
    let result: Result<(), String> = match Cli::parse().command {
        None                 => run().map(|b| println!("✓ {} copied", hb(b))),
        Some(Cmd::List)      => list_backups(),
        Some(Cmd::Restore)   => restore(),
        Some(Cmd::Diff)      => diff_backups(),
        Some(Cmd::Status)    => status(),
        Some(Cmd::Open)      => open_backup(),
        Some(Cmd::Watch)     => watch(),
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
        let df = "Filesystem 1 2 3 4 /Volumes/OP-Z\n\
                  /dev/disk1 100 50 50 50% /normal";
        assert_eq!(find_opz(df), None);
    }

    #[test]
    fn find_opz_matches_on_mount_column_not_filesystem() {
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
        assert_eq!(pct(1, 3), 33);
        assert_eq!(pct(2, 3), 66);
    }

    #[test]
    fn pct_large_values_no_overflow() {
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
            false,
        ).unwrap();

        assert_eq!(bytes, 22);
        assert_eq!(fs::read(dst.path().join("a.txt")).unwrap(), b"hello world");
        assert_eq!(fs::read(dst.path().join("b.txt")).unwrap(), b"foo bar baz");
    }

    #[test]
    fn backup_copy_empty_dir_returns_zero_bytes() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        assert_eq!(backup_copy(src.path().to_str().unwrap(), dst.path().to_str().unwrap(), false).unwrap(), 0);
    }

    #[test]
    fn backup_copy_preserves_subdirectories() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        fs::create_dir(src.path().join("subdir")).unwrap();
        fs::write(src.path().join("subdir").join("nested.txt"), b"nested").unwrap();

        backup_copy(src.path().to_str().unwrap(), dst.path().to_str().unwrap(), false).unwrap();

        assert_eq!(fs::read(dst.path().join("subdir").join("nested.txt")).unwrap(), b"nested");
    }

    #[test]
    fn backup_copy_overwrites_when_flag_set() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        fs::write(src.path().join("f.txt"), b"new").unwrap();
        fs::write(dst.path().join("f.txt"), b"old content longer").unwrap();

        backup_copy(src.path().to_str().unwrap(), dst.path().to_str().unwrap(), true).unwrap();

        assert_eq!(fs::read(dst.path().join("f.txt")).unwrap(), b"new");
    }

    #[test]
    fn backup_copy_errors_on_missing_src() {
        let dst = tempfile::tempdir().unwrap();
        assert!(backup_copy("/nonexistent/path/opz", dst.path().to_str().unwrap(), false).is_err());
    }

    // ── walk_dir ─────────────────────────────────────────────────────────────

    #[test]
    fn walk_dir_collects_files_with_sizes() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), b"hello").unwrap();
        fs::write(root.path().join("b.txt"), b"world!!").unwrap();

        let map = walk_dir(root.path());
        assert_eq!(map.get("a.txt"), Some(&5));
        assert_eq!(map.get("b.txt"), Some(&7));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn walk_dir_collects_nested_files() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join("top.txt"), b"top").unwrap();
        fs::write(root.path().join("sub").join("deep.txt"), b"deep").unwrap();

        let map = walk_dir(root.path());
        assert_eq!(map.get("top.txt"), Some(&3));
        assert_eq!(map.get("sub/deep.txt"), Some(&4));
    }

    #[test]
    fn walk_dir_empty_dir_returns_empty_map() {
        let root = tempfile::tempdir().unwrap();
        assert!(walk_dir(root.path()).is_empty());
    }

    #[test]
    fn walk_dir_missing_dir_returns_empty_map() {
        assert!(walk_dir(Path::new("/nonexistent/path")).is_empty());
    }

    // ── load_backups / list_backups ──────────────────────────────────────────

    #[test]
    fn load_backups_returns_sorted_oldest_first() {
        let _lock = HOME_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", root.path()) };

        let b1 = root.path().join("opz-backups").join("2026-03-24_14-30-00");
        let b2 = root.path().join("opz-backups").join("2026-03-20_10-00-00");
        fs::create_dir_all(&b1).unwrap();
        fs::create_dir_all(&b2).unwrap();

        let entries = load_backups().unwrap();
        assert_eq!(entries[0].0, "2026-03-20_10-00-00");
        assert_eq!(entries[1].0, "2026-03-24_14-30-00");
    }

    #[test]
    fn load_backups_errors_when_root_missing() {
        let _lock = HOME_LOCK.lock().unwrap();
        unsafe { std::env::set_var("HOME", "/nonexistent/path/that/does/not/exist") };
        assert!(load_backups().is_err());
    }

    #[test]
    fn list_backups_ok_with_valid_dirs() {
        let _lock = HOME_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", root.path()) };

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

    // ── diff_backups ─────────────────────────────────────────────────────────

    #[test]
    fn diff_backups_errors_with_fewer_than_two() {
        let _lock = HOME_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", root.path()) };
        fs::create_dir_all(root.path().join("opz-backups").join("2026-03-20_10-00-00")).unwrap();
        assert!(diff_backups().is_err());
    }

    #[test]
    fn diff_backups_ok_with_two_backups() {
        let _lock = HOME_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", root.path()) };

        let b1 = root.path().join("opz-backups").join("2026-03-20_10-00-00");
        let b2 = root.path().join("opz-backups").join("2026-03-24_14-30-00");
        fs::create_dir_all(&b1).unwrap();
        fs::create_dir_all(&b2).unwrap();
        fs::write(b1.join("same.txt"), b"unchanged").unwrap();
        fs::write(b2.join("same.txt"), b"unchanged").unwrap();
        fs::write(b2.join("new.txt"), b"added").unwrap();

        assert!(diff_backups().is_ok());
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
