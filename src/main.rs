use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(name = "hexin", about = "Hexin: The AI Powered Hyprland + Quickshell Config")]
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
    },

    /// Lay Down Main Configs in ~/.config.
    Materialize {
        #[arg(long, default_value = "/usr/share/hexin/configs")]
        src: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Check System and Hexin health.
    Doctor,

    /// Backup Current Config
    Backup {
        #[arg(long)]
        dst: Option<PathBuf>,
    },

    /// Restore Backed Up Config (First run hexin backup).
    Restore {
        #[arg(long)]
        src: Option<PathBuf>,
        #[arg(long)]
        force: bool
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Update { dry_run } => cmd_update(dry_run)?,
        Commands::Materialize { src, force } => cmd_materialize(&src, force)?,
        Commands::Doctor => cmd_doctor()?,
        Commands::Backup { dst } => cmd_backup(dst)?,
        Commands::Restore { src, force } => cmd_restore(src, force)?,
    }

    Ok(())
}

fn cmd_update(dry_run: bool) -> Result<(), Box<dyn std::error::Error>> {
    if dry_run {
        println!("[dry-run] sudo pacman -Syu");
        println!("[dry-run] yay -Syu");
        println!("[dry-run] flatpak update");
        return Ok(());
    }

    let pacman_status = Command::new("sudo")
        .args(["pacman", "-Syu"])
        .status()?;

    if !pacman_status.success() {
        return Err("pacman -Syu failed".into());
    }

    let aur_status = Command::new("yay")
        .args(["-Syu"])
        .status()?;

    if !aur_status.success() {
        return Err("yay -Syu failed".into());
    }

    let flatpak_status = Command::new("flatpak")
        .args(["update"])
        .status()?;

    if !flatpak_status.success() {
        return Err("flatpak update failed".into());
    }

    Ok(())
}

fn cmd_materialize(src: &Path, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !src.is_dir() {
        return Err(format!("source directory not found: {}", src.display()).into());
    }

    let home = std::env::var("HOME")?;
    let dst_base = PathBuf::from(home).join(".config");

    copy_dir_walk(src, src, &dst_base, force)?;
    Ok(())
}

fn copy_dir_walk(
    root: &Path,
    current: &Path,
    dst_base: &Path,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let target = dst_base.join(rel);

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir_all(&target)?;
            copy_dir_walk(root, &path, dst_base, force)?;
        } else if file_type.is_symlink() {
            if !force && target.symlink_metadata().is_ok() {
                println!("{}  {} (already exists)", yellow("skip"), target.display());
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
        } else {
            if !force && target.symlink_metadata().is_ok() {
                println!("{}  {} (already exists)", yellow("skip"), target.display());
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &target)?;
            println!("{} {}", green("wrote"), target.display());
        }
    }
    Ok(())
}

fn cmd_doctor() -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;
    let config_dir = PathBuf::from(&home).join(".config");
    let required_dirs = [config_dir.join("hypr"), config_dir.join("quickshell"), config_dir.join("hexin")];
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
        // Apps & dev/system tools
        "kitty",
        "nautilus",
        "pavucontrol",
        "spotify-launcher",
        "nvim",
        "matugen",
        "python",
        "lua",
        "ydotool",
        "gpu-screen-recorder",
        "qt6ct",
        "git",
    ];

    let mut issues = 0;

    for dir in required_dirs {
        if dir.is_dir() {
            println!("[{}]   config dir: {}", green("OK"), dir.display());
        } else {
            println!("[{}] missing config dir: {}", red("FAIL"), dir.display());
            issues += 1;
        }
    }

    for bin in required_bins {
        let found = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
            .unwrap_or(false);

        if found {
            println!("[{}]   binary found: {bin}", green("OK"));
        } else {
            println!("[{}] binary not found in PATH: {bin}", yellow("WARN"));
            issues += 1;
        }
    }

    if config_dir.is_dir() {
        for entry in fs::read_dir(&config_dir)? {
            let path = entry?.path();
            if let Ok(meta) = path.symlink_metadata() {
                if meta.file_type().is_symlink() && path.metadata().is_err() {
                    println!("[BROKEN SYMLINK] {}", path.display());
                    issues += 1;
                }
            }
        }
    }

    if issues > 0 {
        return Err(format!("found {issues} issue(s)").into());
    }

    println!("\nEverything looks healthy.");
    Ok(())
}

fn cmd_backup(dst: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let required_dirs = ["hypr", "quickshell", "rofi", "waybar", "fuzzel", "swaync"];
    let found_dirs = existing_config_dirs(&required_dirs)?;
    let home = std::env::var("HOME")?;
    let backup_dir = match dst {
        Some(path) => path,
        None => {
            PathBuf::from(home).join(".config/hexin-backup")
        }
    };

    make_folder(&backup_dir)?;
    println!("Created backup folder in {}", backup_dir.display());

    for src_dir in found_dirs {
        if let Some(name) = src_dir.file_name() {
            let dst_dir = backup_dir.join(name);
            fs::create_dir_all(&dst_dir)?;
            copy_dir_walk(&src_dir, &src_dir, &dst_dir, true)?;
        }
    }

    println!("Backed up found configs to {}", backup_dir.display());
    Ok(())
}

fn cmd_restore(src: Option<PathBuf>, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    let required_dirs = ["hypr", "quickshell", "rofi", "waybar", "fuzzel", "swaync"];
    let home = std::env::var("HOME")?;
    let home_path = PathBuf::from(home);

    let backup_dir = match src {
        Some(path) => path,
        None => home_path.join(".config/hexin-backup"),
    };

    if !backup_dir.is_dir() {
        return Err(format!("backup directory not found: {} (Have you ran 'hexin backup' before?)", backup_dir.display()).into());
    }

    let config_root = home_path.join(".config");
    let mut restored_count = 0;

    for name in required_dirs {
        let src_dir = backup_dir.join(name);
        if !src_dir.is_dir() {
            continue;
        }

        let dst_dir = config_root.join(name);
        fs::create_dir_all(&dst_dir)?;
        copy_dir_walk(&src_dir, &src_dir, &dst_dir, force)?;
        restored_count += 1;
    }

    if restored_count == 0 {
        println!(
            "No matching config directories found in {}",
            backup_dir.display()
        );
    } else {
        println!(
            "Restored {restored_count} config dir(s) from {}",
            backup_dir.display()
        );
    }

    Ok(())
}

fn existing_config_dirs(candidates: &[&str]) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;
    let config_root = PathBuf::from(home).join(".config");

    // Array/Vec A starts empty
    let mut existing: Vec<PathBuf> = Vec::new();

    // Loop through array B (candidates)
    for name in candidates {
        let full_path = config_root.join(name);

        // Only add to A if it actually exists on disk
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

fn green(s: &str) -> String {
    format!("\x1b[38;2;80;220;120m{s}\x1b[0m")
}

fn yellow(s: &str) -> String {
    format!("\x1b[38;2;245;195;60m{s}\x1b[0m")
}

fn red(s: &str) -> String {
    format!("\x1b[38;2;240;80;80m{s}\x1b[0m")
}
