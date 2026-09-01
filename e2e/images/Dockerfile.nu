# nushell client image for atuin E2E tests. Build context: e2e/images
#   docker build -t atuin-e2e-nu:local -f Dockerfile.nu .
#
# nushell isn't packaged in Debian, so we fetch the release binary matching the
# image arch. NU_VERSION is overridable via --build-arg.
FROM atuin-e2e-base:local
USER root
ARG NU_VERSION=0.101.0
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && arch="$(dpkg --print-architecture)" \
 && case "$arch" in \
      amd64) target=x86_64-unknown-linux-gnu ;; \
      arm64) target=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported arch: $arch" >&2; exit 1 ;; \
    esac \
 && curl -fsSL "https://github.com/nushell/nushell/releases/download/${NU_VERSION}/nu-${NU_VERSION}-${target}.tar.gz" -o /tmp/nu.tgz \
 && tar -xzf /tmp/nu.tgz -C /tmp \
 && install "/tmp/nu-${NU_VERSION}-${target}/nu" /usr/local/bin/nu \
 && rm -rf /tmp/nu*
USER atuin
RUN mkdir -p /home/atuin/.config/nushell \
 && atuin init nu > /home/atuin/.config/nushell/atuin_init.nu \
 && printf '%s\n' \
      'source ~/.config/nushell/atuin_init.nu' \
      '$env.PROMPT_COMMAND = {|| "ATUIN_E2E> " }' \
      '$env.PROMPT_COMMAND_RIGHT = {|| "" }' \
      '$env.PROMPT_INDICATOR = ""' \
      '$env.PROMPT_INDICATOR_VI_INSERT = ""' \
      '$env.PROMPT_INDICATOR_VI_NORMAL = ""' \
      > /home/atuin/.config/nushell/config.nu \
 && : > /home/atuin/.config/nushell/env.nu
CMD ["sleep", "infinity"]
