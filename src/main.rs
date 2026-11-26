//! Hexin — the command line helper for the Hexin desktop.
//!
//!   hexin update       full system update (pacman + AUR + Flatpak)
//!   hexin materialize  copy the shipped configs into ~/.config
//!   hexin doctor       check the installation and the running session
//!   hexin backup       snapshot the configs
//!   hexin restore      put a snapshot back
//!
//! Everything that touches your files prints what it does. `update --dry-run`
//! and `materialize --dry-run` show the plan without changing anything.
//!
//! Layout the commands expect:
//!   configs tree   /usr/share/hexin/configs  (mirrors ~/.config)
//!   helper scripts /usr/share/hexin/bin      (copied to ~/.local/bin)
//!   backups        $XDG_DATA_HOME/hexin/backups/backup-<timestamp>
//!   shell log      $XDG_STATE_HOME/hexin/shell.log

use chrono::Local;
use clap::{Parser, Subcommand};
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const APP: &str = "hexin";
const SHELL_CONFIG: &str = "hexin";

/// Config directories (relative to ~/.config) that belong to Hexin.
const CONFIG_DIRS: &[&str] = &[
    "hypr",
    "quickshell",
    "hexin",
    "matugen",
    "kitty",
    "gtk-3.0",
    "gtk-4.0",
];

/// Files (relative to $HOME) that are part of Hexin but live outside ~/.config.
const EXTRA_FILES: &[&str] = &[
    ".local/bin/hexin-theme-sync",
    ".local/bin/hexin-matugen-apply",
];

/// Files that `materialize` never overwrites unless `--force-settings` is used.
const PROTECTED_FILES: &[&str] = &["hexin/settings.json", "hypr/hexin/settings.lua"];

const DEFAULT_CONFIGS_SRC: &str = "/usr/share/hexin/configs";
const DEFAULT_BIN_SRC: &str = "/usr/share/hexin/bin";

#[derive(Parser)]
#[command(
    name = "hexin",
    version,
    about = "Hexin: The AI Powered Hyprland + Quickshell Config",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a full system update (pacman, AUR and Flatpak).
    Update {
        /// Print the commands instead of running them.
        #[arg(long)]
        dry_run: bool,
        /// Only show how many updates are available.
        #[arg(long)]
        check: bool,
        /// Pass --noconfirm / --noninteractive to the package managers.
        #[arg(long)]
        yes: bool,
    },

    /// Lay down the shipped configs in ~/.config.
    Materialize {
        /// Directory that mirrors ~/.config.
        #[arg(long, default_value = DEFAULT_CONFIGS_SRC)]
        src: PathBuf,
        /// Directory with helper scripts for ~/.local/bin.
        #[arg(long, default_value = DEFAULT_BIN_SRC)]
        bin_src: PathBuf,
        /// Overwrite files that already exist.
        #[arg(long)]
        force: bool,
        /// Also overwrite hexin/settings.json and hypr/hexin/settings.lua.
        #[arg(long)]
        force_settings: bool,
        /// Show what would be written without touching anything.
        #[arg(long)]
        dry_run: bool,
        /// Do not restart or reload Hyprland and the shell afterwards.
        #[arg(long)]
        no_reload: bool,
    },

    /// Check the system and the Hexin installation.
    Doctor {
        /// Print a JSON report instead of human readable lines.
        #[arg(long)]
        json: bool,
        /// Treat warnings (optional packages, inactive services) as failures.
        #[arg(long)]
        strict: bool,
    },

    /// Snapshot the current configs.
    Backup {
        /// Write the backup here instead of the timestamped default folder.
        #[arg(long)]
        dst: Option<PathBuf>,
    },

    /// Restore a backup (run `hexin backup` first).
    Restore {
        /// Restore this folder instead of the newest backup.
        #[arg(long)]
        src: Option<PathBuf>,
        /// Overwrite existing files (a safety backup is taken first).
        #[arg(long)]
        force: bool,
        /// List the available backups and exit.
        #[arg(long)]
        list: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Update {
            dry_run,
            check,
            yes,
        } => cmd_update(dry_run, check, yes),
        Commands::Materialize {
            src,
            bin_src,
            force,
            force_settings,
            dry_run,
            no_reload,
        } => cmd_materialize(&src, &bin_src, force, force_settings, dry_run, no_reload),
        Commands::Doctor { json, strict } => cmd_doctor(json, strict),
        Commands::Backup { dst } => cmd_backup(dst),
        Commands::Restore { src, force, list } => cmd_restore(src, force, list),
    }
}

