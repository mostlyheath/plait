# Plait artwork

This directory contains the Plait vector kit. The kit reconstructs the supplied
reference artwork. The worktree icon is a later addition drawn for Plait.
[README.txt](README.txt) records the provenance, limitations, and editing notes.

- `marks/`: ribbon-P artwork for branding; not shown inside the current app window.
- `wordmarks/`: outlined lettering that needs no font; not used in current navigation.
- `icons/`: category glyphs. GPUI uses the project, machine, thread, Git, and worktree paths
  with the UI's state colors. Do not add controls for unimplemented features.
- `app-icon/plait-app-icon.svg`: editable app-icon master.
- `app-icon/plait-app-icon-1024.png`: export used to generate the macOS icon set.
  After editing the master, export this PNG at 1024 × 1024 with transparency.
- `gallery.html` and `plait-preview.*`: artwork references, not application UI.

The desktop embeds SVGs at compile time; it does not read this directory at
runtime. The app bundle contains the compiled `.icns`, not the whole kit.
