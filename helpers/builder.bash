builder_error() {
  printf 'aegis builder: %s\n' "$1" >&2
  return 1
}

builder_load_configuration() {
  AEGIS_BUILDER_CONTAINER="${AEGIS_BUILDER_CONTAINER:-aegis-builder}"
  AEGIS_BUILDER_HOST="${AEGIS_BUILDER_HOST:-127.0.0.1}"
  AEGIS_BUILDER_IMAGE="${AEGIS_BUILDER_IMAGE:-aegis-builder}"
  AEGIS_BUILDER_CONTEXT="${AEGIS_BUILDER_CONTEXT:-builder}"
  AEGIS_BUILDER_PORT="${AEGIS_BUILDER_PORT:-31022}"

  if [ -z "${AEGIS_BUILDER_VERSION:-}" ] || [ -z "${AEGIS_BUILDER_CONTEXT_IDENTITY:-}" ]; then
    builder_error "Builder image identity is not configured."
    return
  fi

  if [ "$AEGIS_BUILDER_HOST" != "127.0.0.1" ]; then
    builder_error "SSH must bind to 127.0.0.1."
    return
  fi
}

builder_build_image() {
  AEGIS_BUILDER_IMAGE="${AEGIS_BUILDER_IMAGE:-aegis-builder}"
  AEGIS_BUILDER_CONTEXT="${AEGIS_BUILDER_CONTEXT:-builder}"
  docker build \
    --label "org.aegis.builder.context=$AEGIS_BUILDER_CONTEXT_IDENTITY" \
    --label "org.aegis.builder.version=$AEGIS_BUILDER_VERSION" \
    --platform linux/arm64 \
    --tag "$AEGIS_BUILDER_IMAGE" \
    "$AEGIS_BUILDER_CONTEXT"
}

builder_public_key_fingerprint() {
  local fingerprint=""
  local temporary_public_key=""

  builder_load_public_key || return
  if [ -n "${AEGIS_BUILDER_PUBLIC_KEY_FILE:-}" ]; then
    fingerprint="$(ssh-keygen -l -E sha256 -f "$AEGIS_BUILDER_PUBLIC_KEY_FILE" 2>/dev/null | cut -d ' ' -f 2)" || return
  else
    temporary_public_key="$(mktemp)" || return
    printf '%s\n' "$AEGIS_BUILDER_PUBLIC_KEY" > "$temporary_public_key"
    fingerprint="$(ssh-keygen -l -E sha256 -f "$temporary_public_key" 2>/dev/null | cut -d ' ' -f 2)" || {
      rm --force "$temporary_public_key"
      return 1
    }
    rm --force "$temporary_public_key"
  fi
  if [ -z "$fingerprint" ]; then
    builder_error "Builder public key fingerprint could not be determined."
    return
  fi
  printf '%s\n' "$fingerprint"
}

builder_active_include_count() {
  local configuration_file="$1"

  if [ ! -f "$configuration_file" ]; then
    printf '%s\n' 0
    return
  fi
  grep --extended-regexp --count \
    '^[[:space:]]*!include[[:space:]]+/etc/nix/aegis\.conf([[:space:]]*(#.*)?)?$' \
    "$configuration_file" || true
}

builder_load_public_key() {
  local public_key_content=""
  local public_key_line=""

  if [ -n "${AEGIS_BUILDER_PUBLIC_KEY:-}" ]; then
    public_key_line="$AEGIS_BUILDER_PUBLIC_KEY"
  else
    if [ -z "${AEGIS_BUILDER_PUBLIC_KEY_FILE:-}" ]; then
      builder_error "AEGIS_BUILDER_PUBLIC_KEY or AEGIS_BUILDER_PUBLIC_KEY_FILE is not configured."
      return
    fi
    if [ ! -f "$AEGIS_BUILDER_PUBLIC_KEY_FILE" ]; then
      builder_error "Builder public key file does not exist: $AEGIS_BUILDER_PUBLIC_KEY_FILE."
      return
    fi

    public_key_content="$(<"$AEGIS_BUILDER_PUBLIC_KEY_FILE")"
    public_key_line="$public_key_content"
  fi

  if [ -z "$public_key_line" ]; then
    builder_error "Builder public key is empty."
    return
  fi
  case "$public_key_line" in
    *$'\n'* | *$'\r'*)
      builder_error "Builder public key must contain one public key."
      return
      ;;
  esac
  AEGIS_BUILDER_PUBLIC_KEY="$public_key_line"
}

