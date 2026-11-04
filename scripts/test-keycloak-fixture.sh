#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="limiara-keycloak-fixture-${RANDOM:-0}-$$"
export KEYCLOAK_PORT=0

cleanup() {
  status=$?
  trap - EXIT
  if (( status != 0 )); then
    docker compose --project-name "$project" ps --all || true
    docker compose --project-name "$project" logs --no-color keycloak || true
  fi
  docker compose --project-name "$project" down --volumes --remove-orphans >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

compose=(docker compose --project-name "$project")

docker compose config --quiet
"${compose[@]}" up --detach --wait keycloak

python3 -c '
import json
with open("infra/keycloak/realm-local.json", encoding="utf-8") as source:
    realm = json.load(source)
client = next(item for item in realm["clients"] if item["clientId"] == "platform-web")
assert client["publicClient"] is True
assert client["standardFlowEnabled"] is True
assert client["directAccessGrantsEnabled"] is False
assert client["attributes"]["pkce.code.challenge.method"] == "S256"
assert "http://localhost:8080/auth/oidc/callback" in client["redirectUris"]
'

published_port=$("${compose[@]}" port keycloak 8081 | awk -F: '{print $NF}')
discovery_url="http://127.0.0.1:${published_port}/realms/local/.well-known/openid-configuration"
curl --fail --silent --show-error "$discovery_url" |
  python3 -c 'import json, sys; document = json.load(sys.stdin); assert document["authorization_endpoint"]; assert document["token_endpoint"]'

# The realm's built-in admin-cli client is used only to prove both fixture
# credentials were imported. The application client keeps password grants off.
for username in member operator nonmember; do
  curl --fail --silent --show-error \
    --data client_id=admin-cli \
    --data grant_type=password \
    --data username="$username" \
    --data password=local-password \
    "http://127.0.0.1:${published_port}/realms/local/protocol/openid-connect/token" |
    python3 -c 'import json, sys; assert json.load(sys.stdin)["access_token"]'
done

authorization_status=$(curl --silent --output /dev/null --write-out '%{http_code}' \
  "http://127.0.0.1:${published_port}/realms/local/protocol/openid-connect/auth?client_id=platform-web&redirect_uri=http%3A%2F%2Flocalhost%3A8080%2Fauth%2Foidc%2Fcallback&response_type=code&scope=openid&code_challenge=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU&code_challenge_method=S256")
[[ "$authorization_status" == "200" ]]
