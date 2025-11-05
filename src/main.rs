use std::process::Command;

use fs_extra::dir::{copy_with_progress, CopyOptions, TransitProcess};
use human_bytes::human_bytes;
use ml_progress::progress;

// ============================================================================
// DOMAIN DATA TYPES
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    MacOS,
    Linux,
    Windows,
}

#[derive(Debug)]
struct MountPoint {
    path: String,
    device_name: String,
}

#[derive(Debug)]
enum DeviceDiscovery {
    NotFound,
    Found(MountPoint),
}

#[derive(Debug)]
struct CopyProgress {
    total_bytes: u64,
    copied_bytes: u64,
    current_file: String,
}

#[derive(Debug)]
enum BackupResult {
    Success { bytes_copied: u64 },
    DeviceNotFound,
    DestinationError(String),
    CopyError(String),
}

// ============================================================================
// PURE FUNCTIONS (Data Transforms)
// ============================================================================

fn detect_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOS
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

fn parse_mount_points(df_output: &str, platform: Platform) -> Vec<MountPoint> {
    match platform {
        Platform::MacOS => parse_macos_mount_points(df_output),
        Platform::Linux => parse_linux_mount_points(df_output),
        Platform::Windows => Vec::new(),
    }
}

fn parse_macos_mount_points(df_output: &str) -> Vec<MountPoint> {
    df_output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let mount_path = parts.get(8)?;
            if mount_path.contains("Volume") {
                Some(MountPoint {
                    path: mount_path.to_string(),
                    device_name: extract_device_name(mount_path),
                })
            } else {
                None
            }
        })
        .collect()
}

fn parse_linux_mount_points(df_output: &str) -> Vec<MountPoint> {
    df_output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let mount_path = parts.get(5)?;
            if mount_path.starts_with("/media/") || mount_path.starts_with("/mnt/") {
                Some(MountPoint {
                    path: mount_path.to_string(),
                    device_name: extract_device_name(mount_path),
                })
            } else {
                None
            }
        })
        .collect()
}

fn extract_device_name(path: &str) -> String {
    path.split('/').next_back().unwrap_or("Unknown").to_string()
}

fn find_opz_device(mount_points: Vec<MountPoint>) -> DeviceDiscovery {
    mount_points
        .into_iter()
        .find(|mp| mp.device_name.contains("OP-Z"))
        .map_or(DeviceDiscovery::NotFound, DeviceDiscovery::Found)
}

fn create_backup_path(base_dir: &str, timestamp: &str) -> String {
    format!("{}/{}", base_dir, timestamp)
}

fn calculate_progress_percentage(progress: &CopyProgress) -> u8 {
    if progress.total_bytes == 0 {
        0
    } else {
        ((progress.copied_bytes as f64 / progress.total_bytes as f64) * 100.0) as u8
    }
}

fn format_timestamp(datetime: chrono::DateTime<chrono::Local>) -> String {
    datetime.format("%Y-%m-%d_%H-%M-%S").to_string()
}

fn get_default_backup_dir() -> String {
    let home_dir = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/opz-backups", home_dir)
}

// ============================================================================
// I/O BOUNDARY FUNCTIONS
// ============================================================================

fn get_mounted_devices(platform: Platform) -> Result<String, String> {
    let command = match platform {
        Platform::MacOS | Platform::Linux => "df",
        Platform::Windows => "wmic",
    };

    let output = Command::new(command)
        .arg("-h")
        .output()
        .map_err(|e| format!("Failed to execute df command: {}", e))?;

    String::from_utf8(output.stdout).map_err(|e| format!("Invalid UTF-8 in output: {}", e))
}

fn ensure_directory(path: &str) -> Result<(), String> {
    if !std::path::Path::new(path).exists() {
        std::fs::create_dir_all(path)
            .map_err(|e| format!("Failed to create directory {}: {}", path, e))?;
    }
    Ok(())
}

fn copy_files_with_progress<F>(
    source: &str,
    dest: &str,
    mut progress_callback: F,
) -> Result<u64, String>
where
    F: FnMut(CopyProgress),
{
    let options = CopyOptions::new().content_only(true);
    let mut total_bytes_copied = 0u64;

    let handle = |process_info: TransitProcess| {
        let progress = CopyProgress {
            total_bytes: process_info.total_bytes,
            copied_bytes: process_info.copied_bytes,
            current_file: process_info.file_name,
        };

        total_bytes_copied = process_info.copied_bytes;
        progress_callback(progress);

        fs_extra::dir::TransitProcessResult::ContinueOrAbort
    };

    copy_with_progress(source, dest, &options, handle)
        .map_err(|e| format!("Copy failed: {}", e))?;

    Ok(total_bytes_copied)
}

fn print_device_not_found_instructions() {
    println!("No OP-Z found");
    println!("Turn off OP-Z. Press I and turn on. Connect to USB and run this program again.");
}

// ============================================================================
// MAIN (I/O Orchestration)
// ============================================================================

fn main() {
    let result = run_backup();

    match result {
        BackupResult::Success { bytes_copied } => {
            println!("Copied {}", human_bytes(bytes_copied as f64));
            println!("Backup complete");
        }
        BackupResult::DeviceNotFound => {
            print_device_not_found_instructions();
        }
        BackupResult::DestinationError(msg) => {
            eprintln!("Destination error: {}", msg);
        }
        BackupResult::CopyError(msg) => {
            eprintln!("Copy error: {}", msg);
        }
    }
}

fn run_backup() -> BackupResult {
    let platform = detect_platform();

    let df_output = match get_mounted_devices(platform) {
        Ok(output) => output,
        Err(e) => return BackupResult::CopyError(e),
    };

    let mount_points = parse_mount_points(&df_output, platform);
    let device_discovery = find_opz_device(mount_points);

    let opz_mount = match device_discovery {
        DeviceDiscovery::NotFound => return BackupResult::DeviceNotFound,
        DeviceDiscovery::Found(mount) => mount,
    };

    let timestamp = format_timestamp(chrono::Local::now());
    let backup_dest = create_backup_path(&get_default_backup_dir(), &timestamp);

    println!("Backing up to: {}", backup_dest);

    if let Err(e) = ensure_directory(&backup_dest) {
        return BackupResult::DestinationError(e);
    }

    println!("Copying files...");

    let bar = progress!(100).unwrap();
    let mut current_percent = 0u8;

    let progress_callback = |progress: CopyProgress| {
        let percent = calculate_progress_percentage(&progress);
        if percent > current_percent {
            bar.inc((percent - current_percent) as u64);
            current_percent = percent;
        }
        bar.message(progress.current_file.clone());
    };

    match copy_files_with_progress(&opz_mount.path, &backup_dest, progress_callback) {
        Ok(bytes) => {
            bar.finish();
            BackupResult::Success {
                bytes_copied: bytes,
            }
        }
        Err(e) => BackupResult::CopyError(e),
    }
}
