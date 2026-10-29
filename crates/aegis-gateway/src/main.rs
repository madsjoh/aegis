use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use aegis_core::ca::Authority;
use aegis_core::catalog::catalog;
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
            let mut credentials: Vec<serde_json::Value> = Vec::new();
            for (name, spec) in catalog() {
                if name == "github" || name == "copilot" {
                    continue;
                }
                let Some(credential_name) = selection.credentials.get(&name) else {
                    continue;
                };
                let Some(credentials_map) = file.integrations.get(&name) else {
                    continue;
                };
                let Some(credential) = credentials_map.get(credential_name) else {
                    continue;
                };
                if credential.api_key.is_some() {
                    credentials.push(serde_json::json!({
                        "id": format!("cred_aegis_{}", name),
                        "integrationID": name,
                        "label": "API key",
                        "active": true,
                        "value": { "type": "key", "key": spec.placeholder },
                    }));
                }
            }
            let body = serde_json::to_string_pretty(&credentials).map_err(BoxError::from_error)?;
            std::fs::write(&out, body)?;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o600))?;
            Ok(())
        }
    }
}
