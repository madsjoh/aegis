{ pkgs, ... }:

{
  # Combine the system CA bundle with the gateway CA so guest tools trust the
  # certificates the gateway proxy mints for intercepted hosts.
  systemd.services.aegis-trust-ca = {
    description = "Combine the system and gateway CA bundles for guest tools";
    wantedBy = [ "multi-user.target" ];
    after = [ "local-fs.target" ];
    serviceConfig.Type = "oneshot";
    serviceConfig.RemainAfterExit = true;
    script = ''
      mkdir -p /run/aegis
      cat ${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt > /run/aegis/ca-bundle.crt
      if [ -f /aegis/ca.crt ]; then
        cat /aegis/ca.crt >> /run/aegis/ca-bundle.crt
      fi
    '';
  };
}
