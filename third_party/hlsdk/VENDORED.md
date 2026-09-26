# Vendored: hlsdk-portable headers

Header subset of the Half-Life SDK from [FWGS/hlsdk-portable](https://github.com/FWGS/hlsdk-portable)
(64-bit clean, ABI-identical to GoldSrc on 32-bit targets).

| Item    | Value                                      |
|---------|--------------------------------------------|
| Commit  | `c95f56a16db83191b0f150c19e6460ffeb171f48` |
| Date    | 2026-09-26                                 |
| License | Half Life 1 SDK License (`LICENSE`)        |

Copied without modification: `common/*.h`, `engine/*.h`, `pm_shared/*.h`, `dlls/cdll_dll.h`,
`public/build.h`. Only the C++ adapter includes these headers; the Rust core never sees them.
