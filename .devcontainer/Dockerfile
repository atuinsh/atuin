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
    cargo install cargo-nextest --locked || true

# Use mold by default
RUN mkdir -p /root/.cargo && printf '[target.x86_64-unknown-linux-gnu]\nlinker = "clang"\nrustflags = ["-C", "link-arg=-fuse-ld=mold"]\n' >> /root/.cargo/config.toml

# --- claude code ---
# Binary only. Credentials are NEVER baked in: this image is published to a
# shared registry, so anything in a layer is readable by anyone who can pull it.
# Auth arrives at runtime via CLAUDE_CODE_OAUTH_TOKEN (see the sandbox template).
# The native installer puts the launcher in ~/.local/bin, so it needs to be on
# PATH for non-login shells (kubectl exec).
RUN curl -fsSL https://claude.ai/install.sh | bash
ENV PATH="/root/.local/bin:${PATH}"

# --- shell setup ---
RUN chsh -s /usr/bin/zsh root

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