// ───────────────────────────── paths ─────────────────────────────

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home() -> Result<PathBuf> {
    env_path("HOME").ok_or_else(|| "HOME is not set".into())
}

fn config_root() -> Result<PathBuf> {
    match env_path("XDG_CONFIG_HOME") {
        Some(path) => Ok(path),
        None => Ok(home()?.join(".config")),
    }
}

fn data_root() -> Result<PathBuf> {
    match env_path("XDG_DATA_HOME") {
        Some(path) => Ok(path),
        None => Ok(home()?.join(".local/share")),
    }
}

fn state_root() -> Result<PathBuf> {
    match env_path("XDG_STATE_HOME") {
        Some(path) => Ok(path),
        None => Ok(home()?.join(".local/state")),
    }
}

fn backups_root() -> Result<PathBuf> {
    Ok(data_root()?.join(APP).join("backups"))
}

// ───────────────────────────── update ─────────────────────────────

fn cmd_update(dry_run: bool, check: bool, yes: bool) -> Result<()> {
    let aur = aur_helper();
    let flatpak = if bin_in_path("flatpak") {
        Some("flatpak")
    } else {
        None
    };

    if dry_run {
        let mut pacman = vec!["pacman", "-Syu"];
        if yes {
            pacman.push("--noconfirm");
        }
        println!("[dry-run] sudo {}", pacman.join(" "));

        match aur {
            Some(helper) => {
                let mut args = vec![helper, "-Sua"];
                if yes {
                    args.push("--noconfirm");
                }
                println!("[dry-run] {}", args.join(" "));
            }
            None => warn("neither `yay` nor `paru` found; skipping the AUR update"),
        }

        match flatpak {
            Some(_) => println!(
                "[dry-run] flatpak update{}",
                if yes { " --noninteractive" } else { "" }
            ),
            None => warn("Flatpak is not installed; skipping the Flatpak update"),
        }
        return Ok(());
    }

    if check {
        println!("Checking for updates…");
        return cmd_check_updates();
    }

    // Cache sudo credentials once so the update does not stop halfway through.
    let sudo = Command::new("sudo").arg("-v").status();
    match sudo {
        Ok(status) if status.success() => {}
        Ok(_) => return Err("sudo authentication failed".into()),
        Err(error) => return Err(format!("could not run sudo: {error}").into()),
    }

    let mut pacman = vec!["pacman", "-Syu"];
    if yes {
        pacman.push("--noconfirm");
    }
    println!("{} Updating official packages…", green("=>"));
    run_step("sudo", &pacman)?;

    println!("{} Updating AUR packages…", green("=>"));
    match aur {
        Some(helper) => {
            let mut args = vec!["-Sua"];
            if yes {
                args.push("--noconfirm");
            }
            run_step(helper, &args)?;
        }
        None => warn("no AUR helper (`yay` or `paru`) found; skipping the AUR update"),
    }

    println!("{} Updating Flatpak packages…", green("=>"));
    match flatpak {
        Some(_) => {
            let mut args = vec!["update"];
            if yes {
                args.push("--noninteractive");
            }
            run_step("flatpak", &args)?;
        }
        None => warn("Flatpak is not installed; skipping the Flatpak update"),
    }

    println!("{}", green("Updates complete."));
    if reboot_required() {
        println!("Reboot recommended: the running kernel no longer matches /usr/lib/modules.");
    }
    Ok(())
}

fn aur_helper() -> Option<&'static str> {
    if bin_in_path("yay") {
        Some("yay")
    } else if bin_in_path("paru") {
        Some("paru")
    } else {
        None
    }
}

fn run_step(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| format!("could not run `{program}`: {error}"))?;
    if !status.success() {
        return Err(format!("`{program} {}` failed ({status})", args.join(" ")).into());
    }
    Ok(())
}

fn cmd_check_updates() -> Result<()> {
    let repo_updates = if bin_in_path("checkupdates") {
        count_command_lines("checkupdates", &[])
    } else {
        warn("`checkupdates` not found (install `pacman-contrib` for repo update counts)");
        0
    };

    let aur_updates = match aur_helper() {
        Some(helper) => count_command_lines(helper, &["-Qua"]),
        None => 0,
    };

    let flatpak_updates = if bin_in_path("flatpak") {
        count_command_lines("flatpak", &["remote-ls", "--updates"])
    } else {
        0
    };

    println!("Repo updates:    {repo_updates}");
    println!("AUR updates:     {aur_updates}");
    println!("Flatpak updates: {flatpak_updates}");
    println!(
        "Total available: {}",
        repo_updates + aur_updates + flatpak_updates
    );
    Ok(())
}

