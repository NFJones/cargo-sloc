//! Entry point for the `cargo loc` external Cargo subcommand.

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();

    if arguments.iter().any(|argument| argument == "--version") {
        println!("cargo-loc {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    println!(
        "cargo-loc: configuration-aware Rust source line counts\n\
         \n\
         Usage: cargo loc [OPTIONS]\n\
         \n\
         This command is scaffolded; see SPEC.md for the proposed behavior.\n\
         \n\
         Options:\n\
           -h, --help     Print help\n\
               --version  Print version"
    );
}
