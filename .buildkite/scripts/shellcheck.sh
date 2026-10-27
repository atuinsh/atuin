#!/usr/bin/env bash
# The file discovery and invocation of ludeeus/action-shellcheck@2.0.0, as
# .github/workflows/shellcheck.yml ran it: every shell file by name, plus
# extensionless executables with an sh/bash/ksh shebang, each checked
# separately with SHELLCHECK_OPTS from the environment.
set -uo pipefail

excludes=(! -path '*./.git/*' ! -path '*.go' ! -path '*/mvnw')
files=()
while IFS= read -r -d '' file; do
  files+=("$file")
done < <(find . "${excludes[@]}" -type f '(' \
  -name '*.bash' -o -name '.bashrc' -o -name 'bashrc' -o -name '.bash_aliases' \
  -o -name '.bash_completion' -o -name '.bash_login' -o -name '.bash_logout' \
  -o -name '.bash_profile' -o -name 'bash_profile' -o -name '*.ksh' \
  -o -name 'suid_profile' -o -name '*.zsh' -o -name '.zlogin' -o -name 'zlogin' \
  -o -name '.zlogout' -o -name 'zlogout' -o -name '.zprofile' -o -name 'zprofile' \
  -o -name '.zsenv' -o -name 'zsenv' -o -name '.zshrc' -o -name 'zshrc' \
  -o -name '*.sh' -o -path '*/.profile' -o -path '*/profile' -o -name '*.shlib' \
  ')' -print0)
while IFS= read -r -d '' file; do
  head -n1 "$file" | grep -Eqs '^#! */[^ ]*/(env *)?[abk]*sh' || continue
  files+=("$file")
done < <(find . "${excludes[@]}" -type f ! -name '*.*' -perm /111 -print0)

echo "Checking ${#files[@]} files"
status=0
for file in "${files[@]}"; do
  shellcheck --format=gcc "$file" || status=$?
done
exit "$status"