fn count_command_lines(program: &str, args: &[&str]) -> usize {
    match Command::new(program).args(args).output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        Err(_) => 0,
    }
}

/// A reboot is needed when the running kernel's module tree is gone, which
/// happens when a kernel package replaced it. Comparing version strings is not
/// reliable (6.9 vs 6.10, or several installed kernels).
fn reboot_required() -> bool {
    let Ok(output) = Command::new("uname").arg("-r").output() else {
        return false;
    };
    let running = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if running.is_empty() {
        return false;
    }
    let modules = Path::new("/usr/lib/modules");
    modules.is_dir() && !modules.join(running).is_dir()
}

// ─────────────────────────── materialize ───────────────────────────

#[derive(Clone, Copy)]
struct CopyMode {
    force: bool,
    dry_run: bool,
}

#[derive(Default)]
struct CopyStats {
    written: usize,
    skipped: usize,
    kept: usize,
}

fn cmd_materialize(
    src: &Path,
    bin_src: &Path,
    force: bool,
    force_settings: bool,
    dry_run: bool,
    no_reload: bool,
) -> Result<()> {
    if !src.is_dir() {
        return Err(format!(
            "configs directory not found: {} (pass --src, or install the `hexin` package)",
            src.display()
        )
        .into());
    }

    let home = home()?;
    let dst = config_root()?;
    let mode = CopyMode { force, dry_run };
    let protected: &[&str] = if force_settings { &[] } else { PROTECTED_FILES };

    // Hyprland reads hyprland.conf in preference to the Lua config, so a
    // leftover hyprland.conf would silently disable Hexin.
    if src.join("hypr/hyprland.lua").is_file() {
        move_legacy_hyprland_conf(&dst, dry_run)?;
    }

    // Never overwrite a live config without a snapshot of it.
    if force && !dry_run && has_existing_targets(src, src, &dst)? {
        let safety = create_backup_dir(&backups_root()?)?;
        println!(
            "Backing up the current config to {} first.",
            safety.display()
        );
        cmd_backup(Some(safety))?;
    }

    let mut stats = CopyStats::default();
    copy_tree(src, src, &dst, mode, protected, &mut stats)?;

    let bin_dst = home.join(".local/bin");
    let (bin_written, bin_skipped) = copy_helper_scripts(bin_src, &bin_dst, mode)?;

    println!(
        "Materialized {} file(s) into {}{}{}",
        stats.written + bin_written,
        dst.display(),
        if stats.kept > 0 {
            format!(", kept {} user file(s)", stats.kept)
        } else {
            String::new()
        },
        if dry_run { " (dry run)" } else { "" }
    );
    let skipped = stats.skipped + bin_skipped;
    if skipped > 0 {
        println!("Skipped {skipped} existing file(s); pass --force to overwrite them.");
    }

    if !dry_run && stats.written + bin_written > 0 && !no_reload {
        reload_desktop();
    }
    Ok(())
}

/// Rename `~/.config/hypr/hyprland.conf` out of the way before the Lua config
/// is installed. The file is preserved next to it, never deleted.
fn move_legacy_hyprland_conf(config_root: &Path, dry_run: bool) -> Result<()> {
    let conf = config_root.join("hypr/hyprland.conf");
    if !conf.is_file() {
        return Ok(());
    }

    let backup = unique_sibling(&conf, "hexin-backup");
    if dry_run {
        println!(
            "{} {} -> {}",
            green("would move"),
            conf.display(),
            backup.display()
        );
        return Ok(());
    }

    fs::rename(&conf, &backup)
        .map_err(|error| format!("could not move {} aside: {error}", conf.display()))?;
    println!(
        "{} {} -> {} (Hexin uses hyprland.lua)",
        yellow("moved"),
        conf.display(),
        backup.display()
    );
    Ok(())
}

/// Return `path` with `suffix` appended, adding `-1`, `-2`, … if that name is
/// already taken.
fn unique_sibling(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("backup");
    let mut candidate = path.with_file_name(format!("{file_name}.{suffix}"));
    let mut counter = 1;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{file_name}.{suffix}-{counter}"));
        counter += 1;
    }
    candidate
}

