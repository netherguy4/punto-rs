#!/bin/sh
set -eu
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
binary=${1:-"$source_dir/target/debug/punto-rs"}
destination=$(mktemp -d)
trap 'rm -rf "$destination"' EXIT HUP INT TERM
DESTDIR="$destination" sh "$source_dir/scripts/install.sh" "$binary"
"$destination/usr/local/bin/punto-rs" --check-config -c "$destination/etc/punto-rs/config.conf"
printf '\n# Local setting retained on upgrade\n' >> "$destination/etc/punto-rs/config.conf"
cp "$destination/etc/punto-rs/config.conf" "$destination/expected.conf"
DESTDIR="$destination" sh "$source_dir/scripts/install.sh" "$binary"
cmp "$destination/expected.conf" "$destination/etc/punto-rs/config.conf"
cmp "$source_dir/config/punto-rs.conf" "$destination/etc/punto-rs/config.conf.example"
cmp "$source_dir/systemd/punto-rs.service" "$destination/etc/systemd/system/punto-rs.service"
echo 'PASS: installation and update preserve local config'