builder_require_docker() {
  local architecture=""

  if ! command -v docker >/dev/null 2>&1; then
    builder_error "Docker is not installed."
    return
  fi
  if ! architecture="$(docker info --format '{{.Architecture}}' 2>/dev/null)"; then
    builder_error "Docker is not available. Start Docker and try again."
    return
  fi
  case "$architecture" in
    aarch64 | arm64)
      ;;
    *)
      builder_error "Docker must use the ARM64 architecture, but reported $architecture."
      ;;
  esac
}

builder_container_exists() {
  docker container inspect "$AEGIS_BUILDER_CONTAINER" >/dev/null 2>&1
}

builder_image_exists() {
  docker image inspect "$AEGIS_BUILDER_IMAGE" >/dev/null 2>&1
}

builder_validate_image() {
  local image_identifier="${1:-$AEGIS_BUILDER_IMAGE}"
  local actual=""

  actual="$(docker image inspect --format '{{index .Config.Labels "org.aegis.builder.version"}}' "$image_identifier" 2>/dev/null)" || return
  if [ "$actual" != "$AEGIS_BUILDER_VERSION" ]; then
    builder_error "Builder image identity drift: expected version $AEGIS_BUILDER_VERSION, found ${actual:-missing}."
    return
  fi
  actual="$(docker image inspect --format '{{index .Config.Labels "org.aegis.builder.context"}}' "$image_identifier" 2>/dev/null)" || return
  if [ "$actual" != "$AEGIS_BUILDER_CONTEXT_IDENTITY" ]; then
    builder_error "Builder image identity drift: expected context $AEGIS_BUILDER_CONTEXT_IDENTITY, found ${actual:-missing}."
    return
  fi
}

builder_container_running() {
  [ "$(docker inspect --format '{{.State.Running}}' "$AEGIS_BUILDER_CONTAINER" 2>/dev/null)" = true ]
}

builder_validate_container() {
  local actual=""
  local expected_fingerprint=""
  local image_identifier=""

  actual="$(docker inspect --format '{{.Config.Image}}' "$AEGIS_BUILDER_CONTAINER" 2>/dev/null)" || return
  if [ "$actual" != "$AEGIS_BUILDER_IMAGE" ]; then
    builder_error "Existing container uses image $actual instead of $AEGIS_BUILDER_IMAGE."
    return
  fi

  image_identifier="$(docker inspect --format '{{.Image}}' "$AEGIS_BUILDER_CONTAINER" 2>/dev/null)" || return
  builder_validate_image "$image_identifier" || return

  actual="$(docker inspect --format '{{index .Config.Labels "org.aegis.builder.version"}}' "$AEGIS_BUILDER_CONTAINER" 2>/dev/null)" || return
  if [ "$actual" != "$AEGIS_BUILDER_VERSION" ]; then
    builder_error "Existing container image identity has drifted. Remove it only after stopping dependent builds."
    return
  fi
  actual="$(docker inspect --format '{{index .Config.Labels "org.aegis.builder.context"}}' "$AEGIS_BUILDER_CONTAINER" 2>/dev/null)" || return
  if [ "$actual" != "$AEGIS_BUILDER_CONTEXT_IDENTITY" ]; then
    builder_error "Existing container image identity has drifted. Remove it only after stopping dependent builds."
    return
  fi
  expected_fingerprint="$(builder_public_key_fingerprint)" || return
  actual="$(docker inspect --format '{{index .Config.Labels "org.aegis.builder.public-key-fingerprint"}}' "$AEGIS_BUILDER_CONTAINER" 2>/dev/null)" || return
  if [ "$actual" != "$expected_fingerprint" ]; then
    builder_error "Builder credential drift detected. After confirming no build uses it, run: docker container rm --force $AEGIS_BUILDER_CONTAINER"
    return
  fi

  actual="$(docker image inspect --format '{{.Os}}/{{.Architecture}}' "$image_identifier" 2>/dev/null)" || return
  if [ "$actual" != "linux/arm64" ]; then
    builder_error "Existing container uses platform $actual instead of linux/arm64."
    return
  fi

  actual="$(docker inspect --format '{{(index (index .HostConfig.PortBindings "22/tcp") 0).HostIp}}:{{(index (index .HostConfig.PortBindings "22/tcp") 0).HostPort}}' "$AEGIS_BUILDER_CONTAINER" 2>/dev/null)" || return
  if [ "$actual" != "$AEGIS_BUILDER_HOST:$AEGIS_BUILDER_PORT" ]; then
    builder_error "Existing container does not bind SSH to $AEGIS_BUILDER_HOST:$AEGIS_BUILDER_PORT."
    return
  fi

  actual="$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/nix"}}{{.Type}}:{{.Name}}{{end}}{{end}}' "$AEGIS_BUILDER_CONTAINER" 2>/dev/null)" || return
  if [ "$actual" != "volume:aegis-builder-nix" ]; then
    builder_error "Existing container does not use the aegis-builder-nix volume for /nix."
  fi
}

