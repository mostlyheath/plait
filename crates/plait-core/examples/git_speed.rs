use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Instant,
};

use plait_core::{
    file_view, git,
    host::{Client, git as host_git},
    model::DiffMode,
};

fn tracked_file(root: &Path) -> Result<PathBuf, String> {
    if root.join("README.md").is_file() {
        return Ok(PathBuf::from("README.md"));
    }
    let output = Command::new("git")
        .args(["--no-optional-locks", "-C"])
        .arg(root)
        .args(["ls-files", "-z", "--cached"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .output()
        .map_err(|e| format!("listing tracked files: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "listing tracked files: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    use std::os::unix::ffi::OsStrExt;
    output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(OsStr::from_bytes(p)))
        .find(|p| root.join(p).is_file())
        .ok_or_else(|| "checkout has no readable tracked file".into())
}

fn measure(
    mut direct: impl FnMut() -> Result<(), String>,
    mut host: impl FnMut() -> Result<(), String>,
    iterations: usize,
) -> Result<(f64, f64, f64, f64), String> {
    let mut direct_times = Vec::with_capacity(iterations);
    let mut host_times = Vec::with_capacity(iterations);
    for i in 0..iterations + 3 {
        let start = Instant::now();
        direct()?;
        let d = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        host()?;
        let h = start.elapsed().as_secs_f64() * 1000.0;
        if i >= 3 {
            direct_times.push(d);
            host_times.push(h);
        }
    }
    direct_times.sort_by(f64::total_cmp);
    host_times.sort_by(f64::total_cmp);
    let median = |v: &[f64]| {
        if v.len().is_multiple_of(2) {
            (v[v.len() / 2 - 1] + v[v.len() / 2]) / 2.0
        } else {
            v[v.len() / 2]
        }
    };
    let p95 = |v: &[f64]| v[(v.len() * 95).div_ceil(100) - 1];
    Ok((
        median(&direct_times),
        p95(&direct_times),
        median(&host_times),
        p95(&host_times),
    ))
}

fn run() -> Result<bool, String> {
    let args: Vec<_> = env::args_os().collect();
    if !(3..=4).contains(&args.len()) {
        return Err("usage: git_speed <host-socket> <checkout> [iterations=30]".into());
    }
    let socket = Path::new(&args[1]);
    if !socket.is_absolute() || socket.file_name() != Some(OsStr::new("host.sock")) {
        return Err("host socket must be an absolute path ending in host.sock".into());
    }
    let root = std::fs::canonicalize(&args[2]).map_err(|e| format!("checkout: {e}"))?;
    let iterations: usize = args
        .get(3)
        .map(|s| s.to_string_lossy().parse())
        .transpose()
        .map_err(|_| "iterations must be a positive integer")?
        .unwrap_or(30);
    if iterations == 0 || iterations > usize::MAX - 3 {
        return Err("iterations must be a positive integer".into());
    }
    // This single-threaded example sets the store before creating the client.
    unsafe {
        env::set_var(
            "PLAIT_HOST_STORE",
            socket.parent().ok_or("socket has no parent")?,
        );
    }
    let client = Client::from_environment().map_err(|e| e.to_string())?;
    let status = git::status(&root)?;
    let change = status.changes.first();
    let file = tracked_file(&root)?;
    let mut failed = Vec::new();
    println!(
        "{:<12} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "read", "direct med", "direct p95", "host med", "host p95", "added med"
    );
    println!(
        "{:12} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "", "ms", "ms", "ms", "ms", "ms"
    );
    let mut row = |name, result: Result<(f64, f64, f64, f64), String>| -> Result<(), String> {
        let (dm, dp, hm, hp) = result.map_err(|e| format!("{name}: {e}"))?;
        let added = hm - dm;
        println!("{name:<12} {dm:>12.2} {dp:>12.2} {hm:>12.2} {hp:>12.2} {added:>12.2}");
        if added > 20.0 {
            failed.push(name);
        }
        Ok(())
    };
    row(
        "status",
        measure(
            || git::status(&root).map(|_| ()),
            || {
                host_git::status(&client, &root, None)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            iterations,
        ),
    )?;
    if let Some(change) = change {
        let mode = if change.unstaged() {
            DiffMode::Unstaged
        } else {
            DiffMode::Staged
        };
        row(
            "diff",
            measure(
                || git::diff(&root, change, mode).map(|_| ()),
                || {
                    host_git::diff(&client, &root, None, change, mode)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                },
                iterations,
            ),
        )?;
    } else {
        println!("diff         skipped (clean checkout)");
    }
    row(
        "log",
        measure(
            || git::log(&root, None, 0, 100).map(|_| ()),
            || {
                host_git::log(&client, &root, None, None, 0, 100)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            iterations,
        ),
    )?;
    row(
        "branches",
        measure(
            || git::branches(&root).map(|_| ()),
            || {
                host_git::branches(&client, &root, None)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            iterations,
        ),
    )?;
    row(
        "file",
        measure(
            || file_view::read(&root, &file).map(|_| ()),
            || {
                host_git::file(&client, &root, None, &file)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            iterations,
        ),
    )?;
    if failed.is_empty() {
        println!("20 ms gate passed");
    } else {
        println!("20 ms gate failed: {}", failed.join(", "));
    }
    Ok(failed.is_empty())
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
