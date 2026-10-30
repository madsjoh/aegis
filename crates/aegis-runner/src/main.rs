//! The Aegis runner: orchestrate the guest VM and gateway on the host.

mod config;
mod lock;
mod paths;
mod ssh_key;
mod store_cache;

use std::process::ExitCode;

const USAGE: &str = "\
Usage: aegis [run [args...] | help]
  run   Boot the guest VM (default).
  help  Show this help.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        None | Some("run") => {
            // Orchestration is wired in Task 7.
            let _forwarded = &args[2..];
            ExitCode::SUCCESS
        }
        Some("help") | Some("--help") | Some("-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("Error: unknown command '{other}'.");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}
