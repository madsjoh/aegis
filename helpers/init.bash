# Return 0 when the value parses as JSON, non-zero otherwise.
is_valid_json() {
  printf '%s' "$1" | jq -e . >/dev/null 2>&1
}

# Merge additions over an existing JSON object, with additions winning.
merge_object() {
  printf '%s\n%s\n' "$1" "$2" | jq -s '.[0] * .[1]'
}
