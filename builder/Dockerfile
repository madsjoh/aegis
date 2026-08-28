FROM --platform=linux/arm64 alpine:3.22

RUN apk add --no-cache bash nix openssh shadow \
  && addgroup -S builder \
  && adduser -S -D -h /home/builder -s /bin/bash -G builder builder \
  && passwd -d builder \
  && addgroup -S nixbld \
  && for number in $(seq 1 10); do \
       adduser -S -D -H -h /var/empty -s /sbin/nologin -G nixbld "nixbld${number}"; \
     done \
  && install -d -m 0755 /etc/nix /nix /run/sshd \
  && printf '%s\n' \
       'builders-use-substitutes = true' \
       'sandbox = false' \
       'substituters = https://cache.nixos.org https://microvm.cachix.org' \
       'trusted-public-keys = cache.nixos.org-1:6NCHdD59X431o0gWypbAN9svvLtwWQLBvgXWoamSHB0= microvm.cachix.org-1:oXnBc6hRE3eX5rSYdRyMYXnfzcCxC7yKPTbZXALsqys=' \
       'trusted-users = root builder' \
       > /etc/nix/nix.conf \
  && printf '%s\n' \
       'PasswordAuthentication no' \
       'KbdInteractiveAuthentication no' \
       'PermitRootLogin no' \
       'PubkeyAuthentication yes' \
       'AllowUsers builder' \
       > /etc/ssh/sshd_config.d/aegis.conf

COPY entrypoint.bash /usr/local/bin/aegis-builder-entrypoint

RUN chmod 0755 /usr/local/bin/aegis-builder-entrypoint

VOLUME ["/nix"]
EXPOSE 22

ENTRYPOINT ["/usr/local/bin/aegis-builder-entrypoint"]
