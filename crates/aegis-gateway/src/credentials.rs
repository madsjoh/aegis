//! Placeholder credentials imported into OpenCode.

use aegis_core::catalog::catalog;
use aegis_core::secrets::SecretsFile;
use aegis_core::token_table::{BuildError, Selection, TokenTable};

pub fn provision_credentials(
    file: &SecretsFile,
    selection: &Selection,
) -> Result<Vec<serde_json::Value>, BuildError> {
    let table = TokenTable::build(file, selection)?;
    let mut credentials = Vec::new();
    for (name, spec) in catalog() {
        if name == "github" {
            continue;
        }
        let Some(credential) = selection
            .credentials
            .get(&name)
            .and_then(|selected| file.integrations.get(&name)?.get(selected))
        else {
            continue;
        };
        if credential.api_key.is_some() {
            credentials.push(serde_json::json!({
                "id":format!("cred_aegis_{name}"), "integrationID":name, "label":"API key", "active":true,
                "value":{"type":"key","key":spec.placeholder}
            }));
        }
    }
    if table.copilot().is_some() {
        credentials.push(serde_json::json!({
            "id":"cred_aegis_copilot", "integrationID":"github-copilot", "label":"GitHub Copilot", "active":true,
            "value":{"type":"oauth","methodID":"device","access":crate::copilot::PLACEHOLDER,
                "refresh":crate::copilot::PLACEHOLDER,"expires":0,
                "metadata":{"apiEndpoint":"https://api.githubcopilot.com"}}
        }));
    }
    Ok(credentials)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_oauth_is_provisioned_only_for_selected_enabled_github() {
        let file: SecretsFile = serde_json::from_value(serde_json::json!({
            "github": {"alpha":{"pat":"hidden-alpha"},"work":{"pat":"hidden-work","copilot":true}},
            "openai": {"default":{"api_key":"hidden-openai"}}
        }))
        .unwrap();
        let mut selection = Selection::defaults(&file);
        let credentials = provision_credentials(&file, &selection).unwrap();
        assert_eq!(credentials.len(), 1);
        selection
            .credentials
            .insert("github".to_string(), "work".to_string());
        let credentials = provision_credentials(&file, &selection).unwrap();
        let copilot = credentials
            .iter()
            .find(|value| value["integrationID"] == "github-copilot")
            .unwrap();
        assert_eq!(
            copilot["value"],
            serde_json::json!({
                "type":"oauth", "methodID":"device", "access":"copilot-aegis-placeholder",
                "refresh":"copilot-aegis-placeholder", "expires":0,
                "metadata":{"apiEndpoint":"https://api.githubcopilot.com"}
            })
        );
        assert!(copilot["active"].as_bool().unwrap());
        assert!(!serde_json::to_string(&credentials)
            .unwrap()
            .contains("hidden-"));
        assert_eq!(credentials.len(), 2);
    }
}
