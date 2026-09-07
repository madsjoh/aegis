prepare_vm_log() {
  local vm_log="${1:?VM log path}"

  : > "$vm_log"
}

close_lock_descriptors() {
  local descriptor

  for descriptor in "$@"; do
    exec {descriptor}>&-
  done
}

cleanup() {
  local descriptor
  local pid
  local virtiofsd_pids

  if [ -n "$VM_PID" ]; then
    pid="$VM_PID"
    VM_PID=""
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  virtiofsd_pids="$VIRTIOFSD_PIDS"
  VIRTIOFSD_PIDS=""
  for pid in $virtiofsd_pids; do
    kill "$pid" 2>/dev/null || true
  done
  if [ -n "$IMAGE_LOCK_DESCRIPTOR" ]; then
    descriptor="$IMAGE_LOCK_DESCRIPTOR"
    IMAGE_LOCK_DESCRIPTOR=""
    release_lock "$descriptor"
  fi
  if [ -n "$WORKSPACE_LOCK_DESCRIPTOR" ]; then
    descriptor="$WORKSPACE_LOCK_DESCRIPTOR"
    WORKSPACE_LOCK_DESCRIPTOR=""
    release_lock "$descriptor"
  fi
}

install_cleanup_traps() {
  trap cleanup EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
}
