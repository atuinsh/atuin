# fish client image for atuin E2E tests. Build context: e2e/images
#   docker build -t atuin-e2e-fish:local -f Dockerfile.fish .
FROM atuin-e2e-base:local
USER root
RUN apt-get update \
 && apt-get install -y --no-install-recommends fish \
 && rm -rf /var/lib/apt/lists/*
USER atuin
RUN mkdir -p /home/atuin/.config/fish \
 && printf '%s\n' \
      'atuin init fish | source' \
      'function fish_prompt; printf "ATUIN_E2E> "; end' \
      'function fish_right_prompt; end' \
      > /home/atuin/.config/fish/config.fish
CMD ["sleep", "infinity"]
