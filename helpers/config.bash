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

# Forward the OpenCode configuration to the opencode process through the
# OPENCODE_CONFIG_CONTENT environment variable. OpenCode accepts JSONC and
# validates the content itself, so the file is forwarded verbatim.
prepare_opencode() {
  local configuration_file="${1:?path to the OpenCode configuration file}"
  local configuration_content
  configuration_content="$(cat "$configuration_file")" || return 1
  export OPENCODE_CONFIG_CONTENT="$configuration_content"
}
