mod cli;
mod components;
mod generate;
mod manifest;
mod naming;
mod templates;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    match cli::run(cli) {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(e.exit_code());
        }
    }
}
