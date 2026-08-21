# bash client image for atuin E2E tests. Build context: e2e/images
#   docker build -t atuin-e2e-bash:local -f Dockerfile.bash .
FROM atuin-e2e-base:local
# bash ships in the base image; `atuin init bash` bundles bash-preexec itself.
USER atuin
RUN printf '%s\n' \
      'eval "$(atuin init bash)"' \
      "PS1='ATUIN_E2E> '" \
      > /home/atuin/.bashrc
CMD ["sleep", "infinity"]
