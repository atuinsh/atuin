#!/bin/bash
export VAULT_TOKEN="some-vault-token"
export VAULT_ADDR="https://some.vault.com"

bao policy write atuin - <<EOF
path "kv/data/atuin" {
  capabilities = ["read", "list"]
}
EOF

bao write auth/k8-rwx-dev/role/atuin \
    bound_service_account_names=atuin \
    bound_service_account_namespaces=atuin \
    policies=atuin \
    ttl=24h

bao kv put kv/atuin @secret.json
