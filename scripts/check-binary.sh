#!/usr/bin/env bash
# Checks a built lambdabots module: exactly the five Metamod exports, no dynamic C++ runtime, and on Linux
# GLIBC <= 2.27 (the BugfixedHL build baseline), no text relocations and a non-executable stack.
#
# Usage: scripts/check-binary.sh <lambdabots_mm_i386.so | lambdabots_mm.dll | lambdabots_mm.dylib>...
set -euo pipefail

MAX_GLIBC="2.27"
EXPORTS="GiveFnptrsToDll Meta_Attach Meta_Detach Meta_Init Meta_Query"
failed=0

fail() {
    echo "  FAIL: $*"
    failed=1
}

ok() {
    echo "  ok: $*"
}

# Prefers the i686 cross binutils: host binutils on non-x86 hosts may not read i386 objects.
tool() {
    if command -v "i686-linux-gnu-$1" >/dev/null 2>&1; then echo "i686-linux-gnu-$1"; else echo "$1"; fi
}

version_le() {
    [[ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -n1)" == "$1" ]]
}

check_exports() {
    local got="$1"
    if [[ "$got" == "$EXPORTS" ]]; then
        ok "exports: $got"
    else
        fail "exports are [$got], expected [$EXPORTS]"
    fi
}

check_elf() {
    local f="$1" NM OBJDUMP READELF
    NM="$(tool nm)"
    OBJDUMP="$(tool objdump)"
    READELF="$(tool readelf)"
    file -b "$f" | grep -q "ELF 32-bit LSB shared object, Intel 80386" && ok "ELF i386 shared object" ||
        fail "not an i386 shared object: $(file -b "$f")"

    check_exports "$("$NM" -D --defined-only "$f" | awk '$2 ~ /^[TW]$/ {print $3}' | sort | xargs)"

    local glibc
    glibc="$("$OBJDUMP" -T "$f" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n1)"
    if [[ -z "$glibc" ]] || version_le "$glibc" "$MAX_GLIBC"; then
        ok "max GLIBC ${glibc:-none} <= $MAX_GLIBC"
    else
        fail "needs GLIBC_$glibc (> $MAX_GLIBC):"
        "$OBJDUMP" -T "$f" | grep -E "GLIBC_$glibc\b" | sed 's/^/    /' | head -n 10
    fi

    # Version needs as "file:version"; i386 glibc itself provides _Unwind_Find_FDE@GCC_3.0.
    local cxx
    cxx="$("$READELF" -V "$f" | awk '{ for (i = 1; i < NF; i++) { if ($i == "File:") file = $(i + 1);
                                               if ($i == "Name:" && $(i + 1) ~ /^(GLIBCXX|CXXABI|GCC)_/) print file ":" $(i + 1) } }' |
        grep -v '^libc\.so\.6:GCC_' | sort -u | xargs || true)"
    [[ -z "$cxx" ]] && ok "no C++ runtime or libgcc_s symbol versions" || fail "runtime symbol versions: $cxx"

    local needed bad=""
    needed="$("$READELF" -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | xargs)"
    for lib in $needed; do
        case "$lib" in
            libc.so.6 | libm.so.6 | libdl.so.2 | libpthread.so.0 | librt.so.1 | ld-linux.so.2) ;;
            *) bad="$bad $lib" ;;
        esac
    done
    [[ -z "$bad" ]] && ok "NEEDED: $needed" || fail "unexpected NEEDED libraries:$bad"

    "$READELF" -d "$f" | grep -q TEXTREL && fail "text relocations present" || ok "no text relocations"
    "$READELF" -lW "$f" | grep GNU_STACK | grep -q RWE && fail "executable stack" || ok "non-executable stack"
}

check_macho() {
    local f="$1"
    check_exports "$(nm -gU "$f" | awk '{print $3}' | sed 's/^_//' | sort | xargs)"
    local id bad
    id="$(otool -D "$f" | tail -n +2)"
    bad="$(otool -L "$f" | tail -n +2 | awk '{print $1}' | grep -vxF "${id:-none}" |
        grep -vE '^(/usr/lib/|/System/Library/)' | xargs || true)"
    [[ -z "$bad" ]] && ok "only system libraries linked" || fail "non-system libraries: $bad"
}

check_pe() {
    local f="$1" dump
    if command -v llvm-objdump >/dev/null 2>&1; then
        dump="$(llvm-objdump -p "$f")"
    elif command -v objdump >/dev/null 2>&1; then
        dump="$(objdump -p "$f")"
    else
        fail "no objdump/llvm-objdump to inspect $f"
        return
    fi
    grep -q "i386\|pei-i386\|COFF-i386" <<<"$dump" && ok "PE i386" || fail "not an i386 PE file"
    local exports
    exports="$(awk '/Export Table:/,/^$/' <<<"$dump" | grep -oE '\b(GiveFnptrsToDll|Meta_[A-Za-z]+)\b' | sort -u | xargs)"
    check_exports "$exports"
    local imports
    imports="$(grep -oiE 'DLL Name: .*|^ *[a-z0-9_.-]+\.dll' <<<"$dump" | sed 's/DLL Name: //I' | tr 'A-Z' 'a-z' | sort -u | xargs)"
    grep -qE 'msvcp|vcruntime|api-ms-win-crt' <<<"$imports" && fail "dynamic MSVC runtime imported: $imports" ||
        ok "imports: $imports"
}

[[ $# -gt 0 ]] || { echo "usage: $0 <module>..." >&2; exit 2; }
for f in "$@"; do
    echo "$f"
    [[ -f "$f" ]] || { fail "no such file"; continue; }
    case "$(file -b "$f")" in
        ELF*) check_elf "$f" ;;
        Mach-O*) check_macho "$f" ;;
        PE32*) check_pe "$f" ;;
        *) fail "unknown binary format: $(file -b "$f")" ;;
    esac
done
exit "$failed"
