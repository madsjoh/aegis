use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use aegis_core::ca::Authority;
use aegis_core::secrets::{decrypt, AgeSource};
use aegis_core::token_table::{Selection, TokenTable};

use aegis_gateway::proxy::{self, BoxError};

#[derive(Parser)]
#[command(about = "HTTP gateway for the Aegis sandbox")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authorize Copilot on the host and save the OAuth token with SOPS.
    LoginCopilot {
        #[arg(long)]
        secrets: PathBuf,
        #[arg(long)]
        age_key: PathBuf,
        #[arg(long)]
        force: bool,
    },
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
    /// Write the placeholder auth.json for the configured API key providers.
    ProvisionAuth {
        /// Path to the sops encrypted secrets file.
        #[arg(long)]
        secrets: PathBuf,
        /// Path to the age key file.
        #[arg(long)]
        age_key: PathBuf,
        /// Path to write the auth.json file.
        #[arg(long)]
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt().init();

    match Cli::parse().command {
        Command::LoginCopilot {
            secrets,
            age_key,
            force,
        } => {
            let age = AgeSource::File(age_key);
            let file = decrypt(&secrets, &age).map_err(BoxError::from_error)?;
            let table = TokenTable::build(&file, &Selection::defaults(&file))
                .map_err(BoxError::from_error)?;
            if let Some(configuration) = table.copilot() {
                if force
                    || configuration
                        .oauth_token
                        .as_deref()
                        .is_none_or(str::is_empty)
                {
                    let token = aegis_gateway::copilot::device_login()
                        .await
                        .map_err(BoxError::from_error)?;
                    aegis_core::secrets::update_copilot_oauth(
                        &secrets,
                        &age,
                        &configuration.credential,
                        &token,
                    )
                    .map_err(BoxError::from_error)?;
                    println!(
                        "Saved encrypted Copilot authorization for {}.",
                        configuration.credential
                    );
                }
            }
            Ok(())
        }
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
        Command::ProvisionAuth {
            secrets,
            age_key,
            out,
        } => {
            let file =
                decrypt(&secrets, &AgeSource::File(age_key)).map_err(BoxError::from_error)?;
            let selection = Selection::defaults(&file);
            let credentials = aegis_gateway::credentials::provision_credentials(&file, &selection)
                .map_err(BoxError::from_error)?;
            let body = serde_json::to_string_pretty(&credentials).map_err(BoxError::from_error)?;
            std::fs::write(&out, body)?;
            Ok(())
        }
    }
}
