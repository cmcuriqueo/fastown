use std::ffi::CString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueHint};
use rayon::iter::{ParallelBridge, ParallelIterator};
use rayon::ThreadPoolBuilder;
use walkdir::WalkDir;

#[derive(Debug, Parser)]
#[command(
    name = "fastown",
    version,
    about = "a parallel chown-like utility for large directory trees",
    long_about = "A lightweight Rust MVP for traversing large trees and applying ownership changes without spawning one process per file."
)]
struct Cli {
    #[arg(
        value_name = "OWNER[:GROUP]",
        help = "Target owner and optional group, e.g. alice or alice:staff",
        value_hint = ValueHint::Other
    )]
    owner: String,

    #[arg(
        value_name = "PATH",
        help = "Root path to traverse",
        value_hint = ValueHint::DirPath
    )]
    path: PathBuf,

    #[arg(
        short = 'j',
        long = "jobs",
        default_value = "4",
        value_name = "N|auto",
        help = "Worker count; use 'auto' for the machine default"
    )]
    jobs: String,

    #[arg(long, help = "Show what would change without modifying anything")]
    dry_run: bool,

    #[arg(long, help = "Follow symlinks while descending the directory tree")]
    follow_symlinks: bool,
}

#[derive(Debug)]
struct OwnerSpec {
    uid: u32,
    gid: Option<u32>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("fastown: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    let jobs = parse_jobs(&cli.jobs)?;
    ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build_global()
        .context("failed to initialize the worker pool")?;

    let owner = resolve_owner(&cli.owner)?;
    let root = cli.path.canonicalize().unwrap_or_else(|_| cli.path.clone());

    let start = Instant::now();
    let processed = AtomicUsize::new(0);
    let errors = AtomicUsize::new(0);

    let walk = WalkDir::new(&root)
        .follow_links(cli.follow_symlinks)
        .into_iter()
        .filter_map(|entry| entry.ok());

    walk.par_bridge().for_each(|entry| {
        let path = entry.path();

        match apply_ownership(path, &owner, cli.dry_run, cli.follow_symlinks) {
            Ok(_) => {
                processed.fetch_add(1, Ordering::Relaxed);
            }
            Err(err) => {
                eprintln!("{}: {err}", path.display());
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    });

    let elapsed = start.elapsed();
    let processed_total = processed.load(Ordering::Relaxed);
    let error_total = errors.load(Ordering::Relaxed);

    println!("processed: {} entries", processed_total);
    println!("errors: {}", error_total);
    println!("elapsed: {}", format_duration(elapsed));

    if error_total > 0 {
        bail!("{} path(s) failed to update", error_total);
    }

    Ok(())
}

fn parse_jobs(raw: &str) -> Result<usize> {
    if raw.eq_ignore_ascii_case("auto") {
        return Ok(num_cpus::get());
    }

    let jobs = raw.parse::<usize>().with_context(|| {
        format!("invalid worker count '{raw}'; expected a positive integer or 'auto'")
    })?;

    if jobs == 0 {
        bail!("job count must be greater than zero");
    }

    Ok(jobs)
}

fn resolve_owner(spec: &str) -> Result<OwnerSpec> {
    let (user_name, group_name) = match spec.split_once(':') {
        Some((user, group)) => (user, Some(group)),
        None => (spec, None),
    };

    if user_name.is_empty() {
        bail!("owner cannot be empty");
    }

    let uid = lookup_uid(user_name)?;
    let gid = match group_name {
        Some(group) if !group.is_empty() => Some(lookup_gid(group)?),
        _ => None,
    };

    Ok(OwnerSpec { uid, gid })
}

fn lookup_uid(username: &str) -> Result<u32> {
    let name = CString::new(username)
        .with_context(|| format!("owner name contains embedded NUL: '{username}'"))?;

    unsafe {
        let entry = libc::getpwnam(name.as_ptr());
        if entry.is_null() {
            bail!("unknown user: '{username}'");
        }

        Ok((*entry).pw_uid)
    }
}

fn lookup_gid(groupname: &str) -> Result<u32> {
    let name = CString::new(groupname)
        .with_context(|| format!("group name contains embedded NUL: '{groupname}'"))?;

    unsafe {
        let entry = libc::getgrnam(name.as_ptr());
        if entry.is_null() {
            bail!("unknown group: '{groupname}'");
        }

        Ok((*entry).gr_gid)
    }
}

fn apply_ownership(path: &Path, owner: &OwnerSpec, dry_run: bool, follow_symlinks: bool) -> Result<()> {
    if dry_run {
        println!("DRY RUN: {} -> {}:{}", path.display(), owner.uid, owner.gid.unwrap_or(u32::MAX));
        return Ok(());
    }

    let path_str = path
        .to_str()
        .context("path is not valid UTF-8; cannot pass it to libc")?;
    let c_path = CString::new(path_str)
        .with_context(|| format!("path contains a NUL byte: '{}'", path.display()))?;

    let gid = match owner.gid {
        Some(gid) => gid as libc::gid_t,
        None => libc::gid_t::MAX,
    };

    let flags = if follow_symlinks {
        0
    } else {
        libc::AT_SYMLINK_NOFOLLOW
    };

    let rc = unsafe {
        libc::fchownat(
            libc::AT_FDCWD,
            c_path.as_ptr(),
            owner.uid as libc::uid_t,
            gid,
            flags,
        )
    };

    if rc != 0 {
        let err = io::Error::last_os_error();
        return Err(err).with_context(|| format!("cannot set owner on '{}'", path.display()));
    }

    Ok(())
}

fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 1.0 {
        format!("{:.2} ms", duration.as_secs_f64() * 1000.0)
    } else {
        format!("{:.2}s", secs)
    }
}
