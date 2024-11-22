use std::{cell::LazyCell, process::Command};

use fs_extra::dir::{copy_with_progress, CopyOptions, TransitProcess};
use human_bytes::human_bytes;
use ml_progress::progress;

const BACKUP_DIR: LazyCell<String> = LazyCell::new(|| {
    let home_dir = std::env::var("HOME").expect("Could not get home directory");
    format!("{}/opz-backups", home_dir)
});

fn main() {
    // TODO: reagovat na pripojeni USB disku a automaticky zacit backupovat

    // list mounted disk drives on macos
    // TODO: dodelat pro windows a linux podporu tohoto sameho kk 
    let output = Command::new("df")
        .arg("-h")
        .output()
        .expect("Failed to execute command");

    let output_str = String::from_utf8_lossy(&output.stdout);

    // its macos specific kk
    let opz_mount_path = output_str
        .lines()
        .skip(1)
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let mount_path = parts.get(8)?;
            if mount_path.contains("Volume") {
                Some(*mount_path)
            } else {
                None
            }
        })
        .filter(|mount_dir| mount_dir.contains("OP-Z"))
        .next();

    if opz_mount_path.is_none() {
        println!("No OP-Z found");
        println!("Turn off OP-Z. Press I and turn on. Connect to USB and run this program again.");
        return;
    }

    let backup_dir = BACKUP_DIR.clone();
    let now = chrono::Local::now();
    let formatted_date = now.format("%Y-%m-%d_%H-%M-%S").to_string();
    let new_backup_dir = format!("{}/{}", backup_dir, formatted_date);
    println!("Backing up to: {}", new_backup_dir);

    if !std::path::Path::new(&new_backup_dir).exists() {
        if std::fs::create_dir_all(&new_backup_dir).is_err() {
            println!("Failed to create backup directory: {}", new_backup_dir);
            return;
        }
    }

    println!("Copying files...");

    if let Some(opz_mount_path) = opz_mount_path {
        let options = CopyOptions::new().content_only(true);

        // TODO: pouzit lepsi komponentu kk
        let bar = progress!(100).unwrap();
        let current_percent = 0;
        let mut total_bytes = 0;

        let handle = |process_info: TransitProcess| {
            if total_bytes == 0 {
                total_bytes = process_info.total_bytes;
            }

            let bytes_transferred = process_info.copied_bytes;
            let percent = (bytes_transferred as f64 / total_bytes as f64) * 100.0;
            if percent as u64 > current_percent {
                bar.inc(percent as u64 - current_percent);
            }

            let file_name = process_info.file_name;
            bar.message(format!("{}", file_name));

            fs_extra::dir::TransitProcessResult::ContinueOrAbort
        };
        copy_with_progress(opz_mount_path, new_backup_dir, &options, handle)
            .expect("Failed to copy files");

        bar.finish();

        println!("Copied {}", human_bytes(total_bytes as f64));
        println!("Backup complete");
    }
}
