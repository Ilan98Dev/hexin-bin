use chrono::Local;
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::io::{self, IsTerminal};
use std::fs::OpenOptions;
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "hexin", version, about = "Hexin: The AI Powered Hyprland + Quickshell Config")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a Full System Update.
    Update {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        check: bool,
    },

    /// Lay Down Main Configs in ~/.config.
    Materialize {
        #[arg(long, default_value = "/usr/share/hexin/configs")]
        src: PathBuf,
        #[arg(long)]
        force: bool,
    },

    /// Check System and Hexin health.
    Doctor {
        #[arg(long)]
        json: bool,
    },

    /// Backup Current Config
    Backup {
        #[arg(long)]
        dst: Option<PathBuf>,
    },

    /// Restore Backed Up Config (First run 'hexin backup').
    Restore {
        #[arg(long)]
        src: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        list: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Update { dry_run, check } => cmd_update(dry_run, check)?,
        Commands::Materialize { src, force } => cmd_materialize(&src, force)?,
        Commands::Doctor { json } => cmd_doctor(json)?,
        Commands::Backup { dst } => cmd_backup(dst)?,
        Commands::Restore { src, force, list } => cmd_restore(src, force, list)?,
    }

    Ok(())
}

fn cmd_update(dry_run: bool, check: bool) -> Result<(), Box<dyn std::error::Error>> {
    let aur_helper = if bin_in_path("yay") {
        Some("yay")
    } else if bin_in_path("paru") {
        Some("paru")
    } else {
        None
    };

    let flatpak = if bin_in_path("flatpak") {
        Some("flatpak")
    } else {
        None
    };

    if dry_run {
        println!("[dry-run] sudo pacman -Syu");
        match aur_helper {
            Some(helper) => println!("[dry-run] {helper} -Sua"),
            None => eprintln!("[dry-run] {}: neither `yay` nor `paru` found; skipping AUR update", yellow("WARN")),
        }
        match flatpak {
            Some(_flatpak_cmd) => println!("[dry-run] flatpak update"),
            None => eprintln!("[dry-run] {}: Flatpak is not installed; skipping flatpak update", yellow("WARN")),
        }
        return Ok(());
    }

    if check {
        println!("Please wait!");
        cmd_check_updates()?;
        return Ok(());
    }

    Command::new("sudo").arg("-v").status()?;

    println!("{} Updating Pacman Packages.", green("=>"));
    let pacman_status = Command::new("sudo").args(["pacman", "-Syu"]).status()?;
    if !pacman_status.success() {
        return Err("pacman -Syu failed".into());
    }

    println!("{} Updating AUR Packages.", green("=>"));
    match aur_helper {
        Some(helper) => {
            let aur_status = Command::new(helper).args(["-Sua"]).status()?;
            if !aur_status.success() {
                return Err(format!("{helper} -Sua failed").into());
            }
        }
        None => {
            eprintln!("Error: no AUR helper (`yay` or `paru`) found in PATH; skipping AUR update.");
        }
    }

    println!("{} Updating Flatpak Packages", green("=>"));
    match flatpak {
        Some(flatpak_cmd) => {
            let flatpak_status = Command::new(flatpak_cmd).args(["update"]).status()?;
            if !flatpak_status.success() {
                return Err(format!("flatpak update failed").into());
            }
        }
        None => {
            eprintln!("Error: Flatpak is not installed; skipping flatpak update.");
        }
    }
    println!("{}", green("Updates Complete"));
    if reboot_required() { 
        println!("\nReboot recommended: installed kernel modules differ from running kernel."); 
    }
    Ok(())
}

fn cmd_materialize(src: &Path, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !src.is_dir() {
        return Err(format!("source directory not found: {}", src.display()).into());
    }

    let home = std::env::var("HOME")?;
    let dst_base = PathBuf::from(home).join(".config");

    let (written, skipped) = copy_dir_walk(src, src, &dst_base, force)?;
    println!(
        "Materialized {} file(s) into {} (skipped {})",
        written,
        dst_base.display(),
        skipped
    );
    Ok(())
}

