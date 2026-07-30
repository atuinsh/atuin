# zsh client image for atuin E2E tests. Build context: e2e/images
#   docker build -t atuin-e2e-zsh:local -f Dockerfile.zsh .
FROM atuin-e2e-base:local
USER root
RUN apt-get update \
 && apt-get install -y --no-install-recommends zsh \
 && rm -rf /var/lib/apt/lists/*
USER atuin
# Activate atuin, then pin a deterministic prompt the harness synchronises on.
# unsetopt PROMPT_CR/PROMPT_SP strips zsh's partial-line "%" marker so the pyte
# screen stays clean.
RUN printf '%s\n' \
      'eval "$(atuin init zsh)"' \
      'unsetopt PROMPT_CR PROMPT_SP 2>/dev/null || true' \
      "PROMPT='ATUIN_E2E> '" \
      'RPROMPT=""' \
      > /home/atuin/.zshrc
CMD ["sleep", "infinity"]
