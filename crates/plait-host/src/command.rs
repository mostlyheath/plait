use std::{
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

static CLEANUP_RECEIPT: AtomicBool = AtomicBool::new(false);

/// Only the runtime opts a helper into this private inherited channel.
pub(crate) fn initialize_cleanup_receipt() -> io::Result<()> {
    let Some(fd) = std::env::var_os("PLAIT_CLEANUP_RECEIPT_FD") else {
        return Ok(());
    };
    if fd != "8" || unsafe { libc::fcntl(8, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::other("Invalid cleanup receipt descriptor."));
    }
    CLEANUP_RECEIPT.store(true, Ordering::Relaxed);
    Ok(())
}

pub fn run(args: Vec<String>) -> io::Result<()> {
    if args.len() != 3 {
        return Err(io::Error::other("command expects checkout policy script"));
    }
    let command = sandbox_command(&args).inspect_err(|_| cleanup_receipt("clean\n"))?;
    let keep_env = std::env::var_os("PLAIT_COMMAND_ENV_FD").is_some();
    // Keep store, writer, and lifecycle leases in the guardian, never expose them to commands.
    for fd in 3..inherited_descriptor_limit() {
        if keep_env && fd == 6 {
            continue;
        }
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    guard(command, "sandboxed", io::stdin(), Stdio::null())
}

/// Runs a command as the user, without Seatbelt, for a project whose owner
/// chose the trusted command policy. Lifetime rules match the sandboxed path.
pub fn run_trusted(args: Vec<String>) -> io::Result<()> {
    if args.len() != 2 {
        return Err(io::Error::other("trusted-command expects checkout script"));
    }
    let mut command = trusted_command(&args).inspect_err(|_| cleanup_receipt("clean\n"))?;
    private_environment(&mut command).inspect_err(|_| cleanup_receipt("clean\n"))?;
    // The command has full user access, so no inherited descriptor may reach it:
    // not the leases, and not a host-private channel a parent failed to close.
    for fd in 3..inherited_descriptor_limit() {
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    guard(command, "trusted", io::stdin(), Stdio::null())
}

pub(crate) fn inherited_descriptor_limit() -> i32 {
    let limit = unsafe { libc::getdtablesize() };
    limit.clamp(5, 65_536)
}

pub(crate) fn guard(
    mut command: Command,
    label: &str,
    mut lifetime: impl Read + Send + 'static,
    stdin: Stdio,
) -> io::Result<()> {
    command.env_remove("PLAIT_CLEANUP_RECEIPT_FD");
    let mut child = match command.stdin(stdin).process_group(0).spawn() {
        Ok(child) => child,
        Err(error) => {
            cleanup_receipt("clean\n");
            return Err(error);
        }
    };
    let group = child.id() as i32;
    cleanup_receipt(&format!("group:{group}\n"));
    let (send, receive) = mpsc::channel();
    let exit = send.clone();
    thread::spawn(move || {
        let _ = exit.send(CommandExit::Exited(child.wait()));
    });
    // A pipe is a lifetime lease: even SIGKILL of Node closes it and stops tools.
    thread::spawn(move || {
        loop {
            match lifetime.read(&mut [0u8; 1]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(10));
                }
                _ => {
                    unsafe {
                        libc::kill(-group, libc::SIGKILL);
                    }
                    let _ = send.send(CommandExit::Cancelled);
                    break;
                }
            }
        }
    });
    let status = wait_command(receive, Duration::from_secs(2))?;
    // Shells may exit while background children still hold the output pipes.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    if wait_group_gone(group, std::time::Duration::from_secs(2)) {
        cleanup_receipt("clean\n");
    }
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{label} command failed or stopped: {status}"
        )))
    }
}

enum CommandExit {
    Exited(io::Result<ExitStatus>),
    Cancelled,
}

fn wait_command(receive: mpsc::Receiver<CommandExit>, grace: Duration) -> io::Result<ExitStatus> {
    let failure = || io::Error::other("Command process exit could not be confirmed.");
    let event = receive.recv().map_err(|_| failure())?;
    let event = if matches!(event, CommandExit::Cancelled) {
        // An unkillable child must not keep its guardian and UI waiting forever.
        receive.recv_timeout(grace).map_err(|_| failure())?
    } else {
        event
    };
    match event {
        CommandExit::Exited(status) => status,
        CommandExit::Cancelled => Err(failure()),
    }
}

/// ESRCH alone proves the normal group is gone; a failed signal is not proof.
pub(crate) fn wait_group_gone(group: i32, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if unsafe { libc::kill(-group, 0) } != 0
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Optional private receipt channel. Never put a cleanup claim in tool output.
pub(crate) fn cleanup_receipt(text: &str) {
    if !CLEANUP_RECEIPT.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        libc::write(8, text.as_ptr().cast(), text.len());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn trusted_command(args: &[String]) -> io::Result<Command> {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", &args[1]]).current_dir(&args[0]);
    Ok(command)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn trusted_command(_: &[String]) -> io::Result<Command> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "command containment is not enabled on this platform",
    ))
}

#[cfg(target_os = "macos")]
fn sandbox_command(args: &[String]) -> io::Result<Command> {
    let mut command = if std::env::var_os("PLAIT_COMMAND_ENV_FD").is_some() {
        let mut command = crate::mcp::sandbox_command(&args[1], "/bin/sh")?;
        command.args(["-c", &args[2]]);
        command
    } else {
        let mut command = with_profile(&args[1]);
        command.args(["/bin/sh", "-c", &args[2]]);
        command
    };
    command.current_dir(&args[0]);
    Ok(command)
}

/// Both command execution and recording enter Seatbelt through the host.
#[cfg(target_os = "macos")]
pub(crate) fn with_profile(profile: &str) -> Command {
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command.args(["-p", profile]);
    command
}

#[cfg(not(target_os = "macos"))]
fn sandbox_command(_: &[String]) -> io::Result<Command> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "command containment is not enabled on this platform",
    ))
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn trusted_command_runs_on_linux() {
        let output = trusted_command(&["/tmp".into(), "printf trusted".into()])
            .unwrap()
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"trusted");
    }

    #[test]
    fn unsupported_platform_denies_before_starting_a_command() {
        let error = run(vec!["/nonexistent".into(), String::new(), "exit 0".into()]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_platform_denies_trusted_commands_too() {
        let error = run_trusted(vec!["/nonexistent".into(), "exit 0".into()]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }
}

fn private_environment(command: &mut Command) -> io::Result<()> {
    if std::env::var_os("PLAIT_COMMAND_ENV_FD").is_some() {
        command.env_clear().envs(crate::mcp::environment()?);
    }
    Ok(())
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;
    #[test]
    fn a_stop_without_child_exit_is_bounded_and_never_claims_cleanup() {
        let (send, receive) = mpsc::channel();
        send.send(CommandExit::Cancelled).unwrap();
        assert!(wait_command(receive, Duration::from_millis(1)).is_err());
    }
    #[test]
    fn a_live_group_is_not_proof_and_a_reaped_group_is() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        let group = child.id() as i32;
        assert!(!wait_group_gone(group, std::time::Duration::ZERO));
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
        child.wait().unwrap();
        assert!(wait_group_gone(group, std::time::Duration::from_secs(3)));
    }
}
