//! Constructors for Atuin's outbound HTTP clients.
//!
//! Anything that talks to a sync server, the hub, or a model provider should
//! build its client through here, so that platform-specific configuration
//! lives in one place rather than at each call site.

/// A [`reqwest::ClientBuilder`] with Atuin's platform defaults applied.
///
/// Prefer this over [`reqwest::Client::builder`] directly.
pub fn client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();

    #[cfg(target_env = "musl")]
    let builder = builder.dns_resolver(musl_dns::HickoryResolver);

    builder
}

/// A [`reqwest::Client`] with Atuin's platform defaults applied.
///
/// Drop-in replacement for [`reqwest::Client::new`], and panics under the same
/// circumstances (a TLS backend that fails to initialise).
pub fn client() -> reqwest::Client {
    client_builder()
        .build()
        .expect("failed to build HTTP client")
}

/// DNS resolution for musl targets.
///
/// musl's `getaddrinfo` queries every nameserver in `resolv.conf` in parallel
/// and takes the first definitive reply. On a split-horizon network, a
/// nameserver that doesn't serve the internal zone can therefore answer
/// NXDOMAIN before the one that does, and the lookup fails. glibc queries
/// nameservers in order and never sees the loser's answer, which is why only
/// the musl release artifacts are affected.
///
/// Swapping in hickory-dns fixes the parallel-race half of this, but not the
/// whole problem: hickory orders nameservers by observed latency
/// (`ServerOrderingStrategy::QueryStatistics`), so a nameserver that NXDOMAINs
/// instantly earns the *best* stats and gets promoted to first, and
/// `NameServerConfig::trust_negative_responses` then makes hickory accept that
/// answer without consulting the others. Measured on such a network, that
/// still failed roughly half of all lookups.
///
/// So we build the resolver ourselves with negative-response trust disabled: a
/// bare NXDOMAIN no longer ends the lookup, it falls through to the remaining
/// nameservers. Note this is deliberately more forgiving than glibc, which
/// would also fail here if the unhelpful nameserver happened to be listed
/// first — the goal is to resolve wherever an answer exists, not to reproduce
/// glibc exactly. Only the empty-NXDOMAIN case is affected; every other
/// response code is retried regardless of this setting.
#[cfg(target_env = "musl")]
mod musl_dns {
    use std::net::SocketAddr;
    use std::sync::OnceLock;

    use hickory_resolver::{
        TokioResolver,
        config::{GOOGLE, LookupIpStrategy, ResolverConfig},
        net::{NetError, runtime::TokioRuntimeProvider},
        system_conf,
    };
    use reqwest::dns::{Addrs, Name, Resolve, Resolving};

    /// Built on first use: constructing it needs a Tokio runtime, and
    /// [`client_builder`](super::client_builder) may be called outside one.
    static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();

    #[derive(Debug, Clone, Copy)]
    pub(super) struct HickoryResolver;

    impl Resolve for HickoryResolver {
        fn resolve(&self, name: Name) -> Resolving {
            Box::pin(async move {
                let resolver = match RESOLVER.get() {
                    Some(resolver) => resolver,
                    // Two callers racing here both build one; `get_or_init`
                    // keeps whichever lands first and drops the other.
                    None => {
                        let built = build()?;
                        RESOLVER.get_or_init(|| built)
                    }
                };

                let lookup = resolver.lookup_ip(name.as_str()).await?;
                let addrs: Addrs = Box::new(
                    lookup
                        .iter()
                        .map(|ip| SocketAddr::new(ip, 0))
                        .collect::<Vec<_>>()
                        .into_iter(),
                );
                Ok(addrs)
            })
        }
    }

    fn build() -> Result<TokioResolver, NetError> {
        let mut builder = match system_conf::read_system_conf() {
            Ok((system, options)) => {
                let mut config = ResolverConfig::from_parts(
                    system.domain().cloned(),
                    system.search().to_vec(),
                    vec![],
                );
                for mut name_server in system.name_servers().to_vec() {
                    name_server.trust_negative_responses = false;
                    config.add_name_server(name_server);
                }

                let mut builder =
                    TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
                *builder.options_mut() = options;
                builder
            }
            // Same fallback reqwest's own hickory integration uses when
            // resolv.conf can't be read.
            Err(err) => {
                tracing::debug!(
                    ?err,
                    "could not read system DNS configuration; falling back to Google DNS"
                );
                TokioResolver::builder_with_config(
                    ResolverConfig::udp_and_tcp(&GOOGLE),
                    TokioRuntimeProvider::default(),
                )
            }
        };

        // Match reqwest's own hickory setup so "happy eyeballs" still works.
        builder.options_mut().ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        builder.build()
    }
}
