mod proxy;
mod rewrite;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use aegis_core::ca::Authority;
use aegis_core::secrets::{decrypt, AgeSource};
use aegis_core::token_table::{Selection, TokenTable};

use proxy::BoxError;

#[derive(Parser)]
#[command(about = "HTTP gateway for the Aegis sandbox")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ensure the certificate authority is present and write its certificate.
    EnsureCa {
        /// Directory that stores the CA key pair.
        #[arg(long)]
        ca_dir: PathBuf,
        /// Path to write the CA certificate in PEM form.
        #[arg(long)]
        cert_out: PathBuf,
    },
    /// Serve the forward proxy.
    Serve {
        /// Address to listen on.
        #[arg(long)]
        listen: SocketAddr,
        /// Directory that stores the CA key pair.
        #[arg(long)]
        ca_dir: PathBuf,
        /// Path to the sops encrypted secrets file.
        #[arg(long)]
        secrets: PathBuf,
        /// Path to the age key file.
        #[arg(long)]
        age_key: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt().init();

    match Cli::parse().command {
        Command::EnsureCa { ca_dir, cert_out } => {
            let authority = Authority::load_or_create(&ca_dir).map_err(BoxError::from_error)?;
            std::fs::write(&cert_out, authority.ca_pem())?;
            Ok(())
        }
        Command::Serve {
            listen,
            ca_dir,
            secrets,
            age_key,
        } => {
            let authority =
                Arc::new(Authority::load_or_create(&ca_dir).map_err(BoxError::from_error)?);
            let file =
                decrypt(&secrets, &AgeSource::File(age_key)).map_err(BoxError::from_error)?;
            let selection = Selection::defaults(&file);
            let table =
                Arc::new(TokenTable::build(&file, &selection).map_err(BoxError::from_error)?);
            proxy::serve(listen, table, authority).await
        }
    }
}
