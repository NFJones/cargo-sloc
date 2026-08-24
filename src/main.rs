//! Process entry point for the `cargo loc` external Cargo subcommand.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let output = cargo_loc::run(std::env::args_os().skip(1));

    if let Err(error) = std::io::stdout().write_all(&output.stdout) {
        let _ = writeln!(
            std::io::stderr(),
            "cargo-loc: failed to write stdout: {error}"
        );
        return ExitCode::from(1);
    }
    if std::io::stderr().write_all(&output.stderr).is_err() {
        return ExitCode::from(1);
    }

    ExitCode::from(output.exit_code)
}
