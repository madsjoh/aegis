acquire_lock() {
  local lock_file="${1:?lock file}"
  local descriptor_variable="${2:?descriptor variable}"
  local acquired_descriptor

  exec {acquired_descriptor}>"$lock_file"
  if ! flock --exclusive --nonblock "$acquired_descriptor"; then
    exec {acquired_descriptor}>&-
    return 1
  fi
  printf -v "$descriptor_variable" '%s' "$acquired_descriptor"
}

release_lock() {
  local descriptor="${1:?lock descriptor}"

  flock --unlock "$descriptor"
  exec {descriptor}>&-
}
