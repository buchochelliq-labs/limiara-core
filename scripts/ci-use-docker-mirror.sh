#!/usr/bin/env bash
# CI only: route Docker Hub pulls through Google's public pull-through cache,
# so unauthenticated runner pulls stop hitting Docker Hub's rate limit. The
# daemon falls back to Docker Hub itself when the mirror cannot serve an image.
set -Eeuo pipefail

mirror=${DOCKER_HUB_MIRROR:-https://mirror.gcr.io}
config=/etc/docker/daemon.json

# Merge into the runner's existing daemon settings rather than replacing them.
current='{}'
if sudo test -s "$config"; then
  current=$(sudo cat "$config")
fi
jq --arg mirror "$mirror" '."registry-mirrors" = [$mirror]' <<<"$current" | sudo tee "$config" >/dev/null
sudo systemctl restart docker
docker info --format '{{json .RegistryConfig.Mirrors}}' | grep -qF "$mirror"
