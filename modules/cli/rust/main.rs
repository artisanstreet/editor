use clap::Parser;

fn main() {
    if let Err(error) = artisan_editor_cli::run(artisan_editor_cli::Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(error.exit_code());
    }
}