fn copy_dir_walk(
    root: &Path,
    current: &Path,
    dst_base: &Path,
    force: bool,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let mut written = 0;
    let mut skipped = 0;

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let target = dst_base.join(rel);

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir_all(&target)?;
            let (w, s) = copy_dir_walk(root, &path, dst_base, force)?;
            written += w;
            skipped += s;
        } else if file_type.is_symlink() {
            if !force && target.symlink_metadata().is_ok() {
                println!("{}  {} (already exists)", yellow("skip"), target.display());
                skipped += 1;
                continue;
            }
            if target.symlink_metadata().is_ok() {
                fs::remove_file(&target)?;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let link_target = fs::read_link(&path)?;
            std::os::unix::fs::symlink(link_target, &target)?;
            println!("{} {}", green("wrote"), target.display());
            written += 1;
        } else {
            if !force && target.symlink_metadata().is_ok() {
                println!("{}  {} (already exists)", yellow("skip"), target.display());
                skipped += 1;
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &target)?;
            println!("{} {}", green("wrote"), target.display());
            written += 1;
        }
    }

    Ok((written, skipped))
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn format_string_array(items: &[String]) -> String {
    let escaped: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect();
    format!("[{}]", escaped.join(", "))
}

fn cmd_doctor(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;
    let home_path = PathBuf::from(&home);
    let config_dir = home_path.join(".config");

    let required_paths = [
        config_dir.join("hypr"),
        config_dir.join("hypr/hyprland.lua"),
        config_dir.join("hypr/hexin"),
        config_dir.join("hypr/hexin/settings.lua"),
        config_dir.join("quickshell"),
        config_dir.join("quickshell/hexin"),
        config_dir.join("hexin"),
        config_dir.join("hexin/settings.json"),
        config_dir.join("matugen/templates"),
        home_path.join(".local/bin/hexin-matugen-apply"),
        home_path.join(".local/bin/hexin-theme-sync"),
    ];

    let required_bins = [
        // Core & Hyprland / Quickshell
        "hexin",
        "hyprctl",
        "qs",
        "hypridle",
        "hyprsunset",
        "hyprpicker",
        // Desktop portals & Wayland utilities
        "wl-copy",
        "wl-paste",
        "cliphist",
        "brightnessctl",
        "grim",
        "slurp",
        // Audio, network, bluetooth, power, notifications
        "pipewire",
        "wireplumber",
        "nmcli",
        "bluetoothctl",
        "upower",
        "notify-send",
        // Apps & system tools
        "kitty",
        "nautilus",
        "pavucontrol",
        "nvim",
        "matugen",
        "python",
        "lua",
        "ydotool",
        "gpu-screen-recorder",
    ];

    let mut missing_paths: Vec<String> = Vec::new();
    let mut missing_bins: Vec<String> = Vec::new();
    let mut broken_symlinks: Vec<String> = Vec::new();

    for path in required_paths {
        if path.exists() {
            if !json {
                println!("[{}]   path exists: {}", green("OK"), path.display());
            }
        } else {
            if !json {
                println!("[{}] missing path: {}", red("FAIL"), path.display());
            }
            missing_paths.push(path.display().to_string());
        }
    }

    for bin in required_bins {
        let found = bin_in_path(bin);
        if found {
            if !json {
                println!("[{}]   binary found: {bin}", green("OK"));
            }
        } else {
            if !json {
                println!("[{}] binary not found in PATH: {bin}", yellow("WARN"));
            }
            missing_bins.push(bin.to_string());
        }
    }

    if config_dir.is_dir() {
        for entry in fs::read_dir(&config_dir)? {
            let path = entry?.path();
            if let Ok(meta) = path.symlink_metadata() {
                if meta.file_type().is_symlink() && path.metadata().is_err() {
                    let display = path.display().to_string();
                    if !json {
                        println!("[BROKEN SYMLINK] {display}");
                    }
                    broken_symlinks.push(display);
                }
            }
        }
    }

    let issues = missing_paths.len() + missing_bins.len() + broken_symlinks.len();
    let healthy = issues == 0;

    if json {
        println!(
            "{{\n  \"healthy\": {},\n  \"issues\": {},\n  \"missing_paths\": {},\n  \"missing_bins\": {},\n  \"broken_symlinks\": {}\n}}",
            healthy,
            issues,
            format_string_array(&missing_paths),
            format_string_array(&missing_bins),
            format_string_array(&broken_symlinks),
        );
    } else if healthy {
        println!("\nEverything looks healthy.");
    }

    if issues > 0 {
        return Err(format!("found {issues} issue(s)").into());
    }

    Ok(())
}

