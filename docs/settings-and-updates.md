# Settings backup and self-update

How the Settings screen is backed up and restored, and how the app replaces its own binary.

_Split out of `CLAUDE.md`. Read this before changing anything it covers._

---

## Settings backup

Saves everything on the Settings screen to a file and restores it elsewhere.
`SETTINGS_EXPORT_SCHEMA` is 2.

**It lives behind a `Backup…` button in the Settings header, in its own
`#settingsBackupModal`.** It used to sit above the Settings tabs, where its
heading, description, two buttons, warning and status line were the first thing
on a screen that exists to change a setting — four rows of chrome in front of
the thing you opened Settings for. The modal opens *on top of* Settings rather
than replacing it (`z-index: 300` against `.modal-overlay`'s 100), so closing it
returns you to the tab you were on.

**The file deliberately contains live credentials** — context URLs, the bearer
token, and on the desktop the full Analytics API config including the client
secret. A backup that leaves you re-typing a client secret is not a backup. The
amber warning in the modal is what carries that risk and says so in those words;
the export status line repeats it. Don't quietly narrow what is exported without
changing that warning too.

**`SETTINGS_EXPORT_KEYS` is an allowlist, not a denylist**, so a localStorage key
added later defaults to *not* exported and this list stays the only place that
decision is made. `SETTINGS_EXPORT_EXCLUDED` now holds only `cm-conv-db-path`,
`cm-data-folder` and `cm-perf-debug` — paths stay out because restoring one on
another machine points the app at something that isn't there, which is a
different argument from the one that used to keep credentials out.

**Both halves run in Rust, and that is what preserves the IPC rule.** The client
secret has never crossed the bridge (`getAnalyticsConfig` returns `hasSecret`
only). Exporting it does not change that: `export_settings_backup` merges the
secret into the file *Rust* writes, and `import_settings_backup` reads it back
out and writes `analytics-api.json` before handing the renderer the rest. The
secret reaches disk, which the user asked for; it still never reaches the
renderer, which they did not.
`the_secret_is_never_in_what_the_renderer_receives` pins that, and is written so
it cannot pass vacuously.

- `backup_with_analytics` / `analytics_from_backup` are split out of the
  commands so the on-disk format is testable without a file dialog.
- **An unconfigured Analytics API adds no section at all.** A block of empty
  strings reads as "credentials were exported and they are blank", which on
  restore is worse than silence.
- **An unreadable `analyticsApi` section is an error, not a skip** — otherwise a
  truncated file restores the localStorage half and silently leaves stale
  credentials behind it.
- The exported file is `0600` on unix, matching `analytics-api.json`. It holds
  the same secret and usually lands in a Downloads folder.

**On the desktop the confirm comes *before* the file picker**, unlike the web
path. Rust writes `analytics-api.json` as soon as it reads the file, so a
confirm afterwards could be declined with the credentials already replaced — a
half-restore with no way back. Cancelling the picker cancels everything, which
is the guarantee that actually matters. The web build has no credentials file
and keeps the original parse-then-confirm order.

**Importing can only ever write keys this build knows about**, so a crafted or
newer file cannot set arbitrary localStorage entries. `_applyImportedSettings`
is the single place that filter lives, shared by both paths, and
`frontend/tests/settings-backup.test.js` covers it along with the
include/exclude split.

## Self-update

The app replaces itself in place. The **portable Windows `.exe` is the case that
matters**: an installer needs privileges some users' IT policy withholds, so the
portable build is the primary distribution and it has to be able to update
without one.

`tauri-plugin-updater` is used for **check, download and signature
verification only**. Its install step can only drive an installer on Windows
(NSIS or MSI), so the install half is ours — `src-tauri/src/self_update.rs`.

**Windows will not let you write to or delete a running `.exe`, but it will let
you rename it.** The loader holds the image with `FILE_SHARE_DELETE` and a
rename only rewrites a directory entry. That single fact is the whole mechanism:

```text
write   <dir>/CAIDashboard.exe.new      (verified bytes, same directory)
rename  CAIDashboard.exe      -> .old   (allowed while running)
rename  CAIDashboard.exe.new  -> CAIDashboard.exe
spawn CAIDashboard.exe; app.exit(0)
next launch deletes *.exe.old
```

- **Everything happens in the app's own directory**, so both renames are
  same-volume and atomic, and the first is undoable — if the second fails,
  `.old` goes back and the user is exactly where they started. The error message
  for the case where even *that* fails names the backup file and how to rename
  it by hand.
- **The old binary is deleted on a later launch, never at the end of the
  update.** Keeping it until the new one has actually run is what makes a bad
  release recoverable. `cleanup_stale_backups` retries on a background thread
  because this process was spawned by the one it is cleaning up after, and
  Windows only releases the image once that has fully exited.
- **`is_backup_name` deliberately does not match a bare `*.old`** — that is a
  common enough suffix for a user's own files, and a portable app shares its
  folder with them.
- **`apply_portable_zip` must never be handed unverified bytes**: what it writes
  becomes the application on the next launch. It isn't — `Update::download`
  verifies the minisign signature against the `pubkey` in `tauri.conf.json`
  *before* returning, which is exactly why the plugin is kept for that half. It
  also re-checks the `MZ` magic, so a wrong or truncated asset fails before the
  working binary is moved aside.

**Portable vs managed is decided by `uninstall.exe` sitting beside the binary**,
which the Tauri NSIS template always writes and a portable zip never contains.
Not the registry: this is a property of the directory we would actually modify,
so someone who copies an installed exe onto a USB stick is portable from that
point on. The check is biased toward `Portable` on purpose — misreading a
portable copy as managed runs an installer it cannot use, while the reverse just
swaps the binary in place, which works.

**`canSelfUpdate` is answered before an update is offered**, so the UI never
shows a button that cannot finish. A portable copy in a folder it cannot write
to (a read-only share, Controlled Folder Access) and any debug build both report
`false` with a reason, and the modal shows the manual download instead.
`exe_dir_writable` probes with a real file — permission bits do not account for
any of those cases.

**An unexpected property worth keeping**: files written by our own Rust code get
no Mark-of-the-Web, because the Zone.Identifier stream is applied by browsers
and the Attachment Manager, not by `fs::write`. A self-updated exe therefore
launches without the SmartScreen prompt that a manually downloaded one shows.

### Release plumbing

- **`latest.json` needs a `windows-portable` entry that `tauri-action` does not
  write**, because the portable zip is built after it runs and is not one of its
  bundles. The `finalize-release` job in `.github/workflows/release.yml` adds it.
- **The release is created as a draft and published only by `finalize-release`.**
  A draft is not served by `releases/latest/download/latest.json`, so any way a
  run can fail leaves clients on the previous version instead of on a release
  that only works for some of them. Before flipping the draft off, that job
  checks three things, each of which has actually shipped broken:
  - `latest.json`'s version matches the tag — a stale manifest hands every
    client the wrong artifact while looking healthy.
  - every target a client can ask for is present (`windows-portable`,
    `darwin-aarch64`; add `windows-x86_64-nsis` if the installer returns). **A
    missing platform key is not an error to the updater, it is an answer** — the
    plugin returns `Ok(None)` and `check_for_updates` maps that to
    `"up-to-date"`, so an absent key reads as "no update" forever.
  - every URL in the manifest names an asset actually on the release, and
    carries a non-empty signature. A key pointing at an artifact that was never
    uploaded fails at download time rather than at check time.
- **Only the macOS job runs `tauri-action`, and only it writes `latest.json`.**
  `max-parallel: 1` is therefore no longer about a read-modify-write race — it
  is about order: the Windows job uploads into a release that has to exist
  already, with the body and draft flag the macOS job sets.
- **`tauri-action` cannot be used for the Windows job.** It builds with
  `--no-bundle` and so produces no bundles, and `tauri-action` treats nothing to
  upload as a failure — `##[error]No artifacts were found.` after a clean
  seven-minute compile. Windows runs `npm run tauri build -- --no-bundle`
  directly and uploads with `gh release upload`, which targets the existing
  draft rather than creating a release of its own. This is what half-published
  v0.13.1: the Windows job died, `finalize` was skipped by `needs: build`, and
  the release went out with only the `darwin-*` keys.
- **The portable zip is signed by its own step** (`npx tauri signer sign`) with
  the same key, verified by the same pubkey. `TAURI_SIGNING_PRIVATE_KEY` /
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` are repo secrets; **losing that key means
  no further updates can ever be shipped**, to any installed copy.
- **A local `tauri build` always ends non-zero, and that is expected.** The
  bundles are finished first — a working `.app` and `.dmg` are on disk — and the
  build then fails signing the updater artifacts, because `createUpdaterArtifacts`
  plus the `pubkey` in `tauri.conf.json` require a private key that only CI has:
  `A public key has been found, but no private key.` Nothing is wrong locally.
  - **Do not read a `bundle_dmg.sh` failure as the cause.** Tauri swallows that
    script's output and reports `failed to run bundle_dmg.sh` for any non-zero
    exit, which points at DMG bundling when the real error is further down. Run
    `npm run tauri build -- --verbose` to see what the script actually printed.
  - The DMG step is separately a bit flaky: create-dmg drives Finder over
    AppleScript to position the window icons, and interrupted runs leave
    `rw.*.dmg` staging files behind. If it recurs, clear
    `src-tauri/target/release/bundle/macos/rw.*.dmg` and check nothing is left
    mounted under `/Volumes`.
- Requires no new capability grant: the renderer goes through our own commands,
  not the plugin's JS API.
- **`bundle.targets` must include `app`, and that is for the updater, not for
  distribution.** `dmg` is not an updater-enabled target, so without `app` the
  macOS build produces `.app.tar.gz` and its `.sig` and then discards them —
  `Warn The bundler was configured to create updater artifacts but no
  updater-enabled targets were built`, then `Signature not found for the updater
  JSON. Skipping upload...`. Both jobs still go green and the release still
  publishes; the only symptom is a `latest.json` with no `darwin-*` key, so every
  Mac silently reports itself up to date forever. That is what shipped in v0.12.0.
- **`bundle.targets` is macOS-only (`["dmg", "app"]`) and Windows builds with
  `--no-bundle`.** Windows ships one artifact — the portable zip, built from the
  plain `.exe` by the workflow itself. The NSIS installer was dropped once the
  portable build could update itself: its only real advantage was managed
  updates, it was blocked by the IT policy the portable build exists to work
  around, and across six releases it was downloaded once, by the author, testing.
  - **If it ever comes back, `latest.json` must keep a `windows-x86_64-nsis`
    key.** An installed copy asks for exactly that key, and a missing one is the
    same silent "up to date forever" failure as the macOS bug above. Pointing
    installed copies at `windows-portable` also works — the swap is fine inside
    an NSIS `currentUser` install under `%LOCALAPPDATA%`, it just leaves a stale
    version in Add/Remove Programs.
- **The manifest check is now the `finalize-release` gate, not a manual step.**
  Both silent failures above (v0.12.0's missing `darwin-*`, v0.13.1's missing
  `windows-portable`) would fail that job while the release was still a draft.
  To check by hand anyway:
  `gh release download <tag> --pattern latest.json -O - | jq '.platforms|keys'`
- **A red release run means no release went out**, which is the point — but it
  also means the draft is still sitting there. Fix the cause and re-tag; delete
  the stale draft so it can't be published by hand later.
