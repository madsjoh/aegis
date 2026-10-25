# Resolve the first non-empty value from a list, mirroring a precedence order
# where earlier arguments outrank later ones.
resolve() {
  local value=""
  for value in "$@"; do
    if [ -n "$value" ]; then
      printf '%s' "$value"
      return 0
    fi
  done
  return 1
}

# Merge a list of JSON files into a single document, with later files
# overriding earlier ones. Missing files are skipped. Requires jq on PATH.
merge_json() {
  local files=()
  local file=""
  for file in "$@"; do
    if [ -n "$file" ] && [ -f "$file" ]; then
      files+=("$file")
    fi
  done
  if [ "${#files[@]}" -eq 0 ]; then
    printf '{}\n'
    return 0
  fi
  jq -s 'reduce .[] as $x ({}; . * $x)' "${files[@]}"
}

prepare_opencode() {
  local configuration_file="${1:?path to the Aegis configuration}"
  local configuration_content
  configuration_content="$(jq --compact-output '(.opencode // {}) | del(.auth)' "$configuration_file")" || return 1
  export OPENCODE_CONFIG_CONTENT="$configuration_content"

  if jq --exit-status '.opencode.auth' "$configuration_file" >/dev/null 2>&1; then
    mkdir --parents "$HOME/.local/share/opencode"
    jq '.opencode.auth' "$configuration_file" > "$HOME/.local/share/opencode/auth.json"
    chmod 600 "$HOME/.local/share/opencode/auth.json"
  fi
}