fn cmd_backup(dst: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let required_dirs = [
        "hypr",
        "quickshell",
        "hexin",
        "matugen",
        "kitty",
        "gtk-3.0",
        "gtk-4.0",
    ];
    let found_dirs = existing_config_dirs(&required_dirs)?;
    let home = std::env::var("HOME")?;
    let base_backup_dir = xdg_data_home(Path::new(&home)).join("hexin/backups");

    let backup_dir = match dst {
        Some(path) => {
            make_folder(&path)?;
            path
        }
        None => create_backup_dir(&base_backup_dir)?,
    };
    println!("Created backup folder in {}", backup_dir.display());

    for src_dir in found_dirs {
        if let Some(name) = src_dir.file_name() {
            let dst_dir = backup_dir.join(name);
            fs::create_dir_all(&dst_dir)?;
            let _ = copy_dir_walk(&src_dir, &src_dir, &dst_dir, true)?;
        }
    }

    println!("Backed up found configs to {}", backup_dir.display());
    Ok(())
}

struct RestoreSummary {
    written_files: usize,
    skipped_files: usize,
    touched_dirs: usize,
}

fn cmd_restore(src: Option<PathBuf>, force: bool, list: bool) -> Result<(), Box<dyn std::error::Error>> {
    let required_dirs = [
        "hypr",
        "quickshell",
        "hexin",
        "matugen",
        "kitty",
        "gtk-3.0",
        "gtk-4.0",
    ];
    let home = std::env::var("HOME")?;
    let home_path = PathBuf::from(home);
    let backups_root = home_path.join(".local/share/hexin/backups");

    if list {
        if !backups_root.exists() {
            println!("No backups directory found at {}", backups_root.display());
            return Ok(());
        }

        let mut entries: Vec<PathBuf> = fs::read_dir(&backups_root)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        entries.sort();
        entries.reverse();

        if entries.is_empty() {
            println!("No backups found in {}", backups_root.display());
        } else {
            println!("Backups in {} (newest first):", backups_root.display());
            for entry in entries {
                println!("  {}", entry.display());
            }
        }
        return Ok(());
    }

    if src.is_none() && !has_any_entries(&backups_root)? {
        return Err(format!(
            "Backups not found in {} (Have you ran 'hexin backup' before?)",
            backups_root.display()
        )
        .into());
    }

    let backup_dir = match src {
        Some(path) => {
            if !path.is_dir() {
                return Err(format!(
                    "Backup path does not exist or is not a directory: {}",
                    path.display()
                )
                .into());
            }
            path
        }
        None => find_latest_backup(&backups_root)?.ok_or_else(|| {
            format!(
                "No valid backup found in {} (Have you run `hexin backup` yet?)",
                backups_root.display()
            )
        })?,
    };

    if force {
        let safety_dir = create_backup_dir(&backups_root)?;
        println!(
            "Created pre-restore safety backup in {}",
            safety_dir.display()
        );
        cmd_backup(Some(safety_dir))?;
    }

    let config_root = home_path.join(".config");
    let mut summary = RestoreSummary {
        written_files: 0,
        skipped_files: 0,
        touched_dirs: 0,
    };

    for name in required_dirs {
        let src_dir = backup_dir.join(name);
        if !src_dir.is_dir() {
            continue;
        }

        let dst_dir = config_root.join(name);
        fs::create_dir_all(&dst_dir)?;

        let (written, skipped) = copy_dir_walk(&src_dir, &src_dir, &dst_dir, force)?;
        summary.written_files += written;
        summary.skipped_files += skipped;
        if written > 0 {
            summary.touched_dirs += 1;
        }
    }

    if summary.written_files == 0 && summary.skipped_files == 0 {
        println!(
            "No matching config directories found in {}",
            backup_dir.display()
        );
    } else if summary.written_files == 0 {
        println!(
            "0 files written (skipped {} existing file(s); pass `--force` to overwrite)",
            summary.skipped_files
        );
    } else {
        println!(
            "Restored {} file(s) across {} dir(s) from {} (skipped {})",
            summary.written_files,
            summary.touched_dirs,
            backup_dir.display(),
            summary.skipped_files
        );
        reload_desktop_after_restore()?;
    }

    Ok(())
}

fn existing_config_dirs(candidates: &[&str]) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;
    let config_root = PathBuf::from(home).join(".config");

    let mut existing: Vec<PathBuf> = Vec::new();
    for name in candidates {
        let full_path = config_root.join(name);
        if full_path.exists() {
            existing.push(full_path);
        }
    }

    Ok(existing)
}

