#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

BUILDER_BASH="${1:-${BUILDER_BASH:-helpers/builder.bash}}"
unset BASH_ENV

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

docker() {
  case "$*" in
    "info --format {{.Architecture}}")
      if [ "$TEST_SCENARIO" = "wrong-architecture" ]; then
        printf '%s\n' x86_64
      else
        printf '%s\n' aarch64
      fi
      ;;
    "container inspect aegis-builder")
      [ -f "$TEST_STATE/container" ]
      ;;
    "inspect --format {{.State.Running}} aegis-builder")
      if [ -f "$TEST_STATE/running" ]; then
        printf '%s\n' true
      else
        printf '%s\n' false
      fi
      ;;
    "inspect --format {{.Config.Image}} aegis-builder")
      if [ "$TEST_SCENARIO" = "wrong-image" ]; then
        printf '%s\n' other-image
      else
        printf '%s\n' aegis-builder
      fi
      ;;
    "inspect --format {{.Image}} aegis-builder")
      printf '%s\n' sha256:builder-image
      ;;
    "image inspect --format {{.Os}}/{{.Architecture}} sha256:builder-image")
      if [ "$TEST_SCENARIO" = "wrong-platform" ]; then
        printf '%s\n' linux/amd64
      else
        printf '%s\n' linux/arm64
      fi
      ;;
    "image inspect --format {{index .Config.Labels \"org.aegis.builder.version\"}} aegis-builder" | \
    "image inspect --format {{index .Config.Labels \"org.aegis.builder.version\"}} sha256:builder-image")
      if [ "$TEST_SCENARIO" = "image-version-drift" ] \
        && { [ -f "$TEST_STATE/container" ] || [ ! -f "$TEST_STATE/image-built" ]; }; then
        printf '%s\n' 0
      else
        printf '%s\n' 1
      fi
      ;;
    "image inspect --format {{index .Config.Labels \"org.aegis.builder.context\"}} aegis-builder" | \
    "image inspect --format {{index .Config.Labels \"org.aegis.builder.context\"}} sha256:builder-image")
      if [ "$TEST_SCENARIO" = "image-context-drift" ] && [ ! -f "$TEST_STATE/image-built" ]; then
        printf '%s\n' OLD-CONTEXT
      else
        printf '%s\n' TEST-CONTEXT
      fi
      ;;
    "inspect --format {{index .Config.Labels \"org.aegis.builder.version\"}} aegis-builder")
      printf '%s\n' 1
      ;;
    "inspect --format {{index .Config.Labels \"org.aegis.builder.context\"}} aegis-builder")
      printf '%s\n' TEST-CONTEXT
      ;;
    "inspect --format {{index .Config.Labels \"org.aegis.builder.public-key-fingerprint\"}} aegis-builder")
      if [ "$TEST_SCENARIO" = "credential-drift" ]; then
        printf '%s\n' SHA256:OLD
      else
        printf '%s\n' SHA256:CLIENT
      fi
      ;;
    "image inspect aegis-builder")
      touch "$TEST_STATE/image-inspected"
      if [ "$TEST_SCENARIO" = "image-missing" ] && [ ! -f "$TEST_STATE/image-built" ]; then
        return 1
      fi
      ;;
    "build --label org.aegis.builder.context=TEST-CONTEXT --label org.aegis.builder.version=1 --platform linux/arm64 --tag aegis-builder builder")
      touch "$TEST_STATE/image-built"
      ;;
    "build --label org.aegis.builder.context=TEST-CONTEXT --label org.aegis.builder.version=1 --platform linux/arm64 --tag aegis-builder /nix/store/aegis-builder")
      touch "$TEST_STATE/image-built-with-custom-context"
      ;;
    "inspect --format {{(index (index .HostConfig.PortBindings \"22/tcp\") 0).HostIp}}:{{(index (index .HostConfig.PortBindings \"22/tcp\") 0).HostPort}} aegis-builder")
      if [ "$TEST_SCENARIO" = "wrong-port-binding" ]; then
        printf '%s\n' 0.0.0.0:31022
      else
        printf '%s\n' 127.0.0.1:31022
      fi
      ;;
    "inspect --format {{range .Mounts}}{{if eq .Destination \"/nix\"}}{{.Type}}:{{.Name}}{{end}}{{end}} aegis-builder")
      if [ "$TEST_SCENARIO" = "wrong-volume" ]; then
        printf '%s\n' bind:
      else
        printf '%s\n' volume:aegis-builder-nix
      fi
      ;;
    "run --detach --env AEGIS_BUILDER_PUBLIC_KEY --label org.aegis.builder.context=TEST-CONTEXT --label org.aegis.builder.public-key-fingerprint=SHA256:CLIENT --label org.aegis.builder.version=1 --name aegis-builder --platform linux/arm64 --publish 127.0.0.1:31022:22 --volume aegis-builder-nix:/nix aegis-builder")
      if [ "${AEGIS_BUILDER_PUBLIC_KEY:-}" != 'ssh-ed25519 AEGIS-TEST-KEY' ]; then
        return 1
      fi
      if ! export -p | grep -Fq 'AEGIS_BUILDER_PUBLIC_KEY'; then
        return 1
      fi
      if [ -f "$TEST_STATE/container" ]; then
        return 1
      fi
      touch "$TEST_STATE/container" "$TEST_STATE/running"
      printf '%s\n' builder-id
      ;;
    "start aegis-builder")
      if [ -f "$TEST_STATE/running" ]; then
        return 1
      fi
      touch "$TEST_STATE/running"
      printf '%s\n' aegis-builder
      ;;
    *)
      printf 'unexpected docker arguments:' >&2
      printf ' %q' "$@" >&2
      printf '\n' >&2
      return 1
      ;;
  esac
}

