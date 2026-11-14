//! GraphQL operation classification.
//!
//! Parses a GraphQL document and produces the repository permission
//! requirements the operation needs, so a later stage can feed them into the
//! existing GitHub policy.

use crate::policy::{Access, Scope};
use cynic_parser::common::OperationType;
use cynic_parser::executable::{FieldSelection, OperationDefinition, Selection};
use cynic_parser::{ExecutableDocument, Value};

/// Whether a repository reference identifies a repository or a subject node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Repository,
    Subject,
}

/// How a repository is identified in an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryTarget {
    Named { owner: String, name: String },
    NodeId { id: String, kind: NodeKind },
}

/// The scope a requirement demands, either a fixed scope or a subject node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeSpec {
    Fixed(Scope),
    Subject,
}

/// A single permission requirement: an access level, a repository target, and
/// the scope the access applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub access: Access,
    pub target: RepositoryTarget,
    pub scope: ScopeSpec,
}

/// The outcome of classifying an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    OutsideRepo { access: Access },
    Scoped(Vec<Requirement>),
}

/// An error produced while classifying a GraphQL document.
#[derive(Debug, thiserror::Error)]
pub enum GraphqlError {
    #[error("GraphQL document could not be parsed: {0}")]
    Parse(String),
    #[error("GraphQL operation is not supported")]
    Unsupported,
}

/// Classify a GraphQL document into the repository permission requirements it
/// demands.
///
/// `variables` resolves variable references used in repository arguments, and
/// `operation_name` selects among multiple operations in the document. When
/// `operation_name` is `None`, the document must contain exactly one
/// operation.
pub fn classify(
    query: &str,
    variables: Option<&serde_json::Value>,
    operation_name: Option<&str>,
) -> Result<Operation, GraphqlError> {
    let document = cynic_parser::parse_executable_document(query)
        .map_err(|error| GraphqlError::Parse(error.to_string()))?;
    let operation = select_operation(&document, operation_name)?;
    let access = match operation.operation_type() {
        OperationType::Query => Access::Read,
        OperationType::Mutation => Access::Write,
        OperationType::Subscription => return Err(GraphqlError::Unsupported),
    };

    let mut fields = Vec::new();
    flatten_fields(
        &document,
        &operation.selection_set().collect::<Vec<_>>(),
        &mut fields,
    )?;

    let mut requirements = Vec::new();
    for field in fields {
        match field.name() {
            "viewer" | "search" | "organization" => {}
            "repository" => {
                let target = repository_target(field, variables)?;
                let mut nested = Vec::new();
                flatten_fields(
                    &document,
                    &field.selection_set().collect::<Vec<_>>(),
                    &mut nested,
                )?;
                for selection in nested {
                    let scope = query_scope(selection.name()).ok_or(GraphqlError::Unsupported)?;
                    requirements.push(Requirement {
                        access,
                        target: target.clone(),
                        scope: ScopeSpec::Fixed(scope),
                    });
                }
            }
            _ => {
                let scope = mutation_scope(field.name()).ok_or(GraphqlError::Unsupported)?;
                let target = mutation_target(field, variables)?;
                requirements.push(Requirement {
                    access,
                    target,
                    scope,
                });
            }
        }
    }

    if requirements.is_empty() {
        Ok(Operation::OutsideRepo { access })
    } else {
        Ok(Operation::Scoped(requirements))
    }
}

fn select_operation<'a>(
    document: &'a ExecutableDocument,
    operation_name: Option<&str>,
) -> Result<OperationDefinition<'a>, GraphqlError> {
    let operations: Vec<_> = document.operations().collect();
    match operation_name {
        Some(name) => operations
            .into_iter()
            .find(|operation| operation.name() == Some(name))
            .ok_or(GraphqlError::Unsupported),
        None => match operations.as_slice() {
            [operation] => Ok(*operation),
            _ => Err(GraphqlError::Unsupported),
        },
    }
}

