# Create the configuration files with their defaults when they are missing.
# Existing files are left untouched.
scaffold_config() {
  local configuration_directory="${1:?path to the configuration directory}"
  mkdir --parents "$configuration_directory"

  if [ ! -e "$configuration_directory/vm.json" ]; then
    printf '%s\n' '{"cpu":4,"mem":4096}' > "$configuration_directory/vm.json"
  fi

  if [ ! -e "$configuration_directory/skills.json" ]; then
    printf '%s\n' '{"anthropic":false,"mattpocock":false,"vercel":false}' > "$configuration_directory/skills.json"
  fi

  if [ ! -e "$configuration_directory/opencode.jsonc" ]; then
    printf '%s\n' '{}' > "$configuration_directory/opencode.jsonc"
  fi
}
