#!/usr/bin/env bash

set -euo pipefail

cd /home/svein/dev/ganbot/
(jj status 2>&1 | grep -q 'has no changes') || echo "Uncommitted changes exist but are being ignored."
sleep 2

cd ~/nixos/
nix flake update ganbot --allow-dirty-locks
colmena apply-local --sudo
