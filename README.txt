PloegToolbox 0.1.0 beta

This is an early beta. Fixes and updates will follow.

A real Rust desktop foundation for the Ploeg app family. It uses the approved
blue identity and centered generated icons. SoundPloeg, VoicePloeg and PloegShot
remain Swift applications with a shared local PloegKit package.

What works
- Discover existing compatible Mac app bundles by bundle identifier.
- Open apps; keep external installations distinct from managed copies.
- Load a local release catalog with per-app OS/CPU/version/package metadata.
- Install local build ZIPs or configured HTTPS releases with SHA-256 checking.
- Verify bundle identity, version, executable, macOS version and code signature.
- Stage updates, retain a backup until activation, and recover interrupted work.
- Reject path traversal, escaping links, unsupported packages and concurrent installs.
  Standard internal Mac framework links are preserved and validated.
- Remove only managed application bundles, preserving preferences/models/files.
- Search, installed/update views, on-demand app details and System/Light/Dark appearance.

Run
  cargo run --locked
  cargo run --locked -- --preview --light
  cargo run --locked -- --catalog /absolute/path/catalog.json

The --preview flag uses sample installation states and disables app/installer
actions. It does not save preferences or access private app content.

macOS development bundle
Build the three Swift apps with their sibling PloegKit dependency, then run
packaging/package_macos.py with --sound, --voice and --shot app-bundle paths.
The Rust binary defaults to target/debug/ploegtoolbox. The script creates
dist/PloegToolbox.app with the three locally signed build ZIPs and a real
checksum catalog in Contents/Resources. The Toolbox finds that bundled catalog.
The native app icon is assets/PloegToolbox.icns.

Install locations
macOS managed apps: ~/Applications/PloegToolbox/<App>.app.
Settings and recovery state: the OS-standard PloegToolbox application-data folder.
Existing copies under /Applications or ~/Applications are external. A separate
managed copy can be installed without modifying an external copy. Quit any
running external copy before opening a newly installed managed copy.

Trusted catalog boundary
Choose catalog files only from sources you trust. A local catalog author controls
which local build and checksum you install. Local development packages may be
ad-hoc signed. They are explicitly labeled local builds in the Toolbox.
HTTPS installs additionally require a publisher team pinned at build time through
PLOEG_PUBLISHER_TEAM, matching the release entry. A remote app must pass macOS
publisher-signature and distribution assessment. This development build has no
publisher team configured, so remote installation is disabled. Public publishing,
notarized releases and a maintained remote catalog are a separate release step.

Version 1 catalog shape
  {"schema":1,"releases":[{"app":"sound","version":"1.0.1","os":"macos",
  "arch":"aarch64","source":"packages/sound.zip","sha256":"<64 hex chars>",
  "notes":"Release notes"}]}
IDs are sound, voice and shot. CPU is aarch64, x86_64 or universal. Local source
paths are relative to the catalog directory or absolute. Remote source is HTTPS
and needs publisher_team plus the matching build-time publisher configuration.

Validation
  cargo fmt --all --check
  cargo clippy --all-targets --locked -- -D warnings
  cargo test --locked

Scope
This first delivered build is validated on the current Apple-silicon Mac. The
Rust shell is designed for desktop targets, but the three utility installers are
currently macOS-only. Windows/Linux app ports remain planned. There is no silent
auto-update, self-update, cloud login or telemetry. Updates are explicit actions.

Design and dependencies
assets/tokens.json matches the native PloegKit palette. The approved generated
PNG icons live in assets/icons. Inter is bundled with its license under
assets/fonts. The installer design draws on reviewed ToolboxCraft patterns;
PloegToolbox has independent application identity, catalog and storage and does
not change ToolboxCraft or inherit its Artcraft publisher trust.

Design refinement
The main window is a two-pane app list. No eyebrow headings, repeated section
labels, platform badges or idle status slogan. Search has an icon and balanced
padding. Use the app name or its details button to view setup, versions and
release notes, or install another copy of an external app. Status appears only
for progress or messages. The default window is 1000 by 640 points.

Opening apps
Open explicitly requests the app window on cold launch and uses the native
reopen event for running apps. Updated SoundPloeg, VoicePloeg and PloegShot
builds advertise PloegToolboxOpenVersion=1. Their settings windows are reused
and restored; opening never starts speech, recording or screen capture.
Older copies without window support show Update when the new local build is
available, including same-version builds. Updating an external installation
creates a managed copy. Quit the external copy before opening the managed one.
The catalog toolbox_open_version field defaults to zero for older catalogs;
the installer verifies that the signed app meets the advertised version.

Third-party notices for the pinned Rust dependencies and Inter are distributed
in the app resources and assets/THIRD_PARTY_NOTICES.txt.
