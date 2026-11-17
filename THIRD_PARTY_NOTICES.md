# Third-party notices

The maintained Sideglow project source is provided under MPL-2.0 in `LICENSE`.
This does not change the licenses of the third-party components listed below.
See `NOTICE` and `docs/LICENSING.md` for the scope of the project's MPL license
and the original licenses of the external components.

The 21 exact crate archives were checked against Cargo.lock, and all 42 bundled
crate license files were byte-compared with the upstream archive files.
The Rust standard-library and MinGW/GCC package notices match the actual
toolchain used for this release. The upstream MinGW v12.0.0 runtime-specific
notice is additionally included in `third_party_licenses/mingw-w64/`.
Evidence and limitations: `docs/PROVENANCE.md` and `docs/provenance/`.

The following table lists all locked Rust dependencies, including build-time and target-specific packages. Not every package is linked into the Windows x64 executable. Original license and copyright files are preserved under `third_party_licenses/`. The supplied GNU-target executable also includes Rust standard-library and MinGW runtime code; their notices are included there. Windows system DLLs are supplied by Windows and are not redistributed in this package. The Node.js relay uses Node built-in modules and has no npm dependencies; Node.js itself is not included.

| Package | Version | Declared license |
| --- | --- | --- |
| itoa | 1.0.19 | MIT OR Apache-2.0 |
| memchr | 2.8.3 | Unlicense OR MIT |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.154 | MIT OR Apache-2.0 |
| syn | 3.0.10 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| zmij | 1.0.23 | MIT |