fn copy_tree(
    root: &Path,
    current: &Path,
    dst_base: &Path,
    mode: CopyMode,
    protected: &[&str],
    stats: &mut CopyStats,
) -> Result<()> {
    for entry in fs::read_dir(current)
        .map_err(|error| format!("could not read {}: {error}", current.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let target = dst_base.join(rel);
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            if !mode.dry_run {
                fs::create_dir_all(&target)
                    .map_err(|error| format!("could not create {}: {error}", target.display()))?;
            }
            copy_tree(root, &path, dst_base, mode, protected, stats)?;
            continue;
        }

        let exists = target.symlink_metadata().is_ok();

        // Protected files are only preserved when they already exist. A fresh
        // install must still get the shipped defaults.
        if exists && protected.iter().any(|name| rel == Path::new(*name)) {
            println!(
                "{} {} (kept: user settings)",
                yellow("keep"),
                target.display()
            );
            stats.kept += 1;
            continue;
        }

        if exists && !mode.force {
            println!("{} {} (already exists)", yellow("skip"), target.display());
            stats.skipped += 1;
            continue;
        }

        if mode.dry_run {
            println!("{} {}", green("would write"), target.display());
            stats.written += 1;
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }

        if file_type.is_symlink() {
            if exists {
                fs::remove_file(&target)?;
            }
            let link = fs::read_link(&path)?;
            std::os::unix::fs::symlink(link, &target)
                .map_err(|error| format!("could not link {}: {error}", target.display()))?;
        } else {
            fs::copy(&path, &target).map_err(|error| {
                format!(
                    "could not copy {} to {}: {error}",
                    path.display(),
                    target.display()
                )
            })?;
        }
        println!("{} {}", green("wrote"), target.display());
        stats.written += 1;
    }
    Ok(())
}

/// Copy `bin_src/*` to `~/.local/bin`, always executable.
fn copy_helper_scripts(bin_src: &Path, dst: &Path, mode: CopyMode) -> Result<(usize, usize)> {
    if !bin_src.is_dir() {
        return Ok((0, 0));
    }

    let mut written = 0;
    let mut skipped = 0;
    for entry in fs::read_dir(bin_src)
        .map_err(|error| format!("could not read {}: {error}", bin_src.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let target = dst.join(entry.file_name());
        if target.exists() && !mode.force {
            println!("{} {} (already exists)", yellow("skip"), target.display());
            skipped += 1;
            continue;
        }
        if mode.dry_run {
            println!("{} {}", green("would write"), target.display());
            written += 1;
            continue;
        }

        fs::create_dir_all(dst)?;
        fs::copy(entry.path(), &target)
            .map_err(|error| format!("could not copy {}: {error}", target.display()))?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
        println!("{} {}", green("wrote"), target.display());
        written += 1;
    }
    Ok((written, skipped))
}

fn has_existing_targets(root: &Path, current: &Path, dst_base: &Path) -> Result<bool> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst_base.join(path.strip_prefix(root)?);
        if entry.file_type()?.is_dir() {
            if has_existing_targets(root, &path, dst_base)? {
                return Ok(true);
            }
        } else if target.symlink_metadata().is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

// ───────────────────────────── backup ─────────────────────────────

fn cmd_backup(dst: Option<PathBuf>) -> Result<()> {
    let home = home()?;
    let config = config_root()?;
    let backup_dir = match dst {
        Some(path) => {
            fs::create_dir_all(&path)
                .map_err(|error| format!("could not create {}: {error}", path.display()))?;
            path
        }
        None => create_backup_dir(&backups_root()?)?,
    };

    let mode = CopyMode {
        force: true,
        dry_run: false,
    };
    let mut stats = CopyStats::default();

    for name in CONFIG_DIRS {
        let source = config.join(name);
        if !source.is_dir() {
            continue;
        }
        let target = backup_dir.join(name);
        fs::create_dir_all(&target)?;
        copy_tree(&source, &source, &target, mode, &[], &mut stats)?;
    }

    stats.written += backup_extra_files(&home, &backup_dir)?;
    write_manifest(&backup_dir)?;

    println!(
        "Backed up {} file(s) to {}",
        stats.written,
        backup_dir.display()
    );
    Ok(())
}

fn backup_extra_files(home: &Path, backup_dir: &Path) -> Result<usize> {
    let mut written = 0;
    for rel in EXTRA_FILES {
        let source = home.join(rel);
        if !source.is_file() {
            continue;
        }
        let target = backup_dir.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source, &target)
            .map_err(|error| format!("could not back up {}: {error}", source.display()))?;
        println!("{} {}", green("wrote"), target.display());
        written += 1;
    }
    Ok(written)
}