ssh-keygen() {
  if [ "${1:-}" != -l ] || [ "${2:-}" != -E ] || [ "${3:-}" != sha256 ] || [ "${4:-}" != -f ]; then
    return 1
  fi
  printf '%s\n' '256 SHA256:CLIENT aegis (ED25519)'
}

nc() {
  if [ "$*" != "-z 127.0.0.1 31022" ]; then
    return 0
  fi
  [ "$TEST_SCENARIO" = "occupied-port" ]
}

ssh-keyscan() {
  if [ "$*" != "-p 31022 127.0.0.1" ]; then
    return 2
  fi
  local attempts_file="$TEST_STATE/ssh-attempts"
  local attempts=0
  if [ -f "$attempts_file" ]; then
    attempts="$(<"$attempts_file")"
  fi
  attempts=$((attempts + 1))
  printf '%s\n' "$attempts" > "$attempts_file"

  if [ "$TEST_SCENARIO" = "ssh-terminal-failure" ]; then
    return 1
  fi
  if [ "$TEST_SCENARIO" = "ssh-retry" ] && [ "$attempts" -lt 3 ]; then
    return 1
  fi
  if [ "$TEST_SCENARIO" = "host-key-mismatch" ]; then
    printf '%s\n' '[127.0.0.1]:31022 ssh-ed25519 WRONG'
  else
    printf '%s\n' '[127.0.0.1]:31022 ssh-ed25519 EXPECTED'
  fi
}

sleep() {
  [ "$*" = "1" ]
}

export -f docker nc sleep ssh-keygen ssh-keyscan

run_builder() {
  local scenario="${1:?scenario}"
  local initial_state="${2:-missing}"

  TEST_STATE="$TMP/$scenario"
  mkdir -p "$TEST_STATE"
  if [ "$scenario" != "missing-public-key" ]; then
    write_public_key "$TEST_STATE"
  fi
  if [ "$scenario" = "multiple-public-keys" ]; then
    printf '%s\n' 'ssh-ed25519 SECOND-KEY' >> "$TEST_STATE/id_ed25519.pub"
  fi
  if [ "$initial_state" != "missing" ]; then
    touch "$TEST_STATE/container"
  fi
  if [ "$initial_state" = "running" ]; then
    touch "$TEST_STATE/running"
  fi

  TEST_SCENARIO="$scenario" TEST_STATE="$TEST_STATE" \
    AEGIS_BUILDER_HOST_KEY='ssh-ed25519 EXPECTED' \
    AEGIS_BUILDER_CONTEXT_IDENTITY='TEST-CONTEXT' \
    AEGIS_BUILDER_VERSION='1' \
    AEGIS_BUILDER_PUBLIC_KEY_FILE="$TEST_STATE/id_ed25519.pub" \
    bash -c 'if [ "$TEST_SCENARIO" = missing-docker ]; then unset -f docker; PATH="$TEST_STATE"; fi; source "$1"; ensure_builder' bash "$BUILDER_BASH"
}

run_build_image() {
  local context="${1:-builder}"

  TEST_STATE="$TMP/build-image"
  mkdir -p "$TEST_STATE"
  TEST_SCENARIO=build-image TEST_STATE="$TEST_STATE" AEGIS_BUILDER_CONTEXT="$context" \
    AEGIS_BUILDER_CONTEXT_IDENTITY='TEST-CONTEXT' AEGIS_BUILDER_VERSION='1' \
    bash -c 'source "$1"; builder_build_image' bash "$BUILDER_BASH"
}

write_public_key() {
  local state="${1:?state}"
  printf '%s\n' 'ssh-ed25519 AEGIS-TEST-KEY' > "$state/id_ed25519.pub"
}

if run_builder missing-docker; then
  fail "missing Docker fails"
else
  pass "missing Docker fails"
fi

if run_builder first-creation && [ -f "$TEST_STATE/running" ]; then
  pass "first use creates and starts the builder"
