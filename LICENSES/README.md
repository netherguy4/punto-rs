Static builds include libdbus through the `vendored` feature of
`libdbus-sys 0.2.7`. libdbus is used under the Academic Free License 2.1
option; its upstream authors list and complete license text are preserved
in `libdbus-AUTHORS` and `libdbus-COPYING`.

The exact vendored sources, including per-file copyright notices, are available
in https://crates.io/crates/libdbus-sys/0.2.7 (the `vendor/dbus` directory).
The Rust dependency versions are pinned in `Cargo.lock`.