fn write_manifest(dir: &Path) -> Result<()> {
    let body = format!(
        "{{\n  \"app\": \"{}\",\n  \"version\": \"{}\",\n  \"created\": \"{}\"\n}}\n",
        APP,
        env!("CARGO_PKG_VERSION"),
        Local::now().to_rfc3339()
    );
    fs::write(dir.join("manifest.json"), body)?;
    Ok(())
}

fn create_backup_dir(root: &Path) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    let stamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    let mut dest = root.join(format!("backup-{stamp}"));
    let mut suffix = 1;
    while dest.exists() {
        dest = root.join(format!("backup-{stamp}-{suffix}"));
        suffix += 1;
    }
    fs::create_dir_all(&dest)?;
    Ok(dest)
}

fn find_latest_backup(root: &Path) -> Result<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let mut backups: Vec<PathBuf> = fs::read_dir(root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("backup-"))
                    .unwrap_or(false)
        })
        .collect();
    backups.sort();
    Ok(backups.pop())
}

// ───────────────────────────── restore ─────────────────────────────

fn cmd_restore(src: Option<PathBuf>, force: bool, list: bool) -> Result<()> {
    let home = home()?;
    let config = config_root()?;
    let backups = backups_root()?;

    if list {
        if !backups.is_dir() {
            println!("No backups yet in {}", backups.display());
            return Ok(());
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(&backups)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        entries.sort();
        entries.reverse();

        if entries.is_empty() {
            println!("No backups yet in {}", backups.display());
        } else {
            println!("Backups in {} (newest first):", backups.display());
            for entry in entries {
                println!("  {}", entry.display());
            }
        }
        return Ok(());
    }

    let backup_dir = match src {
        Some(path) => {
            if !path.is_dir() {
                return Err(format!("backup directory not found: {}", path.display()).into());
            }
            path
        }
        None => find_latest_backup(&backups)?.ok_or_else(|| {
            format!(
                "no backups in {} (run `hexin backup` first)",
                backups.display()
            )
        })?,
    };

    // Overwriting a live config without a snapshot is how people lose their
    // settings. Take one first.
    if force {
        let safety = create_backup_dir(&backups)?;
        println!("Saving the current config to {} first.", safety.display());
        cmd_backup(Some(safety))?;
    }

    let mode = CopyMode {
        force,
        dry_run: false,
    };
    let mut stats = CopyStats::default();
    for name in CONFIG_DIRS {
        let source = backup_dir.join(name);
        if !source.is_dir() {
            continue;
        }
        let target = config.join(name);
        fs::create_dir_all(&target)?;
        copy_tree(&source, &source, &target, mode, &[], &mut stats)?;
    }
    let (extra_written, extra_skipped) = restore_extra_files(&home, &backup_dir, mode)?;
    stats.written += extra_written;
    stats.skipped += extra_skipped;

    if stats.written == 0 && stats.skipped == 0 {
        println!("No matching configs found in {}", backup_dir.display());
    } else if stats.written == 0 {
        println!(
            "0 files written (skipped {} existing file(s); pass --force to overwrite)",
            stats.skipped
        );
    } else {
        println!(
            "Restored {} file(s) from {} (skipped {})",
            stats.written,
            backup_dir.display(),
            stats.skipped
        );
        reload_desktop();
    }
    Ok(())
}

fn restore_extra_files(home: &Path, backup_dir: &Path, mode: CopyMode) -> Result<(usize, usize)> {
    let mut written = 0;
    let mut skipped = 0;
    for rel in EXTRA_FILES {
        let source = backup_dir.join(rel);
        if !source.is_file() {
            continue;
        }
        let target = home.join(rel);
        if target.exists() && !mode.force {
            println!("{} {} (already exists)", yellow("skip"), target.display());
            skipped += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source, &target)
            .map_err(|error| format!("could not restore {}: {error}", target.display()))?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
        println!("{} {}", green("wrote"), target.display());
        written += 1;
    }
    Ok((written, skipped))
}

/// Reload Hyprland and the shell after files changed. Best effort: a missing
/// `hyprctl` or a session that is not running must not fail the command.
fn reload_desktop() {
    match Command::new("hyprctl").arg("reload").status() {
        Ok(status) if !status.success() => {
            eprintln!("Warning: `hyprctl reload` failed; is Hyprland running?");
        }
        Err(_) => eprintln!("Warning: `hyprctl` not found; skipped the Hyprland reload."),
        Ok(_) => {}
    }

    if shell_reload() {
        return;
    }
    if let Err(error) = start_shell() {
        eprintln!("Warning: could not start the Hexin shell: {error}");
    }
}

fn shell_reload() -> bool {
    Command::new("qs")
        .args(["ipc", "-c", SHELL_CONFIG, "call", "shell", "reload"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn start_shell() -> Result<()> {
    let _ = Command::new("qs")
        .args(["kill", "-c", SHELL_CONFIG])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let log_dir = state_root()?.join(APP);
    fs::create_dir_all(&log_dir)?;
    let log = log_dir.join("shell.log");
    let stdout = OpenOptions::new().create(true).append(true).open(&log)?;
    let stderr = stdout.try_clone()?;

    let mut command = if bin_in_path("setsid") {
        let mut command = Command::new("setsid");
        // -f always forks, so the shell outlives this process even when it is
        // a process group leader.
        command.args(["-f", "qs"]);
        command
    } else {
        Command::new("qs")
    };
    command
        .args(["-c", SHELL_CONFIG])
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;
    Ok(())
}

// ───────────────────────────── doctor ─────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

struct Finding {
    level: Level,
    message: String,
}

impl Finding {
    fn ok(message: impl Into<String>) -> Self {
        Self {
            level: Level::Ok,
            message: message.into(),
        }
    }

    fn warn(message: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            message: message.into(),
        }
    }

    fn fail(message: impl Into<String>) -> Self {
        Self {
            level: Level::Fail,
            message: message.into(),
        }
    }
}

fn cmd_doctor(json: bool, strict: bool) -> Result<()> {
    let home = home()?;
    let config = config_root()?;
    let mut findings: Vec<Finding> = Vec::new();

    let required_paths = [
        config.join("hypr"),
        config.join("hypr/hyprland.lua"),
        config.join("hypr/hexin"),
        config.join("hypr/hexin/settings.lua"),
        config.join("quickshell"),
        config.join("quickshell/hexin"),
        config.join("hexin"),
        config.join("hexin/settings.json"),
        config.join("matugen/templates"),
        home.join(".local/bin/hexin-matugen-apply"),
        home.join(".local/bin/hexin-theme-sync"),
    ];

    let required_bins = [
        "hexin",
        "hyprctl",
        "qs",
        "hypridle",
        "hyprsunset",
        "hyprpicker",
        "wl-copy",
        "wl-paste",
        "cliphist",
        "brightnessctl",
        "grim",
        "slurp",
        "pipewire",
        "wireplumber",
        "nmcli",
        "bluetoothctl",
        "notify-send",
        "kitty",
        "python",
        "lua",
    ];

    let optional_bins = [
        "matugen",
        "ydotool",
        "nautilus",
        "pavucontrol",
        "nvim",
        "gpu-screen-recorder",
        "spotify-launcher",
        "qt6ct",
        "flatpak",
        "yay",
        "paru",
        "upower",
        "git",
    ];

    for path in required_paths {
        if path.exists() {
            findings.push(Finding::ok(format!("path exists: {}", path.display())));
        } else {
            findings.push(Finding::fail(format!("missing path: {}", path.display())));
        }
    }

    for bin in required_bins {
        if bin_in_path(bin) {
            findings.push(Finding::ok(format!("binary found: {bin}")));
        } else {
            findings.push(Finding::fail(format!("binary not found in PATH: {bin}")));
        }
    }

    for bin in optional_bins {
        if !bin_in_path(bin) {
            findings.push(Finding::warn(format!("optional binary not found: {bin}")));
        }
    }

    if config.is_dir() {
        for entry in fs::read_dir(&config)? {
            let path = entry?.path();
            if let Ok(meta) = path.symlink_metadata()
                && meta.file_type().is_symlink()
                && path.metadata().is_err()
            {
                findings.push(Finding::fail(format!("broken symlink: {}", path.display())));
            }
        }
    }

    findings.push(check_hyprland_config());
    findings.push(check_shell_running());
    findings.push(check_ydotool());
    findings.push(check_uinput());
    findings.push(check_input_group());
    findings.push(check_agent_settings(&home));

    let failures = findings.iter().filter(|f| f.level == Level::Fail).count();
    let warnings = findings.iter().filter(|f| f.level == Level::Warn).count();
    let healthy = failures == 0 && (!strict || warnings == 0);

    if json {
        print_json_report(&findings, failures, warnings, healthy);
    } else {
        for finding in &findings {
            let tag = match finding.level {
                Level::Ok => green("OK"),
                Level::Warn => yellow("WARN"),
                Level::Fail => red("FAIL"),
            };
            println!("[{tag}] {}", finding.message);
        }
        if healthy {
            println!("\nEverything looks healthy.");
        } else {
            println!("\n{failures} failure(s), {warnings} warning(s).");
        }
    }

    if !healthy {
        return Err(format!("found {failures} failure(s) and {warnings} warning(s)").into());
    }
    Ok(())
}

fn print_json_report(findings: &[Finding], failures: usize, warnings: usize, healthy: bool) {
    let items: Vec<String> = findings
        .iter()
        .map(|finding| {
            let level = match finding.level {
                Level::Ok => "ok",
                Level::Warn => "warn",
                Level::Fail => "fail",
            };
            format!(
                "    {{\"level\": \"{level}\", \"message\": \"{}\"}}",
                json_escape(&finding.message)
            )
        })
        .collect();

    println!(
        "{{\n  \"healthy\": {healthy},\n  \"failures\": {failures},\n  \"warnings\": {warnings},\n  \"findings\": [\n{}\n  ]\n}}",
        items.join(",\n")
    );
}

fn check_hyprland_config() -> Finding {
    match Command::new("hyprctl").arg("configerrors").output() {
        Ok(output) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let text = text.trim();
            if !output.status.success() {
                Finding::warn(format!("could not read the Hyprland config: {text}"))
            } else if text.is_empty() {
                Finding::ok("Hyprland config has no errors")
            } else {
                Finding::fail(format!("Hyprland config errors: {text}"))
            }
        }
        Err(_) => Finding::warn("`hyprctl` not found; cannot check the Hyprland config"),
    }
}

fn check_shell_running() -> Finding {
    match Command::new("qs").args(["list", "--all"]).output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.contains(SHELL_CONFIG) {
                Finding::ok("Hexin shell is running")
            } else {
                Finding::warn("Hexin shell is not running (`qs -c hexin`)")
            }
        }
        Ok(_) => Finding::warn("`qs list` failed; cannot check the shell"),
        Err(_) => Finding::warn("`qs` not found; cannot check the shell"),
    }
}