builder_create() {
  local public_key_fingerprint=""

  builder_load_public_key || return
  public_key_fingerprint="$(builder_public_key_fingerprint)" || return

  if nc -z "$AEGIS_BUILDER_HOST" "$AEGIS_BUILDER_PORT" >/dev/null 2>&1; then
    builder_error "SSH port $AEGIS_BUILDER_PORT is already in use."
    return
  fi

  export AEGIS_BUILDER_PUBLIC_KEY
  docker run --detach \
    --env AEGIS_BUILDER_PUBLIC_KEY \
    --label "org.aegis.builder.context=$AEGIS_BUILDER_CONTEXT_IDENTITY" \
    --label "org.aegis.builder.public-key-fingerprint=$public_key_fingerprint" \
    --label "org.aegis.builder.version=$AEGIS_BUILDER_VERSION" \
    --name "$AEGIS_BUILDER_CONTAINER" \
    --platform linux/arm64 \
    --publish "$AEGIS_BUILDER_HOST:$AEGIS_BUILDER_PORT:22" \
    --volume aegis-builder-nix:/nix \
    "$AEGIS_BUILDER_IMAGE" >/dev/null || {
      unset AEGIS_BUILDER_PUBLIC_KEY
      return 1
    }
  unset AEGIS_BUILDER_PUBLIC_KEY
}

builder_verify_host_key() {
  local scanned_key=""
  local attempts=0

  while [ "$attempts" -lt 30 ]; do
    scanned_key="$(ssh-keyscan -p "$AEGIS_BUILDER_PORT" "$AEGIS_BUILDER_HOST" 2>/dev/null || true)"
    if [ -n "$scanned_key" ]; then
      break
    fi
    attempts=$((attempts + 1))
    sleep 1
  done

  if [ -z "$scanned_key" ]; then
    builder_error "SSH did not become ready on port $AEGIS_BUILDER_PORT."
    return
  fi
  if ! printf '%s\n' "$scanned_key" | cut -d ' ' -f 2- | grep -Fqx "$AEGIS_BUILDER_HOST_KEY"; then
    builder_error "SSH host key mismatch."
  fi
}

builder_start() {
  builder_load_configuration || return
  builder_require_docker || return

  if ! builder_container_exists; then
    if ! builder_image_exists; then
      builder_build_image || return
    elif ! builder_validate_image; then
      builder_build_image || return
    fi
    builder_validate_image || return
    builder_create || return
  else
    builder_validate_container || return
    if ! builder_container_running; then
      docker start "$AEGIS_BUILDER_CONTAINER" >/dev/null || return
    fi
  fi

}

ensure_builder() {
  if [ -z "${AEGIS_BUILDER_HOST_KEY:-}" ]; then
    builder_error "AEGIS_BUILDER_HOST_KEY is not configured."
    return
  fi
  builder_start || return
  builder_verify_host_key
}
