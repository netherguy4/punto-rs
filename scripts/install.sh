#!/bin/sh
set -eu

source_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
binary=${1:-"$source_dir/punto-rs"}
destination=${DESTDIR:-}

if [ ! -f "$binary" ]; then
    echo "Binary not found: $binary" >&2
    exit 1
fi

install -d "$destination/usr/local/bin" "$destination/etc/punto-rs" "$destination/etc/systemd/system"
install -m644 "$source_dir/config/punto-rs.conf" "$destination/etc/punto-rs/config.conf.example"
if [ ! -e "$destination/etc/punto-rs/config.conf" ] && [ ! -L "$destination/etc/punto-rs/config.conf" ]; then
    install -m644 "$source_dir/config/punto-rs.conf" "$destination/etc/punto-rs/config.conf"
fi
install -m644 "$source_dir/systemd/punto-rs.service" "$destination/etc/systemd/system/punto-rs.service"
# Замена inode позволяет обновлять файл, пока предыдущий бинарник ещё выполняется.
temporary=$(mktemp "$destination/usr/local/bin/.punto-rs.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
install -m755 "$binary" "$temporary"
mv -f "$temporary" "$destination/usr/local/bin/punto-rs"
echo 'Files installed; existing config preserved. Service state has not been changed.'
