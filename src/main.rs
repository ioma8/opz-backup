use std::process::Command;

use fs_extra::dir::{copy_with_progress, CopyOptions, TransitProcess};
use human_bytes::human_bytes;
use ml_progress::progress;

// ============================================================================
// ERROR HANDLING
// ============================================================================

#[derive(Debug)]
enum BackupError {
    DeviceNotFound,
    CommandExecution(String),
    InvalidOutput(String),
    DirectoryCreation(String),
    CopyOperation(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::DeviceNotFound => write!(f, "OP-Z device not found"),
            BackupError::CommandExecution(msg) => write!(f, "Command execution failed: {}", msg),
            BackupError::InvalidOutput(msg) => write!(f, "Invalid command output: {}", msg),
            BackupError::DirectoryCreation(msg) => write!(f, "Directory creation failed: {}", msg),
            BackupError::CopyOperation(msg) => write!(f, "Copy operation failed: {}", msg),
        }
    }
}

impl std::error::Error for BackupError {}

type BackupResult = Result<u64, BackupError>;

// ============================================================================
// DOMAIN DATA TYPES
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    MacOS,
    Linux,
    Windows,
}

#[derive(Debug, Clone)]
struct BackupConfig {
    backup_dir: String,
    device_name_pattern: String,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            backup_dir: get_default_backup_dir(),
            device_name_pattern: "OP-Z".to_string(),
        }
    }
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
        Platform::Windows => parse_windows_mount_points(df_output),
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

fn parse_windows_mount_points(_df_output: &str) -> Vec<MountPoint> {
    // TODO: Implement Windows mount point parsing
    // For now, return empty vector
    Vec::new()
}

fn extract_device_name(path: &str) -> String {
    path.split('/')
        .last()
        .unwrap_or("Unknown")
        .to_string()
}

fn find_device(mount_points: Vec<MountPoint>, pattern: &str) -> DeviceDiscovery {
    mount_points
        .into_iter()
        .find(|mp| mp.device_name.contains(pattern))
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

fn get_mounted_devices(platform: Platform) -> Result<String, BackupError> {
    let (command, args) = match platform {
        Platform::MacOS | Platform::Linux => ("df", vec!["-h"]),
        Platform::Windows => ("wmic", vec!["logicaldisk", "get", "size,freespace,caption"]),
    };

    let output = Command::new(command)
        .args(&args)
        .output()
        .map_err(|e| BackupError::CommandExecution(format!("Failed to execute command '{}': {}", command, e)))?;

    if !output.status.success() {
        return Err(BackupError::CommandExecution(format!(
            "Command '{}' failed with exit code: {}",
            command,
            output.status.code().unwrap_or(-1)
        )));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| BackupError::InvalidOutput(format!("Invalid UTF-8 in command output: {}", e)))
}

fn ensure_directory(path: &str) -> Result<(), BackupError> {
    if !std::path::Path::new(path).exists() {
        std::fs::create_dir_all(path)
            .map_err(|e| BackupError::DirectoryCreation(format!("Failed to create directory '{}': {}", path, e)))?;
    }
    Ok(())
}

fn copy_files_with_progress<F>(
    source: &str,
    dest: &str,
    mut progress_callback: F,
) -> Result<u64, BackupError>
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
        .map_err(|e| BackupError::CopyOperation(format!("Copy operation failed: {}", e)))?;

    Ok(total_bytes_copied)
}

fn print_device_not_found_instructions(device_pattern: &str) {
    println!("No {} device found", device_pattern);
    println!("Instructions:");
    println!("1. Turn off the OP-Z device");
    println!("2. Press 'I' button and turn on the device");
    println!("3. Connect the device to USB");
    println!("4. Run this program again");
}

// ============================================================================
// MAIN BUSINESS LOGIC
// ============================================================================

fn run_backup_with_config(config: BackupConfig) -> BackupResult {
    let platform = detect_platform();

    println!("Detecting platform: {:?}", platform);
    println!("Looking for device matching pattern: '{}'", config.device_name_pattern);

    let df_output = get_mounted_devices(platform)?;
    let mount_points = parse_mount_points(&df_output, platform);
    
    println!("Found {} mount points", mount_points.len());
    
    let device_discovery = find_device(mount_points, &config.device_name_pattern);

    let device_mount = match device_discovery {
        DeviceDiscovery::NotFound => return Err(BackupError::DeviceNotFound),
        DeviceDiscovery::Found(mount) => {
            println!("Found device '{}' at path: {}", mount.device_name, mount.path);
            mount
        }
    };

    let timestamp = format_timestamp(chrono::Local::now());
    let backup_dest = create_backup_path(&config.backup_dir, &timestamp);

    println!("Creating backup at: {}", backup_dest);

    ensure_directory(&backup_dest)?;

    println!("Starting file copy...");

    // Progress bar setup
    let bar = progress!(100).unwrap();
    let mut current_percent = 0u8;

    let progress_callback = |progress: CopyProgress| {
        let percent = calculate_progress_percentage(&progress);
        if percent > current_percent {
            bar.inc((percent - current_percent) as u64);
            current_percent = percent;
        }
        
        // Show current file being copied (truncate if too long)
        let display_file = if progress.current_file.len() > 50 {
            format!("...{}", &progress.current_file[progress.current_file.len() - 47..])
        } else {
            progress.current_file.clone()
        };
        bar.message(display_file);
    };

    let bytes_copied = copy_files_with_progress(&device_mount.path, &backup_dest, progress_callback)?;
    bar.finish();
    
    Ok(bytes_copied)
}

// ============================================================================
// MAIN ENTRY POINT
// ============================================================================

fn main() {
    println!("OP-Z Backup Tool");
    println!("================");

    let config = BackupConfig::default();
    let result = run_backup_with_config(config.clone());

    match result {
        Ok(bytes_copied) => {
            println!();
            println!("✅ Backup completed successfully!");
            println!("📁 Copied: {}", human_bytes(bytes_copied as f64));
            println!("📍 Location: {}", config.backup_dir);
        }
        Err(BackupError::DeviceNotFound) => {
            println!();
            println!("❌ Device not found");
            print_device_not_found_instructions(&config.device_name_pattern);
        }
        Err(err) => {
            println!();
            println!("❌ Backup failed: {}", err);
            eprintln!("Error details: {:?}", err);
        }
    }
}