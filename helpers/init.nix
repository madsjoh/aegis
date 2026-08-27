{ pkgs }:

pkgs.writeShellApplication {
  name = "aegis-init";
  runtimeInputs = with pkgs; [ coreutils jq ];
  text = ''
    ${builtins.readFile ./init.bash}

    CONFIG_DIR="''${XDG_CONFIG_HOME:-$HOME/.config}/aegis"
    CONFIG_FILE="$CONFIG_DIR/config.json"
    mkdir -p "$CONFIG_DIR"

    EXISTING="{}"
    if [ -f "$CONFIG_FILE" ]; then
      EXISTING="$(cat "$CONFIG_FILE")"
      if ! is_valid_json "$EXISTING"; then
        echo "Error: $CONFIG_FILE is not valid JSON." >&2
        exit 1
      fi
    fi

    ADDITIONS="{}"

    if command -v opencode >/dev/null 2>&1; then
      read -r -p "Include OpenCode auth? [y/N] " ANSWER
      case "$ANSWER" in
        y|Y)
          AUTH_FILE="''${XDG_DATA_HOME:-$HOME/.local/share}/opencode/auth.json"
          if [ -f "$AUTH_FILE" ]; then
            ADDITIONS="$(printf '%s' "$ADDITIONS" | jq --argjson auth "$(cat "$AUTH_FILE")" '.opencode.auth = $auth')"
            echo "Included OpenCode auth from $AUTH_FILE."
          else
            echo "Warning: no OpenCode auth file found at $AUTH_FILE." >&2
          fi
          ;;
        *)
          echo "Skipped OpenCode auth."
          ;;
      esac
    else
      echo "OpenCode is not installed; skipping."
    fi

    if command -v gh >/dev/null 2>&1; then
      read -r -p "Include GitHub token? [y/N] " ANSWER
      case "$ANSWER" in
        y|Y)
          TOKEN="$(gh auth token 2>/dev/null || true)"
          if [ -n "$TOKEN" ]; then
            ADDITIONS="$(printf '%s' "$ADDITIONS" | jq --arg token "$TOKEN" '.github.token = $token')"
            echo "Included GitHub token from gh."
          else
            echo "Warning: gh is not authenticated; run 'gh auth login' first." >&2
          fi
          ;;
        *)
          echo "Skipped GitHub token."
          ;;
      esac
    else
      echo "gh is not installed; skipping."
    fi

    MERGED="$(merge_object "$EXISTING" "$ADDITIONS")"
    printf '%s\n' "$MERGED" > "$CONFIG_FILE"
    chmod 600 "$CONFIG_FILE"
    echo "Wrote $CONFIG_FILE"
  '';
}
