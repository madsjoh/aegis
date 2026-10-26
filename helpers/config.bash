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

# Convert the Aegis authentication object into OpenCode 2 credential entries.
# OpenCode 1 stored provider credentials in an auth.json file; OpenCode 2 keeps
# them in a SQLite credential store populated by the auth import command.
opencode_auth_entries() {
  local configuration_file="${1:?path to the Aegis configuration}"
  jq '
    (.opencode.auth // {})
    | to_entries
    | map(
        .key as $integration_id
        | .value as $value
        | {
            id: ("cred_aegis_" + ($integration_id | gsub("[^a-zA-Z0-9_-]"; "-"))),
            integrationID: $integration_id,
            label: (if $value.type == "api" then "API key" else "OAuth" end),
            active: true,
            value:
              if $value.type == "api" then
                { type: "key", key: $value.key } + ($value | del(.type, .key))
              else
                $value
              end
          }
      )
  ' "$configuration_file"
}

# Import the configured provider credentials into the OpenCode credential
# store. The opencode binary only exists in the guest, so the caller checks for
# it first. Existing credentials are removed before import so that re-imports
# activate and key rotations take effect.
import_opencode_auth() {
  local configuration_file="${1:?path to the Aegis configuration}"
  local credentials_file
  credentials_file="$(mktemp)"
  local status=0
  opencode_auth_entries "$configuration_file" > "$credentials_file" || status=1
  if [ "$status" -eq 0 ]; then
    jq --raw-output '.[] | "\(.integrationID) \(.id)"' "$credentials_file" \
      | while read -r integration_id credential_id; do
          opencode auth logout --standalone "$integration_id" "$credential_id" </dev/null >/dev/null 2>&1 || true
        done
    opencode auth import --standalone "$credentials_file" >/dev/null 2>&1 || status=1
  fi
  rm -f "$credentials_file"
  return "$status"
}

prepare_opencode() {
  local configuration_file="${1:?path to the Aegis configuration}"
  local configuration_content
  configuration_content="$(jq --compact-output '(.opencode // {}) | del(.auth)' "$configuration_file")" || return 1
  export OPENCODE_CONFIG_CONTENT="$configuration_content"

  if jq --exit-status '.opencode.auth' "$configuration_file" >/dev/null 2>&1 \
    && command -v opencode >/dev/null 2>&1; then
    if ! import_opencode_auth "$configuration_file"; then
      echo "Warning: failed to import OpenCode credentials." >&2
    fi
  fi
}
