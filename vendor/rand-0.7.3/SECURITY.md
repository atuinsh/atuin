# Security Policy

## No guarantees

Support is provided on a best-effort bases only.
No binding guarantees can be provided.

## Security premises

Rand provides the trait `rand_core::CryptoRng` aka `rand::CryptoRng` as a marker
trait. Generators implementating `RngCore` *and* `CryptoRng`, and given the
additional constraints that:

-   Instances of seedable RNGs (those implementing `SeedableRng`) are
    constructed with cryptographically secure seed values
-   The state (memory) of the RNG and its seed value are not be exposed

are expected to provide the following:

-   An attacker can gain no advantage over chance (50% for each bit) in
    predicting the RNG output, even with full knowledge of all prior outputs.

For some RNGs, notably `OsRng`, `ThreadRng` and those wrapped by `ReseedingRng`,
we provide limited mitigations against side-channel attacks:

-   After a process fork on Unix, there is an upper-bound on the number of bits
    output by the RNG before the processes diverge, after which outputs from
    each process's RNG are uncorrelated
-   After the state (memory) of an RNG is leaked, there is an upper-bound on the
    number of bits of output by the RNG before prediction of output by an
    observer again becomes computationally-infeasible

Additionally, derivations from such an RNG (including the `Rng` trait,
implementations of the `Distribution` trait, and `seq` algorithms) should not
introduce signficant bias other than that expected from the operation in
question (e.g. bias from a weighted distribution).

## Supported Versions

We will attempt to uphold these premises in the following crate versions,
provided that only the latest patch version is used, and with potential
exceptions for theoretical issues without a known exploit:

| Crate | Versions | Exceptions |
| ----- | -------- | ---------- |
| `rand` | 0.7 |  |
| `rand` | 0.5, 0.6 | Jitter |
| `rand` | 0.4 | Jitter, ISAAC |
| `rand_core` | 0.2 - 0.5 | |
| `rand_chacha` | 0.1 - 0.2 | |
| `rand_hc` | 0.1 - 0.2 | |

Explanation of exceptions:

-   Jitter: `JitterRng` is used as an entropy source when the primary source
    fails; this source may not be secure against side-channel attacks, see #699.
-   ISAAC: the [ISAAC](https://burtleburtle.net/bob/rand/isaacafa.html) RNG used
    to implement `thread_rng` is difficult to analyse and thus cannot provide
    strong assertions of security.

## Known issues

In `rand` version 0.3 (0.3.18 and later), if `OsRng` fails, `thread_rng` is
seeded from the system time in an insecure manner.

## Reporting a Vulnerability

To report a vulnerability, [open a new issue](https://github.com/rust-random/rand/issues/new).
Once the issue is resolved, the vulnerability should be [reported to RustSec](https://github.com/RustSec/advisory-db/blob/master/CONTRIBUTING.md).
