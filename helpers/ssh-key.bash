ensure_ssh_key() {
  local ssh_key="${1:?SSH key path}"
  local lock_file="$ssh_key.lock"
  local lock_descriptor
  local private_public_key
  local published_public_key
  local temporary_directory

  until acquire_lock "$lock_file" lock_descriptor; do
    sleep 0.1
  done

  if private_public_key="$(ssh-keygen -y -f "$ssh_key" 2>/dev/null)"; then
    published_public_key="$(cut -d ' ' -f 1,2 "$ssh_key.pub" 2>/dev/null || true)"
    if [ "$(printf '%s\n' "$private_public_key" | cut -d ' ' -f 1,2)" = "$published_public_key" ]; then
      release_lock "$lock_descriptor"
      return 0
    fi

    temporary_directory="$(mktemp -d "$ssh_key.tmp.XXXXXX")"
    chmod 700 "$temporary_directory"
    printf '%s\n' "$private_public_key" > "$temporary_directory/key.pub"
    mv "$temporary_directory/key.pub" "$ssh_key.pub"
    rmdir "$temporary_directory"
    release_lock "$lock_descriptor"
    return 0
  fi

  if [ -e "$ssh_key" ] || [ -L "$ssh_key" ]; then
    release_lock "$lock_descriptor"
    return 1
  fi

  temporary_directory="$(mktemp -d "$ssh_key.tmp.XXXXXX")"
  chmod 700 "$temporary_directory"
  if ! ssh-keygen -t ed25519 -f "$temporary_directory/key" -N "" -q; then
    rm -rf "$temporary_directory"
    release_lock "$lock_descriptor"
    return 1
  fi
  rm "$temporary_directory/key.pub"
  mv "$temporary_directory/key" "$ssh_key"
  if ! ssh-keygen -y -f "$ssh_key" > "$temporary_directory/key.pub"; then
    rm -rf "$temporary_directory"
    release_lock "$lock_descriptor"
    return 1
  fi
  mv "$temporary_directory/key.pub" "$ssh_key.pub"
  rmdir "$temporary_directory"
  release_lock "$lock_descriptor"
}
