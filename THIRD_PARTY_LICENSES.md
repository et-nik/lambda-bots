# Third-party components and licenses

lambdabots is distributed under the MIT license (`LICENSE`). The components below are included in the project
or used during the build.

## Included in the sources or the binary

| Component                  | What is used                                  | License                     |
|----------------------------|-----------------------------------------------|-----------------------------|
| YaPB / yapb-halflife       | behavior algorithms, name lists, observer     | MIT (text in `NOTICE`)      |
| hlsdk-portable (FWGS)      | Half-Life SDK headers in `third_party/hlsdk/` | Half Life 1 SDK License     |
| ReHLDS                     | public API declarations in `adapter/src/`     | MIT (`third_party/rehlds`)  |
| Rust dependencies (crates) | statically linked into the module             | MIT / Apache-2.0 and others |

- **hlsdk-portable.** A subset of headers is copied unmodified; the commit and file list are in
  `third_party/hlsdk/VENDORED.md`. The Valve SDK license allows free distribution of modifications and
  requires shipping `third_party/hlsdk/LICENSE`.
- **ReHLDS.** The SDK is not copied: `adapter/src/rehlds_api.h` reproduces only the required interfaces in the
  original order. Details are in `third_party/rehlds/VENDORED.md`.
- **Metamod.** The ABI header `adapter/include/lb/metamod_abi.h` was written from scratch based on the
  interface 5:13 description; the text of Metamod's GPL headers is not part of the project.
- **Rust dependencies.** Allowed licenses are listed in `deny.toml` (`[licenses] allow`); CI checks them
  with `cargo deny check licenses`. Full list with versions:
  `cargo metadata --format-version 1 | jq '.packages[] | {name, version, license}'`.

## Build-only

| Component | Purpose                                               | License      |
|-----------|-------------------------------------------------------|--------------|
| Corrosion | building the Rust staticlib from CMake (FetchContent) | MIT          |
| cbindgen  | generating `lb_abi.h` and `lb_core.h` (`cargo xtask`) | MPL-2.0      |
| CMake     | building the adapter (Docker image)                   | BSD-3-Clause |
