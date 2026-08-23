acquire_lock() {
  LOCK_DIR="${1:?lock directory}"

  local acquired=false
  local stale=""
  for _ in 1 2; do
    if mkdir "$LOCK_DIR" 2>/dev/null; then
      acquired=true
      break
    fi
    if [ -f "$LOCK_DIR/pid" ] && ! kill -0 "$(cat "$LOCK_DIR/pid")" 2>/dev/null; then
      stale="$LOCK_DIR.stale.$$"
      if mv "$LOCK_DIR" "$stale" 2>/dev/null; then
        rm -rf "$stale"
      fi
    else
      break
    fi
  done

  if [ "$acquired" != true ]; then
    return 1
  fi

  echo "$$" > "$LOCK_DIR/pid"
  return 0
}
