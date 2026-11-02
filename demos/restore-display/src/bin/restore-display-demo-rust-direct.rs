fn main() -> std::process::ExitCode {
    match restore_display::demo::run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("restore-display-demo: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
