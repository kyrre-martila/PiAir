#!/bin/sh
# Run in the native ARM64 Debian Bookworm builder, then in a clean runtime below.
set -eu
binary=${1:?binary required}
test "$(uname -m)" = aarch64
readelf -h "$binary" | grep -q 'Machine:.*AArch64'
readelf -l "$binary" | grep -q '/lib/ld-linux-aarch64.so.1'
readelf -d "$binary"
# Fail closed if a new dependency appears. Bookworm provides these runtime libs.
needed=$(readelf -d "$binary" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p' | sort)
for library in $needed; do
    case "$library" in
        libasound.so.2|libgcc_s.so.1|libm.so.6|libc.so.6|ld-linux-aarch64.so.1) ;;
        *) echo "Unexpected runtime dependency: $library" >&2; exit 1 ;;
    esac
done
printf '%s\n' "$needed" | grep -qx 'libasound.so.2'
maximum=$(readelf --version-info "$binary" | grep -o 'GLIBC_[0-9][0-9.]*' | sed s/GLIBC_// | sort -Vu | tail -1)
test -n "$maximum"
test "$(printf '%s\n' "$maximum" 2.36 | sort -V | tail -1)" = 2.36
echo "Maximum required glibc: $maximum (Bookworm ceiling: 2.36)"
ldd "$binary"
! ldd "$binary" | grep -q 'not found'
"$binary" --version
"$binary" --help >/dev/null
