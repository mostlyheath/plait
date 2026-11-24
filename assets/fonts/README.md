# Bundled interface fonts

Plait bundles three weights of [IBM Plex Sans](https://github.com/IBM/plex) for
the desktop interface (`Settings → Appearance → Interface font → Plex Sans`):
Regular (400), Medium (500), and SemiBold (600).

- **Source:** [IBM/plex](https://github.com/IBM/plex), OpenType (`otf`) font
  version `3.005` (Copyright 2018 IBM Corp.; designed by Mike Abbink, Paul van
  der Laan, and Pieter van Rosmalen / Bold Monday).
- **License:** SIL Open Font License, Version 1.1 with Reserved Font Name
  `"Plex"` (see [OFL.txt](OFL.txt)).
- **Modifications:** None. The three font files are unmodified original
  binaries.

| File | Weight | Version | SHA-256 |
| --- | --- | --- | --- |
| `IBMPlexSans-Regular.otf` | 400 (Regular) | 3.005 | `6b17a35a31ded2e81b3ed19e5eb532d22b9a0b5a76833b0d757a5c71ab5e0f6c` |
| `IBMPlexSans-Medium.otf` | 500 (Medium) | 3.005 | `27d25e3e08ea63df7cd1fa535c9c45aa04ba11ca75b8031b2ffb83f247601ad1` |
| `IBMPlexSans-SemiBold.otf` | 600 (SemiBold) | 3.005 | `1aff1f99f0f415632e71a4b9d43804d093e85b8954489a973f0cf1e2e24b9b04` |

The desktop embeds these faces at compile time and registers them with GPUI's
text system at startup, verifying that each weight resolves to `IBM Plex Sans`
before drawing with it.