fn check_ydotool() -> Finding {
    match Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "ydotool.service"])
        .status()
    {
        Ok(status) if status.success() => Finding::ok("ydotool.service is active"),
        Ok(_) => Finding::warn("ydotool.service is not active (HexinAgent mouse control needs it)"),
        Err(_) => Finding::warn("`systemctl` not found; cannot check ydotool.service"),
    }
}

fn check_uinput() -> Finding {
    let path = Path::new("/dev/uinput");
    if !path.exists() {
        return Finding::warn("/dev/uinput is missing (load the `uinput` module)");
    }
    match OpenOptions::new().write(true).open(path) {
        Ok(_) => Finding::ok("/dev/uinput is writable"),
        Err(_) => Finding::warn("/dev/uinput is not writable (add your user to the `input` group)"),
    }
}

fn check_input_group() -> Finding {
    match Command::new("id").arg("-nG").output() {
        Ok(output) if output.status.success() => {
            let groups = String::from_utf8_lossy(&output.stdout);
            if groups.split_whitespace().any(|group| group == "input") {
                Finding::ok("user is in the `input` group")
            } else {
                Finding::warn("user is not in the `input` group (ydotool needs /dev/uinput)")
            }
        }
        _ => Finding::warn("could not read the user's groups"),
    }
}