else
  fail "first use creates and starts the builder"
fi

if run_builder image-missing \
  && [ -f "$TEST_STATE/image-inspected" ] \
  && [ -f "$TEST_STATE/image-built" ] \
  && [ -f "$TEST_STATE/running" ]; then
  pass "a missing image is built before first container creation"
else
  fail "a missing image is built before first container creation"
fi

if run_builder image-existing \
  && [ -f "$TEST_STATE/image-inspected" ] \
  && [ ! -f "$TEST_STATE/image-built" ] \
  && [ -f "$TEST_STATE/running" ]; then
  pass "an existing image is not rebuilt before first container creation"
else
  fail "an existing image is not rebuilt before first container creation"
fi

if run_builder image-context-drift \
  && [ -f "$TEST_STATE/image-built" ] \
  && [ -f "$TEST_STATE/running" ]; then
  pass "a stale unused image is rebuilt before container creation"
else
  fail "a stale unused image is rebuilt before container creation"
fi

if run_builder image-version-drift running 2> "$TEST_STATE/error"; then
  fail "a running container with stale image identity is rejected"
elif [ ! -f "$TEST_STATE/image-built" ] \
  && grep --fixed-strings --quiet 'image identity' "$TEST_STATE/error"; then
  pass "a running container with stale image identity is rejected"
else
  fail "a running container with stale image identity is rejected"
fi

if run_builder image-version-drift \
  && [ -f "$TEST_STATE/image-built" ] \
  && [ -f "$TEST_STATE/running" ]; then
  pass "an unused image with a stale builder version is rebuilt"
else
  fail "an unused image with a stale builder version is rebuilt"
fi

if run_builder missing-public-key; then
  fail "container creation requires a configured public key file"
else
  pass "container creation requires a configured public key file"
fi

if run_builder multiple-public-keys; then
  fail "container creation rejects a public key file with multiple lines"
else
  pass "container creation rejects a public key file with multiple lines"
fi

if run_build_image && [ -f "$TEST_STATE/image-built" ]; then
  pass "the ARM64 builder image is built with the deterministic tag"
else
  fail "the ARM64 builder image is built with the deterministic tag"
fi

if run_build_image /nix/store/aegis-builder && [ -f "$TEST_STATE/image-built-with-custom-context" ]; then
  pass "the builder image context is configurable"
else
  fail "the builder image context is configurable"
fi

if run_builder stopped-startup stopped && [ -f "$TEST_STATE/running" ]; then
  pass "a stopped builder starts"
else
  fail "a stopped builder starts"
fi

if run_builder healthy-reuse running && [ -f "$TEST_STATE/running" ]; then
  pass "a healthy builder is reused"
else
  fail "a healthy builder is reused"
fi

if run_builder wrong-image running; then
  fail "a builder with the wrong image is rejected"
else
  pass "a builder with the wrong image is rejected"
fi

if run_builder wrong-platform running; then
  fail "a builder with the wrong platform is rejected"
else
  pass "a builder with the wrong platform is rejected"
fi

if run_builder wrong-port-binding running; then
  fail "a builder without a loopback SSH binding is rejected"
else
  pass "a builder without a loopback SSH binding is rejected"
fi

if run_builder wrong-volume running; then
  fail "a builder without the expected Nix volume is rejected"
else
  pass "a builder without the expected Nix volume is rejected"
fi

if run_builder credential-drift running 2> "$TEST_STATE/error"; then
  fail "a container with stale credentials is rejected"
elif grep --fixed-strings --quiet 'docker container rm --force aegis-builder' "$TEST_STATE/error"; then
  pass "credential drift reports the exact safe recovery command"
else
  fail "credential drift reports the exact safe recovery command"
fi

if run_builder wrong-architecture; then
  fail "a Docker engine with the wrong architecture fails"
else
  pass "a Docker engine with the wrong architecture fails"
fi

if run_builder occupied-port; then
  fail "an occupied SSH port prevents creation"
else
  pass "an occupied SSH port prevents creation"
fi

if run_builder host-key-mismatch running; then
  fail "a host key mismatch is fatal"
else
  pass "a host key mismatch is fatal"
fi

if run_builder ssh-retry running && [ "$(<"$TEST_STATE/ssh-attempts")" = "3" ]; then
  pass "SSH readiness retries until the builder responds"
else
  fail "SSH readiness retries until the builder responds"
fi

if run_builder ssh-terminal-failure running; then
  fail "SSH readiness fails after thirty attempts"
elif [ "$(<"$TEST_STATE/ssh-attempts")" = "30" ]; then
  pass "SSH readiness fails after thirty attempts"
else
  fail "SSH readiness fails after thirty attempts"
fi

if [ "$failures" -eq 0 ]; then
  echo "all builder tests passed"
else
  echo "$failures builder test(s) failed" >&2
  exit 1
fi