/// Resolve inline fragments and fragment spreads into a flat list of fields.
fn flatten_fields<'a>(
    document: &'a ExecutableDocument,
    selections: &[Selection<'a>],
    fields: &mut Vec<FieldSelection<'a>>,
) -> Result<(), GraphqlError> {
    for selection in selections {
        match selection {
            Selection::Field(field) => fields.push(*field),
            Selection::InlineFragment(fragment) => {
                let nested: Vec<_> = fragment.selection_set().collect();
                flatten_fields(document, &nested, fields)?;
            }
            Selection::FragmentSpread(spread) => {
                let name = spread.fragment_name();
                let definition = document
                    .fragments()
                    .find(|fragment| fragment.name() == name)
                    .ok_or(GraphqlError::Unsupported)?;
                let nested: Vec<_> = definition.selection_set().collect();
                flatten_fields(document, &nested, fields)?;
            }
        }
    }
    Ok(())
}

fn repository_target(
    field: FieldSelection<'_>,
    variables: Option<&serde_json::Value>,
) -> Result<RepositoryTarget, GraphqlError> {
    let owner = field
        .arguments()
        .find(|argument| argument.name() == "owner")
        .and_then(|argument| resolve_string(argument.value(), variables));
    let name = field
        .arguments()
        .find(|argument| argument.name() == "name")
        .and_then(|argument| resolve_string(argument.value(), variables));
    match (owner, name) {
        (Some(owner), Some(name)) => Ok(RepositoryTarget::Named { owner, name }),
        _ => Err(GraphqlError::Unsupported),
    }
}

fn mutation_target(
    field: FieldSelection<'_>,
    variables: Option<&serde_json::Value>,
) -> Result<RepositoryTarget, GraphqlError> {
    let input = field
        .arguments()
        .find(|argument| argument.name() == "input")
        .map(|argument| argument.value())
        .ok_or(GraphqlError::Unsupported)?;

    if let Some(value) = find_key(input, "repositoryId") {
        return Ok(RepositoryTarget::NodeId {
            id: resolve_string(value, variables).ok_or(GraphqlError::Unsupported)?,
            kind: NodeKind::Repository,
        });
    }
    if let Some(value) = find_key(input, "subjectId") {
        return Ok(RepositoryTarget::NodeId {
            id: resolve_string(value, variables).ok_or(GraphqlError::Unsupported)?,
            kind: NodeKind::Subject,
        });
    }
    if let Some(value) = find_key(input, "repositoryNameWithOwner") {
        let owner_and_name = resolve_string(value, variables).ok_or(GraphqlError::Unsupported)?;
        let (owner, name) = owner_and_name
            .split_once('/')
            .ok_or(GraphqlError::Unsupported)?;
        return Ok(RepositoryTarget::Named {
            owner: owner.to_string(),
            name: name.to_string(),
        });
    }
    Err(GraphqlError::Unsupported)
}

fn resolve_string(value: Value<'_>, variables: Option<&serde_json::Value>) -> Option<String> {
    if let Some(string) = value.as_str() {
        return Some(string.to_string());
    }
    if let Some(name) = value.as_variable() {
        return variables?
            .get(name)
            .and_then(|variable| variable.as_str())
            .map(|string| string.to_string());
    }
    None
}

fn find_key<'a>(value: Value<'a>, key: &str) -> Option<Value<'a>> {
    match value {
        Value::Object(object) => {
            for field in object.fields() {
                if field.name() == key {
                    return Some(field.value());
                }
            }
            for field in object.fields() {
                if let Some(found) = find_key(field.value(), key) {
                    return Some(found);
                }
            }
            None
        }
        Value::List(list) => list.items().find_map(|item| find_key(item, key)),
        _ => None,
    }
}

fn query_scope(name: &str) -> Option<Scope> {
    match name {
        "pullRequests" | "pullRequest" => Some(Scope::PullRequests),
        "issues" | "issue" | "issueComments" => Some(Scope::Issues),
        "object" | "defaultBranchRef" | "refs" | "content" => Some(Scope::Contents),
        "id" | "name" | "nameWithOwner" | "description" => Some(Scope::Metadata),
        _ => None,
    }
}

