# Bundled Base16 schemes

These scheme files are copied unchanged from
[tinted-theming/schemes](https://github.com/tinted-theming/schemes) at revision
`1bd5ad4c2c554c9c8e288b56deab2364ac8dada0` (branch `spec-0.11`), under the MIT
license in [LICENSE](LICENSE). Each file credits its scheme's author.

Plait embeds them at build time and maps their colors onto its own roles
(`crates/plait-client/src/theme.rs`). To update, fetch the same file names at a
newer revision, record that revision here, and run `cargo test -p plait-client theme::`.
