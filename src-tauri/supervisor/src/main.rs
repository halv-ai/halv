#[cfg(unix)]
fn main() {
    if let Some(code) = run_agent_hook() {
        std::process::exit(code);
    }
    if let Err(error) = halv_supervisor::server::run_from_env() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    if let Some(code) = run_agent_hook() {
        std::process::exit(code);
    }
    eprintln!("halv-supervisor requires Unix");
    std::process::exit(1);
}

/// Runs the agent hook helper when the first argument selects it.
fn run_agent_hook() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next()? != halv_supervisor::agent_hook::AGENT_HOOK_COMMAND {
        return None;
    }
    Some(halv_supervisor::agent_hook::run_from_env(arguments))
}
