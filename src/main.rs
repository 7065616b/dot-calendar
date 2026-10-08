#![cfg_attr(not(test), windows_subsystem = "windows")]

mod calendar_info;
mod cli;
mod desktop;
mod dot_connection;
mod google;
mod mcp;
mod model;
mod preferences;
mod store;
mod widget;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--mcp") {
        if let Err(error) = mcp::run() {
            eprintln!("{error}");
            std::process::exit(1);
        }
    } else if args.as_slice() == ["--connect-dot"]
        || args.as_slice() == ["--disconnect-dot"]
        || args.as_slice() == ["--shutdown"]
    {
        let result = match args[0].as_str() {
            "--connect-dot" => dot_connection::install(),
            "--disconnect-dot" => dot_connection::remove(),
            _ => widget::shutdown(),
        };
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
    } else if !args.is_empty() && args.as_slice() != ["--dot-onboarding"] {
        // A GUI executable does not create a console. Reuse its caller's console
        // for interactive CLI use; redirected stdio (MCP/tests) remains usable.
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
        }
        std::process::exit(cli::run(&args));
    } else if let Err(error) = widget::run(args.as_slice() == ["--dot-onboarding"]) {
        eprintln!("{error}");
        widget::show_error(&error);
        std::process::exit(1);
    }
}