fn mutation_scope(name: &str) -> Option<ScopeSpec> {
    match name {
        "createPullRequest" | "mergePullRequest" | "updatePullRequest" | "requestReviews"
        | "submitPullRequestReview" => Some(ScopeSpec::Fixed(Scope::PullRequests)),
        "createIssue" | "updateIssue" | "updateIssueComment" | "deleteIssue" => {
            Some(ScopeSpec::Fixed(Scope::Issues))
        }
        "createCommitOnBranch" | "createRef" | "updateRef" | "createBlob" | "createTree" => {
            Some(ScopeSpec::Fixed(Scope::Contents))
        }
        "addComment" | "addReaction" | "addLabel" | "removeLabel" => Some(ScopeSpec::Subject),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Access, Scope};

    #[test]
    fn query_repository_id_is_metadata_read() {
        assert_eq!(
            classify("query { repository(owner: \"org\", name: \"repo\") { id } }", None, None)
                .unwrap(),
            Operation::Scoped(vec![Requirement {
                access: Access::Read,
                target: RepositoryTarget::Named {
                    owner: "org".into(),
                    name: "repo".into()
                },
                scope: ScopeSpec::Fixed(Scope::Metadata)
            }])
        );
    }

    #[test]
    fn mutation_create_issue_is_issues_write() {
        assert_eq!(
            classify(
                "mutation { createIssue(input: { repositoryId: \"abc\", title: \"t\" }) { issue { id } } }",
                None,
                None
            )
            .unwrap(),
            Operation::Scoped(vec![Requirement {
                access: Access::Write,
                target: RepositoryTarget::NodeId {
                    id: "abc".into(),
                    kind: NodeKind::Repository
                },
                scope: ScopeSpec::Fixed(Scope::Issues)
            }])
        );
    }

    #[test]
    fn mutation_add_comment_is_subject_scoped() {
        assert_eq!(
            classify(
                "mutation { addComment(input: { subjectId: \"s1\", body: \"hi\" }) { clientMutationId } }",
                None,
                None
            )
            .unwrap(),
            Operation::Scoped(vec![Requirement {
                access: Access::Write,
                target: RepositoryTarget::NodeId {
                    id: "s1".into(),
                    kind: NodeKind::Subject
                },
                scope: ScopeSpec::Subject
            }])
        );
    }

    #[test]
    fn repository_variables_resolve_from_input() {
        let variables = serde_json::json!({ "o": "org", "n": "repo" });
        assert_eq!(
            classify(
                "query($o: String!, $n: String!) { repository(owner: $o, name: $n) { id } }",
                Some(&variables),
                None
            )
            .unwrap(),
            Operation::Scoped(vec![Requirement {
                access: Access::Read,
                target: RepositoryTarget::Named {
                    owner: "org".into(),
                    name: "repo".into()
                },
                scope: ScopeSpec::Fixed(Scope::Metadata)
            }])
        );
    }

    #[test]
    fn repository_name_with_owner_splits_into_owner_and_name() {
        assert_eq!(
            classify(
                "mutation { createCommitOnBranch(input: { branch: { repositoryNameWithOwner: \"org/repo\", branchName: \"main\" }, message: { headline: \"x\" }, fileChanges: { additions: [] } }) { commit { oid } } }",
                None,
                None
            )
            .unwrap(),
            Operation::Scoped(vec![Requirement {
                access: Access::Write,
                target: RepositoryTarget::Named {
                    owner: "org".into(),
                    name: "repo".into()
                },
                scope: ScopeSpec::Fixed(Scope::Contents)
            }])
        );
    }

    #[test]
    fn viewer_is_outside_repository_read() {
        assert_eq!(
            classify("query { viewer { login } }", None, None).unwrap(),
            Operation::OutsideRepo {
                access: Access::Read
            }
        );
    }

    #[test]
    fn unmapped_field_fails_closed() {
        assert!(
            classify(
                "query { repository(owner: \"org\", name: \"repo\") { collaborators { nodes { id } } } }",
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn subscription_is_unsupported() {
        assert!(
            classify(
                "subscription { repository(owner: \"org\", name: \"repo\") { id } }",
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn unresolvable_variable_reference_fails_closed() {
        assert!(
            classify(
                "query($o: String!, $n: String!) { repository(owner: $o, name: $n) { id } }",
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_document_is_a_parse_error() {
        assert!(matches!(
            classify("query {", None, None),
            Err(GraphqlError::Parse(_))
        ));
    }
}
