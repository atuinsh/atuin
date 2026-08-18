FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# --- system + build deps ---
# protobuf-compiler: atuin daemon (tonic/prost) needs protoc at build time.
# mold: fast linker, big win on iterative Rust builds.
RUN apt-get update && apt-get install -y --no-install-recommends \
      openssh-server zsh tmux ripgrep fd-find jq \
      protobuf-compiler mold clang \
      sqlite3 libssl-dev pkg-config && \
    rm -rf /var/lib/apt/lists/* && \
    mkdir -p /run/sshd

# --- rust conveniences ---
RUN rustup component add rust-analyzer clippy rustfmt && \
    rustup toolchain install nightly --profile minimal --component rustfmt && \
    cargo install cargo-nextest --locked || true

# Use mold by default. This has to live in $CARGO_HOME (/usr/local/cargo in this
# base image), NOT ~/.cargo: cargo reads $CARGO_HOME/config.toml and ignores
# ~/.cargo/config.toml whenever CARGO_HOME points elsewhere, so the old
# /root/.cargo/config.toml was silently dead. Here it also applies to every
# user (root, dev, vscode) rather than just root.
RUN printf '[target.x86_64-unknown-linux-gnu]\nlinker = "clang"\nrustflags = ["-C", "link-arg=-fuse-ld=mold"]\n' >> "${CARGO_HOME:-/usr/local/cargo}/config.toml"

# --- dev user (the default user for this image) ---
# The base image already ships `vscode` at uid 1000, so `dev` lands on 1001.
# Passwordless sudo is granted via sudoers.d, mirroring how the base image
# grants it to vscode; visudo -c validates the file at build time so a typo
# fails the build rather than locking sudo at runtime.
RUN useradd --create-home --shell /usr/bin/zsh --groups sudo dev && \
    echo 'dev ALL=(root) NOPASSWD:ALL' > /etc/sudoers.d/dev && \
    chmod 0440 /etc/sudoers.d/dev && \
    visudo -c -f /etc/sudoers.d/dev

# --- shell setup ---
# root keeps zsh too, for `sudo -i`.
RUN chsh -s /usr/bin/zsh root

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Everything past this point runs as dev, so per-user installs land in
# /home/dev rather than /root.
USER dev
# HOME is set explicitly: Kubernetes does not always derive it from the image
# USER, and claude/git/gh all resolve their config through it.
ENV HOME=/home/dev

# --- claude code ---
# Binary only. Credentials are NEVER baked in: this image is published to a
# shared registry, so anything in a layer is readable by anyone who can pull it.
# Auth arrives at runtime via CLAUDE_CODE_OAUTH_TOKEN (see the sandbox template).
# The native installer puts the launcher in ~/.local/bin, so it needs to be on
# PATH for non-login shells (kubectl exec).
RUN curl -fsSL https://claude.ai/install.sh | bash
ENV PATH="/home/dev/.local/bin:${PATH}"

ENTRYPOINT ["/entrypoint.sh"]
