mod bridge;
mod build_output;
mod command;
mod files;
mod fingerprint;
mod git_read;
mod installation;
mod lease;
#[cfg(target_os = "linux")]
mod linux_service;
mod mcp;
mod record;
mod review;
mod service;
mod startup;
mod tailnet;
mod transport;
mod update;
mod worktree;

use std::{
    env,
    io::{self, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn main() {
    if let Err(error) = run() {
        eprintln!("plait-host: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    command::initialize_cleanup_receipt()?;
    let mut args = env::args().skip(1);
    let operation = args.next().unwrap_or_default();
    if operation == "file" {
        return files::run(args.collect());
    }
    if operation == "lease-prepare" {
        return lease::prepare(args.collect());
    }
    if operation == "lock-fd" {
        return lease::acquire(args.collect());
    }
    if operation == "cleanup" {
        return lease::cleanup_operation(args.collect());
    }
    if operation == "command" {
        return command::run(args.collect());
    }
    if operation == "trusted-command" {
        return command::run_trusted(args.collect());
    }
    if operation == "mcp-exec" {
        return mcp::exec_contained(args.collect());
    }
    if operation == "mcp-server" {
        return mcp::run(args.collect());
    }
    // The host's records of a checkout; JSON on stdin and stdout.
    match operation.as_str() {
        "record" => return record::run(),
        "review" => return review::run(),
        "git-read" => return git_read::run(),
        "worktree" => return worktree::run(),
        "build-output" => return build_output::run(),
        "worktree-git" => return worktree::git_run(),
        "review-contained" => return review::contained(),
        "undo" => return record::undo(),
        "record-worker"
        | "undo-worker"
        | "prune-worker"
        | "review-worker"
        | "worktree-worker"
        | "worktree-git-worker" => {
            return record::worker(&operation);
        }
        "undo-contained" => return record::undo_contained(),
        "record-contained" => return record::run_contained(),
        "prune" => return record::prune(),
        "prune-contained" => return record::prune_contained(),
        _ => {}
    }
    if operation == "serve" || operation == "tailnet" {
        let mut store = env::var_os("PLAIT_HOST_STORE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share/plait")
            });
        let values: Vec<_> = args.collect();
        if operation == "tailnet" {
            if values.len() != 2 {
                return Err(io::Error::other(
                    "usage: plait-host tailnet allow <login>|--me or allow-node <name>",
                ));
            }
            return tailnet::add(&store, &values[0], &values[1]);
        }
        let mut port = 7419;
        let mut enabled = false;
        let mut iter = values.iter();
        while let Some(option) = iter.next() {
            match option.as_str() {
                "--tailnet" => enabled = true,
                "--port" => {
                    port = iter
                        .next()
                        .and_then(|v| v.parse::<u16>().ok())
                        .filter(|p| *p != 0)
                        .ok_or_else(|| io::Error::other("invalid port"))?
                }
                "--store" => {
                    store = iter
                        .next()
                        .ok_or_else(|| io::Error::other("missing store"))?
                        .into()
                }
                _ => return Err(io::Error::other("unknown option")),
            }
        }
        if !enabled || !store.is_absolute() {
            return Err(io::Error::other(
                "serve requires --tailnet and an absolute store path",
            ));
        }
        return tailnet::serve(&store, port);
    }
    if operation == "install-service" || operation == "uninstall-service" {
        let tailnet = match (args.next().as_deref(), args.next()) {
            (None, None) => false,
            (Some("--tailnet"), None) if operation == "install-service" => true,
            _ => return Err(io::Error::other("unknown service option")),
        };
        #[cfg(target_os = "linux")]
        return if operation == "install-service" {
            linux_service::install(tailnet)
        } else {
            linux_service::uninstall()
        };
        #[cfg(not(target_os = "linux"))]
        return {
            let _ = tailnet;
            Err(io::Error::other("service commands are Linux-only"))
        };
    }
    if operation == "version" {
        serde_json::to_writer(
            io::stdout(),
            &serde_json::json!({
                "component": "host", "version": env!("CARGO_PKG_VERSION"),
                "revision": option_env!("PLAIT_BUILD_REVISION").unwrap_or("development"),
                "release": plait_core::update::current_version().map(|version| version.to_string()),
                "protocol_version": 6,
            }),
        )?;
        return Ok(());
    }
    if operation == "update" {
        return update::run(args.collect());
    }
    if operation == "bridge" {
        let mut store = env::var_os("PLAIT_HOST_STORE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share/plait")
            });
        while let Some(option) = args.next() {
            if option != "--store" {
                return Err(io::Error::other("unknown option"));
            }
            store = args
                .next()
                .ok_or_else(|| io::Error::other("missing option value"))?
                .into();
        }
        if !store.is_absolute() {
            return Err(io::Error::other("store path must be absolute"));
        }
        // Unbuffered descriptors: io::Stdin buffers internally, so a partial-
        // frame deadline that polls fd 0 would wait on bytes already read.
        use std::os::fd::AsFd;
        let input = std::fs::File::from(io::stdin().as_fd().try_clone_to_owned()?);
        let output = std::fs::File::from(io::stdout().as_fd().try_clone_to_owned()?);
        return bridge::run(input, output, &store);
    }
    let paths = installation::runtime_paths(&env::current_exe()?)?;
    let service_home = (operation == "run-user")
        .then(service::user_home)
        .transpose()?;
    let mut store = service_home
        .as_ref()
        .map(|home| home.join(".local/share/plait"))
        .or_else(|| env::var_os("PLAIT_HOST_STORE").map(PathBuf::from))
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share/plait")
        });
    let mut runtime = paths.runtime;
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| io::Error::other("missing option value"))?;
        match option.as_str() {
            "--store" => store = value.into(),
            "--runtime" => runtime = value.into(),
            _ => return Err(io::Error::other("unknown option")),
        }
    }
    if !store.is_absolute() || !runtime.is_absolute() {
        return Err(io::Error::other("store and runtime paths must be absolute"));
    }
    match operation.as_str() {
        "run" | "run-user" => {
            transport::serve(&store, &runtime, &paths.node, service_home.as_deref())
        }
        "status" => {
            let reply = transport::status(&store)?;
            io::stdout().write_all(&reply)
        }
        "start" => {
            if transport::status(&store).is_ok() {
                return Ok(());
            }
            drop(transport::private_store(&store)?);
            let mut command = Command::new(env::current_exe()?);
            command
                .args(["run", "--store"])
                .arg(&store)
                .arg("--runtime")
                .arg(runtime)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // Detach from the desktop's process group and terminal lifetime.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = command.spawn()?;
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                if transport::status(&store).is_ok() {
                    return Ok(());
                }
                if child.try_wait()?.is_some() {
                    return Err(io::Error::other(
                        "host startup failed; run in foreground for diagnostics",
                    ));
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(io::Error::other(
                "host readiness timed out; check status before retrying",
            ))
        }
        _ => Err(io::Error::other(
            "usage: plait-host run|run-user|start|status [--store PATH] [--runtime PATH], or plait-host version|update [--check]",
        )),
    }
}
