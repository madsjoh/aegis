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
