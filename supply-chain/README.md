# supply-chain

## Who

Currently just me, Conrad Ludgate.

## What

Audit dependencies.

This directory makes use of <https://github.com/mozilla/cargo-vet> in order to
1) Import audit results from well known actors.
2) Keep record of the list of crates that I have personally audited.

If there is a record in the audits.toml, authored by me, and the commit that introduced it was
signed by me, then that means that I have read through all the code that was introduced and that
I personally believe it to be of an acceptable quality. I will not define what an acceptable quality means,
but I am operating with the best interests of atuin and the Rust ecosystem.

## Where/When

Here, now.

Currently there's a bunch of crates with excemptions. These are crates
that I haven't auditted that we currently blindly trust. Ideally this list will go to 0.

Right now, I'm not dead set on having this be a hard rule that crates be audited.
Maybe before release I will go through any deltas and update. This is primarily a hint for me
and not a rule.

## Why

Security.

I am quite interested in making sure the software I maintain is secure.
Part of that security is reduced by using third party crates. On occasion, some crates
have featured malware. There are notorious cases in the npm industry and the crates.io
registry offers no extra security than npm has.

## How

1. Install `cargo-vet`
2. Run `cargo vet suggest`
3. Choose a crate that is suggested and run the corresponding command
4. Read through the source code and certify.

With `jq` installed, I have an interactive script that makes the process easier for myself:
`./supply-chain/vet.sh safe-to-deploy`

## Cool

Thanks. If you trust me, you can import the audits I have performed by adding

```toml
[imports.atuinsh]
url = "https://raw.githubusercontent.com/atuinsh/atuin/vetting/supply-chain/audits.toml"
```

