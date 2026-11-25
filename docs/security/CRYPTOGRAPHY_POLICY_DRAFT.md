# Cryptography policy proposal

**Status: Draft for review.** This document proposes a future cryptography policy; it does not describe a validated security implementation. No security-provider integration is established by this draft. It is not an ATO, security assessment, or claim that OpenATCA or a product build is FIPS validated.

## Purpose and current boundary

The workspace's current SHA-256 implementation is used for content identity, reproducibility, and accidental-corruption checks. These digests are unkeyed. A party that can replace both a file and its digest can recompute the digest. They do not authenticate a producer, authorize an action, protect a channel, or prove provenance. Do not use them as signatures, message authentication codes, key derivation, or security decisions.

Any future adversarial-authentication or confidentiality service must use a reviewed cryptographic provider and protocol, with identity, authorization, freshness, key-purpose, and lifecycle controls. A cryptographic module alone does not provide application authorization, trust enrollment, a secure protocol, or an ATO.

## Candidate module boundary

As checked in NIST's CMVP records on 2026-11-25, AWS-LC 4 certificates **5595** (dynamic) and **5596** (static) were listed Active, FIPS 140-3, Security Level 1, with a sunset date of 2031-11-18. Each certificate and its security policy govern the precise module, services, configuration, and approved-mode conditions. These records validate modules under stated conditions; they do not validate OpenATCA, an application, or a deployment as a whole.

- [CMVP certificate 5595: AWS-LC 4 dynamic](https://csrc.nist.gov/projects/cryptographic-module-validation-program/certificate/5595)
- [CMVP certificate 5596: AWS-LC 4 static](https://csrc.nist.gov/projects/cryptographic-module-validation-program/certificate/5596)
- [AWS-LC 4 dynamic module security policy, 5595](https://csrc.nist.gov/CSRC/media/projects/cryptographic-module-validation-program/documents/security-policies/140sp5595.pdf)
- [AWS-LC 4 static module security policy, 5596](https://csrc.nist.gov/CSRC/media/projects/cryptographic-module-validation-program/documents/security-policies/140sp5596.pdf)

The policies identify AWS-LC FIPS 4.0.0 `bcm.o` as the cryptographic boundary. Certificate 5595 covers the dynamic module configuration specified by its policy; 5596 covers `bcm.o` statically linked into a user-space application. The policies' tested operating environments are specific Amazon Linux 2023 and EC2 bare-metal configurations. A different build, host, or operating environment is not shown by those records to be covered. The exact installation, initialization, configuration, approved service, and mode requirements must be followed for any selected use.

AWS-LC is a software cryptographic module, not an HSM or TPM. It does not by itself satisfy a hardware key-custody requirement. Loading a key from hardware into software does not establish that the key remains in hardware custody. The intended HSM/TPM product, the applicable custody boundary, and whether custody must include ephemeral TLS secrets remain unresolved. No software fallback should be inferred or authorized by this proposal.

## Proposed service and protocol constraints

The following are design constraints for review, not a selected or implemented profile:

- **Authenticated records:** use a signature or an explicitly approved shared-secret MAC from an appropriately validated service. Bind canonical record bytes to protocol version, signer or key identity and role, intended action, sequence/freshness, and relevant input/state identity. Verification must check trust, role, validity, revocation, and replay policy before the record affects state.
- **Transport:** evaluate TLS 1.3 with mutual authentication for node-to-node connections, a reviewed certificate and revocation policy, pinned acceptable versions and suites, and explicit downgrade behavior. [NIST SP 800-52 Rev. 2](https://csrc.nist.gov/pubs/sp/800/52/r2/final) specifies TLS 1.2 with FIPS suites and support for TLS 1.3; any TLS 1.2 interoperability need should be separately reviewed. A module's validated primitives do not establish that a complete TLS stack is validated. The relevant security policy and key-custody rule must cover ephemeral key agreement, traffic secrets, derivation, and AEAD use.
- **Key lifecycle:** define owners and procedures for generation, enrollment, purpose separation, activation, rotation, revocation, compromise response, backup or non-recoverability, recovery, and destruction. Keys must have explicit identities, purposes, algorithms, providers, and policy versions. Follow [NIST SP 800-57 Part 1 Rev. 5](https://csrc.nist.gov/pubs/sp/800/57/pt1/r5/final) and the selected provider's policy; set key limits from the applicable algorithm and policy rather than one universal interval.
- **Algorithm agility and post-quantum work:** version and identify algorithms and protocols; reject unknown or downgraded choices and support only reviewed migration windows. [FIPS 203 ML-KEM](https://csrc.nist.gov/pubs/fips/203/final) and [FIPS 204 ML-DSA](https://csrc.nist.gov/pubs/fips/204/final) are standards to evaluate, not proof of product support or an approved end-to-end protocol. Any hybrid design needs a standardized combiner, provider-service coverage, hardware-custody compatibility, and independent protocol review. Do not claim post-quantum readiness from primitive availability.
- **Confidentiality at rest:** add authenticated encryption only where a threat assessment requires it. Any use must define nonce uniqueness, per-key limits, replay handling, rotation, and failure behavior. Keep this separate from content digests.

## Review gates

Before implementation, owners should decide which data flows need reproducibility only, authenticated integrity, peer authentication, confidentiality, or a combination; confirm applicable aviation/security requirements; define trust bootstrap, authorization, freshness, and offline revocation behavior; and select a compatible hardware provider and deployment environment. The key-custody decision must explicitly say whether it includes ephemeral TLS key shares, derived traffic secrets, and transient key-establishment state. If the chosen provider cannot satisfy the approved custody rule, the affected service remains unavailable rather than silently falling back to software keys.

For any future compliance statement, record the exact module certificate, module artifact and build, target environment, approved service and parameters, required configuration, and evidence of approved-mode operation. Local known-answer tests, a dependency feature flag, or a module certificate alone do not establish module or application validation.

## References

- [NIST CMVP validated modules](https://csrc.nist.gov/projects/cryptographic-module-validation-program/validated-modules)
- [FIPS 186-5, Digital Signature Standard](https://csrc.nist.gov/pubs/fips/186-5/final)
- [FIPS 180-4, Secure Hash Standard](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)
- [NIST CAVP secure hashing](https://csrc.nist.gov/projects/cryptographic-algorithm-validation-program/secure-hashing)

