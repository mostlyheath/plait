//! MCP keeps stdin for JSON-RPC and uses a separate pipe as its lifetime lease.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    os::{fd::FromRawFd, unix::process::CommandExt},
    process::{Command, Stdio},
};

pub fn run(args: Vec<String>) -> io::Result<()> {
    if args.len() < 3 {
        return Err(io::Error::other(
            "mcp-server expects checkout profile program [arguments]",
        ));
    }
    let command = prepare(&args).inspect_err(|_| crate::command::cleanup_receipt("clean\n"))?;
    // The guardian retains both writer leases. No private descriptor reaches a server.
    close_descriptors(args[1] != "trusted");
    if unsafe { libc::fcntl(5, libc::F_GETFD) } < 0 {
        return Err(io::Error::other("MCP lifetime lease is missing"));
    }
    let lifetime = unsafe { File::from_raw_fd(5) };
    crate::command::guard(command, "MCP", lifetime, Stdio::inherit())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn prepare(args: &[String]) -> io::Result<std::process::Command> {
    let mut command = if args[1] == "trusted" {
        let mut command = Command::new(&args[2]);
        command.env_clear().envs(environment()?);
        command
    } else {
        prepare_sandbox(&args[1], &args[2])?
    };
    command.args(&args[3..]).current_dir(&args[0]);
    Ok(command)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn prepare(_: &[String]) -> io::Result<std::process::Command> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "MCP processes require the macOS command policy",
    ))
}

#[cfg(target_os = "macos")]
fn prepare_sandbox(profile: &str, program: &str) -> io::Result<Command> {
    sandbox_command(profile, program)
}

#[cfg(target_os = "linux")]
fn prepare_sandbox(_: &str, _: &str) -> io::Result<Command> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "MCP processes require the macOS command policy",
    ))
}

/// sandbox-exec and this short exec stage both retain the guardian's clean env.
#[cfg(target_os = "macos")]
pub(crate) fn sandbox_command(profile: &str, program: &str) -> io::Result<Command> {
    let executable = std::env::current_exe()?;
    let literal = serde_json::to_string(&executable)
        .map_err(|_| io::Error::other("Invalid host executable"))?;
    let profile = format!("{profile}\n(allow file-read* (literal {literal}))");
    let mut command = crate::command::with_profile(&profile);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    command.arg(executable).args(["mcp-exec", program]);
    close_descriptors(true);
    Ok(command)
}

fn close_descriptors(keep_environment: bool) {
    for fd in 3..crate::command::inherited_descriptor_limit() {
        unsafe {
            libc::fcntl(
                fd,
                libc::F_SETFD,
                if keep_environment && fd == 6 {
                    0
                } else {
                    libc::FD_CLOEXEC
                },
            );
        }
    }
}

/// Runs inside Seatbelt already; replacing this process adds no persistent wrapper.
pub fn exec_contained(args: Vec<String>) -> io::Result<()> {
    let program = args
        .first()
        .ok_or_else(|| io::Error::other("MCP executable is missing"))?;
    close_descriptors(false);
    let env = environment()?;
    Err(Command::new(program)
        .args(&args[1..])
        .env_clear()
        .envs(env)
        .exec())
}

/// A private pipe is parsed only after the guardian's loader has started clean.
pub(crate) fn environment() -> io::Result<BTreeMap<String, String>> {
    if unsafe { libc::fcntl(6, libc::F_GETFD) } < 0 {
        return Err(io::Error::other("MCP environment pipe is missing"));
    }
    unsafe {
        libc::fcntl(6, libc::F_SETFD, libc::FD_CLOEXEC);
        let flags = libc::fcntl(6, libc::F_GETFL);
        libc::fcntl(6, libc::F_SETFL, flags & !libc::O_NONBLOCK);
    }
    let pipe = unsafe { File::from_raw_fd(6) };
    let mut bytes = Vec::new();
    pipe.take(1_048_577).read_to_end(&mut bytes)?;
    let invalid = || io::Error::other("Invalid MCP environment");
    if bytes.len() > 1_048_576 {
        return Err(invalid());
    }
    let env: BTreeMap<String, String> = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if env
        .iter()
        .any(|(key, value)| key.is_empty() || key.contains(['=', '\0']) || value.contains('\0'))
    {
        return Err(invalid());
    }
    Ok(env)
}
