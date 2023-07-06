---
title: Atuin's new encryption scheme
description: Details about the new encryption system for Atuin
slug: new-encryption
authors: [conrad]
tags: [insights]
---

End-to-end encryption is an essential component of Atuin.
One of our core philosophies, when we created the sync service, is that
we want no reason to worry about user data. The shell is a very
sensitive system with API keys, AWS credentials, account passwords, etc.
We didn't want to give the opportunity for that data to leak, either through an attack,
through a mistake on our part, or through state actor interference.

If there's one thing I have learnt as an engineer, it's that cryptography is hard.
If you are an expert in cryptographic implementations or cryptoanalysis, please get in touch.
This post will cover my research as - a non-crypto expert - into the long-term security of Atuin history data.

> Disclaimer, where reasonable, I have considered the security of
> [Side channel attacks](https://en.wikipedia.org/wiki/Side-channel_attack).
> Right now, our biggest concern is attacks on the atuin server, where the encrypted data is stored at rest.
> All atuin data is stored unencrypted on your local device in order to perform search queries. Improvements
> to cryptographic implementations can come in later revisions if any realistic side-channel attacks are found.

## TL;DR

Our encryption system is changing from [NaCl secretbox](https://nacl.cr.yp.to/secretbox.html),
and moving to [PASETO v4 local encryption](https://github.com/paseto-standard/paseto-spec/tree/master/docs/01-Protocol-Versions#version-4-sodium-modern)
with [PASERK local key wrapping](https://github.com/paseto-standard/paserk/blob/master/types/local-wrap.md).

## Backstory

All the way back in [April 2021, in our V0.5 release](https://github.com/ellie/atuin/pull/31/files#diff-6cb394acf0a1c664cf29bc71085c713dc29308df03dfcd58d44d91b536201041),
Ellie decided to use the [NaCl standard](https://nacl.cr.yp.to/) (aka salt/libsodium) for our encryption as a tried and trusted standard.
Specifically, [secretbox](https://nacl.cr.yp.to/secretbox.html) was the algorithm of choice.

If you're not familiar, secretbox is an implementation of _authenticated symmetrical encryption_. This means
that only the owner of the encryption key can decrypt the data (this will be the user), and that any attempts
to tamper with the data can be detected.

Honestly, this is a great system and offers everything we needed. However, our interface to libsodium was an unmaintained crate called [sodiumoxide](https://github.com/sodiumoxide/sodiumoxide) and had issues being portable. Because of this, I started looking into what algorithms libsodium uses underneath and if we can use a native Rust implementation.

Secretbox is made up of 2 main components. A stream-cipher and a message authentication code.
These are XSalsa20 and Poly1305 respectively, designed by NaCl's author [Daniel J. Bernstein](https://en.wikipedia.org/wiki/Daniel_J._Bernstein).
In a brave effort, I decided to [roll my own crypto](https://security.stackexchange.com/questions/18197/why-shouldnt-we-roll-our-own)
and implement this [XSalsa20 + Poly1305 system in Rust](https://github.com/ellie/atuin/pull/805).

> NOTE: I didn't actually implement the underlying algorithms. we are using:
> * [poly1305](https://github.com/RustCrypto/universal-hashes/tree/master/poly1305)
> * [salsa20](https://github.com/RustCrypto/stream-ciphers/tree/master/salsa20)
> From the RustCrypto project.
> These algorithms are not known to be vulnerable to software-based side-channel attacks.

## Back to the drawing board

After peeling back the veil that is our cryptographic implementation,
I started thinking a lot more about just how secure the system is.

The more I started looking, the more I noticed potential improvements.
Salsa20/Poly1305 both date back to 2005. In another 20 years, is this system going to still be secure?

Let's take a look at some potential attacks

### We don't guarantee a unique Initialisation Vector per message

We use a random 192-bit IV. There is a known attack on stream-ciphers if the
Key + IV pair is ever re-used. For all practical purposes, this is enough, assuming the OS random source is any good. A [birthday attack](https://en.wikipedia.org/wiki/Birthday_attack) calculation suggests that it needs in the order of 10^23 messages for a one-in-a-trillion chance of collision.

This is not an issue as all of our users combined are never going to generate 10^23 entries, and we certainly aren't willing to store [zettabytes](https://en.wikipedia.org/wiki/Byte#Multiple-byte_units) of their data.

### We use the same key for each message

Shell history is quite predictable. If you have a 2-byte history entry, it's quite likely that it's `ls`. Given the encrypted blob, you can start to brute force the associated key. A proof was published stating that no attack on Salsa20 with 128-bit key is possible with an average search time of less than 2^130 (about 10^39) random guesses.

To put that number into perspective. Performing 1 billion key operations per CPU core per second, and using a suite of 1 billion CPU cores, the attack will take roughly 10 trillion years.

Atuin uses a 256-bit key which is even more secure, and therefore not at risk of a practical brute-force attack. It follows that we are likely safe from a known plain-text attack.

However, there is still the issue of key leaking. We have no key-upgrade policy.
If a key is leaked, maybe through a side-channel attack, a social attack, or malware,
then the only solution is to create a new account with a new key.

This is partially an issue.

## Change

While researching these systems, I have learnt a lot more new techniques that modern cryptographic systems use. While the analysis above indicates that we are protected, there might be attacks we are unaware of, so keeping up with modern research is important.

We're also in the middle of redesigning our sync service. While we're already planning a big change, we might as well consider updating the encryption too

### Key wrapping

A common approach to encrypting lots of different data is the use of wrapped keys.
The idea here is that each payload has an ephemeral encryption key.
This is then encrypted (wrapped) using the master key and stored with the data.

Initially, this seemed less secure to me. However, my analysis seems to point out that
the master key is less vulnerable to side-channel attacks since it is less used.
It also offers no decrease in security since brute-forcing the master key from an
ephemeral data key is just as hard as it is for any message. In the end, it's like
a password manager for your encrypted data.

This unlocks some potential future upgrades.

1. Key rotation is easier since you need to re-encrypt the wrapped keys. This means much less data needs to be updated.
2. Wrapped data keys can be decrypted in Hardware Security Modules (HSM) which are immune to side-channel attacks

### Stronger ciphers

XSalsa20 was later superseded by XChaCha20 by the same author. It has a very similar construction,
but the stream cipher has better mixing characteristics, which makes any non-brute-force attacks harder to craft.

## Conclusion

I started to craft a new solution using these concepts.
But eventually, I realised that I shouldn't be reinventing the wheel here.
During more and more of my research sessions, I stumbled upon [PASETO](https://paseto.io/).
While the intended use case is security tokens,
their local encryption scheme is designed such that encrypted data is safe to be shared publically.
Their V4 scheme also uses the XChaCha20 cipher which I was initially planning to use.

In the end, I bit the bullet and decided to use the standard.
The nice thing with secretbox is that existing implementations in other languages are widely available.
Making it easy to implement sync in third parties. If we implemented our own scheme,
that would make it much easier for third parties to make mistakes if they wanted to use the sync data directly.

Using PASETO, there are existing implementations that we didn't have to write.
This means that we don't build software doomed to die a lonely death. It also means
that we benefit directly from future versions of the specification.

