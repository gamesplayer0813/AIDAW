#!/usr/bin/env bash
# Cross-build aidaw.exe with ASIO from Linux, with MinGW, as aidaw-asio.exe.
#
#   packaging/build-asio-mingw.sh [OUT_DIR]
#
# Read the README's ASIO section first: the exe this makes contains the
# Steinberg ASIO SDK, so it may only be passed on under the SDK's licence
# (GPLv3, or Steinberg's own), not just AIDAW's MIT licence.
#
# Needs: rustup target add x86_64-pc-windows-gnu; mingw-w64 (g++); clang
# (libclang, for bindgen); curl. asio-sys downloads the SDK unless
# CPAL_ASIO_DIR points at an unpacked copy.
#
# On Windows with the MSVC toolchain none of this is needed:
#   cargo build --release -p aidaw-gui --features asio
#
# Two things differ from that build:
# - asio-sys 0.4 runs bindgen's layout tests for every target, and MinGW lays
#   out the SDK's C++ classes differently from MSVC (a derived class reuses
#   its base's tail padding), so the build fails. Rust only uses the plain C
#   structs, so this builds against a copy of asio-sys whose layout tests run
#   only for MSVC (Cargo.lock is put back afterwards).
# - The SDK is C++, and MinGW would link the C++ library as libstdc++-6.dll,
#   which Windows doesn't have. A folder holding only the static libstdc++.a
#   makes the linker take that instead, so the exe needs no extra DLLs.
# - asio-sys only names the C++ library when it compiles the SDK, which it
#   skips when an earlier build left the compiled SDK behind; the next link
#   then fails ("undefined reference to operator new"). The copy names it on
#   every run.
set -euo pipefail

target=x86_64-pc-windows-gnu
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/target/$target/release}
work=$root/target/asio-mingw
cd "$root"

version=$(grep -A1 '^name = "asio-sys"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p')
if [[ -z $version ]]; then
    echo "asio-sys is not in Cargo.lock" >&2
    exit 1
fi
cargo fetch --target "$target" >/dev/null
src=$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 2 -type d \
    -name "asio-sys-$version" | head -1)
if [[ -z $src ]]; then
    echo "asio-sys $version is not in the cargo registry" >&2
    exit 1
fi

mkdir -p "$work/static-cxx"
rm -rf "$work/asio-sys"
cp -r "$src" "$work/asio-sys"
sed -i 's/^\([[:space:]]*\)\.clang_arg("-x")/\1.layout_tests(std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc"))\n\1.clang_arg("-x")/' \
    "$work/asio-sys/build.rs"
sed -i 's/^\([[:space:]]*\)println!("cargo:rustc-link-lib=static=asio");/&\n\1println!("cargo:rustc-link-lib=stdc++");/' \
    "$work/asio-sys/build.rs"
if ! grep -q 'layout_tests' "$work/asio-sys/build.rs" ||
    ! grep -q 'rustc-link-lib=stdc++' "$work/asio-sys/build.rs"; then
    echo "couldn't patch asio-sys $version: its build.rs changed, update this script" >&2
    exit 1
fi
ln -sf "$(x86_64-w64-mingw32-g++ -print-file-name=libstdc++.a)" "$work/static-cxx/libstdc++.a"

cp Cargo.lock "$work/Cargo.lock"
trap 'cp "$work/Cargo.lock" "$root/Cargo.lock"' EXIT
# A target folder of its own: the flags differ from the plain Windows build,
# and sharing one would rebuild everything each time you switch.
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-L native=$work/static-cxx" \
    cargo build --release --target "$target" -p aidaw-gui --features asio \
    --target-dir "$work/target" \
    --config "patch.crates-io.asio-sys.path=\"$work/asio-sys\""

exe=$work/target/$target/release/aidaw.exe
if x86_64-w64-mingw32-objdump -p "$exe" | grep -qi 'DLL Name: lib'; then
    echo "aidaw-asio.exe still needs a MinGW DLL:" >&2
    x86_64-w64-mingw32-objdump -p "$exe" | grep -i 'DLL Name: lib' >&2
    exit 1
fi
mkdir -p "$out"
cp "$exe" "$out/aidaw-asio.exe"
echo "built $out/aidaw-asio.exe"
