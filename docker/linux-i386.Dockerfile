# Build image for lambdabots_mm_i386.so. Ubuntu 18.04 gives glibc 2.27, the same baseline as the BugfixedHL-Rebased
# build image, so the module loads on any server that runs BHL.
#
# The i686 cross toolchain (GCC 7, glibc 2.27, static libstdc++) exists for both amd64 and arm64 hosts, so the image
# runs natively on CI runners and on Apple Silicon: rustc crashes under QEMU amd64 emulation. Rust builds the core for
# i686-unknown-linux-gnu with the host toolchain; CMake comes from Kitware because 18.04 ships 3.10.
FROM ubuntu:18.04

ARG RUST_VERSION=1.94.0
ARG CMAKE_VERSION=3.31.8

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        binutils binutils-i686-linux-gnu ca-certificates curl file gcc g++ g++-i686-linux-gnu gcc-i686-linux-gnu \
        git libc6-dev-i386-cross libstdc++-7-dev-i386-cross make ninja-build python3 xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN arch="$(uname -m)" \
    && curl -fsSL "https://github.com/Kitware/CMake/releases/download/v${CMAKE_VERSION}/cmake-${CMAKE_VERSION}-linux-${arch}.tar.gz" \
        | tar -xz -C /opt \
    && ln -s "/opt/cmake-${CMAKE_VERSION}-linux-${arch}/bin/cmake" /usr/local/bin/cmake \
    && ln -s "/opt/cmake-${CMAKE_VERSION}-linux-${arch}/bin/ctest" /usr/local/bin/ctest

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain "${RUST_VERSION}" --target i686-unknown-linux-gnu \
    && mkdir -p /opt/cargo/registry /opt/cargo/git \
    && chmod -R a+rwX /opt/rustup /opt/cargo \
    && chmod 1777 /opt/cargo/registry /opt/cargo/git

# i686 test binaries run through the cross loader: natively on x86 hosts, through binfmt emulation elsewhere.
ENV HOME=/tmp \
    CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_LINKER=i686-linux-gnu-gcc \
    CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUNNER="/usr/i686-linux-gnu/lib/ld-linux.so.2 --library-path /usr/i686-linux-gnu/lib" \
    CC_i686_unknown_linux_gnu=i686-linux-gnu-gcc \
    CXX_i686_unknown_linux_gnu=i686-linux-gnu-g++ \
    AR_i686_unknown_linux_gnu=i686-linux-gnu-ar
WORKDIR /work/lambdabots
