//! `plait-host update`: installs the newest signed release's host on this
//! machine, for hosts that run without the desktop app.

use plait_release as update;
#[cfg(not(target_os = "linux"))]
use plait_release::install;
use std::io;

pub fn run(args: Vec<String>) -> io::Result<()> {
    let check_only = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => false,
        ["--check"] => true,
        _ => return Err(io::Error::other("usage: plait-host update [--check]")),
    };
    #[cfg(target_os = "linux")]
    {
        linux_update(check_only)
    }
    #[cfg(not(target_os = "linux"))]
    other_update(check_only)
}

#[cfg(not(target_os = "linux"))]
fn other_update(check_only: bool) -> io::Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(io::Error::other(
            "host updates currently install only on macOS",
        ));
    }
    let current = plait_protocol::update::current_version();
    let running = current
        .as_ref()
        .map_or("a source build".to_owned(), |version| {
            format!("Plait {version}")
        });
    let Some(offer) = plait_release::check(current.as_ref()).map_err(io::Error::other)? else {
        println!("No newer signed release is published; this host runs {running}.");
        return Ok(());
    };
    println!("Plait {} is available (running {running}).", offer.version);
    if check_only {
        println!("Release notes: {}", offer.notes_url());
        return Ok(());
    }
    let home = crate::service::user_home()?;
    let program = install::apply_host_update(&offer, &home).map_err(io::Error::other)?;
    println!("Host {} is active: {}", offer.version, program.display());
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_update(check_only: bool) -> io::Result<()> {
    use update::linux::{self, Decision};
    let home = crate::service::user_home()?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let build = linux::running_build(&home, &executable).map_err(io::Error::other)?;
    let release = plait_release::host_release(plait_protocol::update::platform())
        .map_err(io::Error::other)?;
    match linux::decide(
        &release,
        &build,
        plait_protocol::update::current_version().as_ref(),
    )
    .map_err(io::Error::other)?
    {
        Decision::Current => println!("Host {build} is current."),
        Decision::Offer(version) => {
            println!("Host {version} is available (running {build}).");
            if !check_only {
                let path = linux::apply(&home, &executable, &release).map_err(io::Error::other)?;
                println!("Host {version} is active: {}", path.display());
            }
        }
    }
    Ok(())
}
