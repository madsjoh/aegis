forward_secrets() {
  local gh_token="${1:-}"

  if [ -z "$gh_token" ]; then
    gh_token="${GH_TOKEN:-}"
  fi

  export GH_TOKEN="$gh_token"
  export GITHUB_TOKEN="$gh_token"
}
