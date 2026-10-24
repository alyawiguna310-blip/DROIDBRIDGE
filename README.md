# DROIDBRIDGE

**DROIDBRIDGE** is an experimental cross-platform Android compatibility project with two planned host frontends:

- **WINDROID** — Windows host integration.
- **LINDROID** — Linux host integration.

The shared core is intended to manage APK metadata, portable device profiles, runtime backends, and performance settings. The long-term goal is to run Android apps and games in host-integrated windows while using the host machine's actual CPU and GPU.

> **Status: early scaffold.** This repository does not yet run Android APKs. Android framework compatibility, ART, graphics, ARM translation, and Google Play support are separate engineering milestones, not completed features.

## Initial configuration

- Performance preset: **balanced**
- Google services preference: **Play Store + required services**, where a compatible, properly licensed runtime is available
- Portable settings: **enabled**
- Example device profile: **POCO F6**
- Host hardware reporting: **actual host capabilities remain distinct from reported Android device properties**

Device identity profiles must not claim unsupported CPU instructions or GPU capabilities. Apps may detect differences between reported identity and real host capabilities.

## Repository layout

- `crates/droid-core` — shared types and profile validation.
- `profiles/POCO_F6.json` — example portable profile.
- `docs/architecture.md` — staged architecture and implementation plan.

## Development

Install a current stable Rust toolchain, then run:

```sh
cargo test --workspace
cargo check --workspace
```

## Roadmap

1. Validate portable configuration and host/device profile separation.
2. Implement APK archive inspection and manifest metadata extraction.
3. Define a replaceable runtime backend interface.
4. Evaluate existing Android runtimes separately for Windows and Linux.
5. Validate execution of a real APK, graphics, input, and lifecycle.
6. Investigate ARM64 native-library translation where required.
7. Evaluate legitimate Google Play support and its certification constraints.
8. Benchmark RAM, CPU, GPU, startup time, and portability across machines.

## Licensing

The project license has not yet been selected. Runtime components and third-party dependencies must be evaluated under their own licenses before redistribution.
