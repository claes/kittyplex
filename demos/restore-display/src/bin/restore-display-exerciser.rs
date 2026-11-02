fn main() -> std::process::ExitCode {
    match restore_display::exerciser::run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("restore-display-exerciser: {e}");
            std::process::ExitCode::from(2)
        }
    }
}