fn make_folder(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(path)?;
    Ok(())
}

fn use_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    io::stdout().is_terminal()
}

fn green(s: &str) -> String {
    if use_color() {
        format!("\x1b[38;2;80;220;120m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn yellow(s: &str) -> String {
    if use_color() {
        format!("\x1b[38;2;245;195;60m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn red(s: &str) -> String {
    if use_color() {
        format!("\x1b[38;2;240;80;80m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn create_backup_dir(backup_root: &Path) -> std::io::Result<PathBuf> {
    let stamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    let dest = backup_root.join(format!("backup-{stamp}"));
    fs::create_dir_all(&dest)?;
    Ok(dest)
}

fn find_latest_backup(backups_root: &Path) -> Result<Option<PathBuf>, std::io::Error> {
    if !backups_root.exists() {
        return Ok(None);
    }

    let mut backups: Vec<PathBuf> = fs::read_dir(backups_root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|name| name.starts_with("backup-"))
                    .unwrap_or(false)
        })
        .collect();

    backups.sort();
    Ok(backups.pop())
}

fn has_any_entries(root: &Path) -> std::io::Result<bool> {
    if !root.exists() {
        return Ok(false);
    }

    let mut entries = fs::read_dir(root)?;
    Ok(entries.next().transpose()?.is_some())
}

fn reload_desktop_after_restore() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Reload Hyprland config
    let hypr_status = Command::new("hyprctl").arg("reload").status()?;
    if !hypr_status.success() {
        eprintln!("Warning: `hyprctl reload` exited with status {hypr_status}");
    }

    // 2. Stop existing qs / quickshell instances (ignore if none were running)
    let _ = Command::new("qs")
        .args(["kill", "-c", "hexin"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let home = std::env::var("HOME")?;
    let home_path = PathBuf::from(home);
    // 3. Start Quickshell detached so `hexin restore` doesn't hang
    spawn_quickshell(&home_path)?;

    Ok(())
}

fn bin_in_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

fn count_command_lines(cmd: &str, args: &[&str]) -> usize {
    match Command::new(cmd).args(args).output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        Err(_) => 0,
    }
}

fn cmd_check_updates() -> Result<(), Box<dyn std::error::Error>> {
    let repo_updates = if bin_in_path("checkupdates") {
        count_command_lines("checkupdates", &[])
    } else {
        eprintln!("Warning: `checkupdates` not found (install `pacman-contrib` for repo update counts)");
        0
    };

    let aur_updates = if bin_in_path("yay") {
        count_command_lines("yay", &["-Qua"])
    } else if bin_in_path("paru") {
        count_command_lines("paru", &["-Qua"])
    } else {
        0
    };

    let flatpak_updates = if bin_in_path("flatpak") {
        count_command_lines("flatpak", &["remote-ls", "--updates"])
    } else {
        0
    };

    let total = repo_updates + aur_updates + flatpak_updates;

    println!("Repo updates:    {repo_updates}");
    println!("AUR updates:     {aur_updates}");
    println!("Flatpak updates: {flatpak_updates}");
    println!("Total available: {total}");

    Ok(())
}

fn reboot_required() -> bool {
    let Ok(output) = Command::new("uname").arg("-r").output() else {
        return false;
    };
    let running = output.stdout.as_slice();
    let running_str = String::from_utf8_lossy(running).trim();
    if running_str.is_empty() {
        return false;
    }
    !Path::new("/usr/lib/modules").join(running_str).exists()
}

fn xdg_data_home(home_path: &Path) -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(val) if !val.is_empty() => PathBuf::from(val),
        _ => home_path.join(".local/share"),
    }
}

fn spawn_quickshell(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let state_dir = xdg_state_home(home).join("hexin");
    fs::create_dir_all(&state_dir)?;

    let log_path = state_dir.join("shell.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let log_err = log_file.try_clone()?;

    Command::new("setsid")
        .arg("-f") // fork and run in a new session (if supported) or `setsid qs ...`
        .arg("qs")
        .arg("-c")
        .arg("hexin")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .spawn()?;

    Ok(())
}

fn xdg_state_home(home: &Path) -> PathBuf {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(val) if !val.is_empty() => PathBuf::from(val),
        _ => home.join(".local/state"),
    }
}
