{ lib, ... }:

{
  system.stateVersion = lib.trivial.release;

  environment.sessionVariables = {
    VM_GIT_NAME = builtins.getEnv "VM_GIT_NAME";
    VM_GIT_EMAIL = builtins.getEnv "VM_GIT_EMAIL";
  };
}
