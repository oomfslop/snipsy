#!/bin/sh
# Fetches the crates listed in Cargo.toml's [patch.crates-io] into vendor/ and applies patches/.
set -eu
cd "$(dirname "$0")/.."

fetch() {
    name=$1 version=$2 checksum=$3
    directory="vendor/$name-$version"
    archive="vendor/$name-$version.crate"
    mkdir -p vendor
    curl -fsSL "https://static.crates.io/crates/$name/$name-$version.crate" -o "$archive"
    echo "$checksum  $archive" | shasum -a 256 -c - >/dev/null
    rm -rf "$directory"
    tar -xzf "$archive" -C vendor
    rm "$archive"
    patch -s -d "$directory" -p1 <"patches/$name.patch"
}

fetch gpui-pre-macos 0.3.9 c7c988b7e49775cc8542dbe48d5a10e073d8e898a023ef3ff3d24f33363e9863
