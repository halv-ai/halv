// The release build is a GUI program on Windows, so it gets no console
// window. The two subcommands below talk over piped stdio that the parent
// process supplies. They keep working without a console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::var_os("HALV_APPLY_PATCH_HELPER").is_some() {
        std::process::exit(halv_lib::exec::run_apply_patch_helper_from_stdin());
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--mcp-savings")) {
        std::process::exit(halv_lib::savings_mcp::run_from_args(
            std::env::args_os().skip(2),
        ));
    }
    halv_lib::run();
}
