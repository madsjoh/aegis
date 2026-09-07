wait_for_image_lock() {
  local lock_file="${1:?lock file}"
  local descriptor_variable="${2:?descriptor variable}"

  until acquire_lock "$lock_file" "$descriptor_variable"; do
    sleep 0.1
  done
}

wait_for_guest_start() {
  local vm_pid="${1:?VM pid}"
  local vm_log="${2:?VM log}"

  while true; do
    if ! kill -0 "$vm_pid" 2>/dev/null; then
      return 1
    fi
    if grep -Fxq '[vzvm] guest started' "$vm_log" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
}

link_shared_store_images() {
  local workspace_state_directory="${1:?workspace state directory}"
  local shared_cache_directory="${2:?shared cache directory}"
  local shared_image

  for shared_image in "$shared_cache_directory"/store-*.img; do
    if [ -f "$shared_image" ]; then
      ln -sfn "$shared_image" "$workspace_state_directory/$(basename "$shared_image")"
    fi
  done
}

publish_store_images() {
  local workspace_state_directory="${1:?workspace state directory}"
  local shared_cache_directory="${2:?shared cache directory}"
  local workspace_image
  local image_name

  for workspace_image in "$workspace_state_directory"/store-*.img; do
    if [ -f "$workspace_image" ] && [ ! -L "$workspace_image" ]; then
      image_name="$(basename "$workspace_image")"
      mv -f -- "$workspace_image" "$shared_cache_directory/$image_name"
      ln -s "$shared_cache_directory/$image_name" "$workspace_image"
    fi
  done
}