fn check_agent_settings(home: &Path) -> Finding {
    let path = state_root()
        .unwrap_or_else(|_| home.join(".local/state"))
        .join(APP)
        .join("hexinagent/settings.json");
    if !path.is_file() {
        return Finding::warn(format!("HexinAgent settings not found: {}", path.display()));
    }
    match fs::metadata(&path) {
        Ok(meta) => {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                Finding::warn(format!(
                    "{} is mode {mode:o}; API keys should be 600",
                    path.display()
                ))
            } else {
                Finding::ok("HexinAgent settings are private (mode 600)")
            }
        }
        Err(error) => Finding::warn(format!("could not read {}: {error}", path.display())),
    }
}

// ───────────────────────────── helpers ─────────────────────────────

fn bin_in_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| is_executable_file(&dir.join(bin))))
        .unwrap_or(false)
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn use_color() -> bool {
    std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal()
}

fn green(text: &str) -> String {
    paint(text, "80;220;120")
}

fn yellow(text: &str) -> String {
    paint(text, "245;195;60")
}

fn red(text: &str) -> String {
    paint(text, "240;80;80")
}

fn paint(text: &str, rgb: &str) -> String {
    if use_color() {
        format!("\x1b[38;2;{rgb}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn warn(message: &str) {
    eprintln!("{} {message}", yellow("WARN"));
}

fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hexin-test-{}-{name}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn json_escape_handles_control_characters() {
        assert_eq!(json_escape("a\"b\\c\nd\te"), "a\\\"b\\\\c\\nd\\te");
        assert_eq!(json_escape("\u{1}"), "\\u0001");
    }

    #[test]
    fn backup_dirs_are_unique() {
        let root = scratch("unique");
        let first = create_backup_dir(&root).unwrap();
        let second = create_backup_dir(&root).unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn latest_backup_ignores_other_directories() {
        let root = scratch("latest");
        fs::create_dir_all(root.join("backup-2026-01-01_00-00-00")).unwrap();
        fs::create_dir_all(root.join("backup-2026-02-01_00-00-00")).unwrap();
        fs::create_dir_all(root.join("not-a-backup")).unwrap();

        let latest = find_latest_backup(&root).unwrap().unwrap();
        assert_eq!(latest, root.join("backup-2026-02-01_00-00-00"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_backup_root_is_not_an_error() {
        let root = scratch("missing");
        fs::remove_dir_all(&root).unwrap();
        assert!(find_latest_backup(&root).unwrap().is_none());
    }

    #[test]
    fn executability_requires_the_exec_bit() {
        let dir = scratch("exec");
        let file = dir.join("tool");
        fs::write(&file, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable_file(&file));

        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable_file(&file));
        assert!(!is_executable_file(&dir));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn protected_files_are_kept_only_when_present() {
        let root = scratch("protected");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(src.join("hexin")).unwrap();
        fs::write(src.join("hexin/settings.json"), "shipped").unwrap();

        let mode = CopyMode {
            force: true,
            dry_run: false,
        };
        let mut stats = CopyStats::default();
        copy_tree(&src, &src, &dst, mode, PROTECTED_FILES, &mut stats).unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("hexin/settings.json")).unwrap(),
            "shipped"
        );
        assert_eq!(stats.written, 1);

        fs::write(dst.join("hexin/settings.json"), "user").unwrap();
        let mut stats = CopyStats::default();
        copy_tree(&src, &src, &dst, mode, PROTECTED_FILES, &mut stats).unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("hexin/settings.json")).unwrap(),
            "user"
        );
        assert_eq!(stats.kept, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_conf_is_moved_aside_without_clobbering() {
        let root = scratch("legacy");
        let hypr = root.join("hypr");
        fs::create_dir_all(&hypr).unwrap();
        fs::write(hypr.join("hyprland.conf"), "old").unwrap();
        fs::write(hypr.join("hyprland.conf.hexin-backup"), "older").unwrap();

        move_legacy_hyprland_conf(&root, false).unwrap();
        assert!(!hypr.join("hyprland.conf").exists());
        assert_eq!(
            fs::read_to_string(hypr.join("hyprland.conf.hexin-backup")).unwrap(),
            "older"
        );
        assert_eq!(
            fs::read_to_string(hypr.join("hyprland.conf.hexin-backup-1")).unwrap(),
            "old"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dry_run_does_not_move_the_legacy_conf() {
        let root = scratch("legacy-dry");
        let hypr = root.join("hypr");
        fs::create_dir_all(&hypr).unwrap();
        fs::write(hypr.join("hyprland.conf"), "old").unwrap();

        move_legacy_hyprland_conf(&root, true).unwrap();
        assert!(hypr.join("hyprland.conf").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_targets_detects_a_live_config() {
        let root = scratch("targets");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(src.join("hexin")).unwrap();
        fs::write(src.join("hexin/settings.json"), "{}").unwrap();

        assert!(!has_existing_targets(&src, &src, &dst).unwrap());
        fs::create_dir_all(dst.join("hexin")).unwrap();
        fs::write(dst.join("hexin/settings.json"), "{}").unwrap();
        assert!(has_existing_targets(&src, &src, &dst).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
