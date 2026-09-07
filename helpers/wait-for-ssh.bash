wait_for_ssh() {
  local failed_probes=0

  until ssh "${SSH_OPTS[@]}" "${PROBE_TARGET[@]}" true 2>/dev/null; do
    if ! kill -0 "$VM_PID" 2>/dev/null; then
      echo "Error: The Aegis VM exited before SSH became available." >&2
      cat "$VM_LOG" >&2
      return 1
    fi

    failed_probes=$((failed_probes + 1))
    if [ $((failed_probes % 150)) -eq 0 ]; then
      echo "Still waiting for the guest SSH server..."
    fi
    sleep 0.2
  done
}
