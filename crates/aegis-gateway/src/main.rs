mod rewrite;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "HTTP gateway for the Aegis sandbox")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ensure the certificate authority is present.
    EnsureCa,
    /// Serve the gateway.
    Serve,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::EnsureCa | Command::Serve => {
            println!("unimplemented");
            std::process::exit(1);
        }
    }
}
