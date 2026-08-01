# Client-side WASM/PWA migration plan

> ## ⏸ PAUSED — 1 August 2026
>
> Parked deliberately, not abandoned or blocked. Everything lives on
> **`client-side-refactor`**; `main` is untouched at v0.11.0 and the desktop app
> builds and tests clean from this branch too (`cargo check --all-targets` exits 0,
> 57 Rust tests and 110 frontend checks pass).
>
> **Why paused:** the Analytics API import is the one feature that is structurally
> worse in a browser than on the desktop, and it cannot be made equal. The desktop
> mints its own token with `reqwest`; a browser cannot (see the CORS finding below),
> so the web build needs either a server-side relay file or a token pasted daily.
> That is a real regression for the feature that matters most, and it is not
> something more work removes.
>
> **What is actually finished and working**, verified against real data in a real
> browser: content export loading, session search, CSV import, date range, daily
> stats, hour coverage, context options, session interactions, import-run
> bracketing, the Analytics API import (via relay), PWA install + offline, and
> version/update handling.
>
> **Where to pick up** — the remaining unported commands, in rough order of value:
> 1. **Flagged conversations** — 11 commands, all plain SQLite, the biggest gap.
> 2. **Export for AI** — needs a `Vec<u8>` sink instead of a `File`, plus a Blob
>    download.
> 3. **Save collection export** — trivial (Blob download); the only content-side gap.
> 4. **Delete stored days** / **Compact database** — `compact_database` needs a real
>    seam, since it reads file size via `fs::metadata`.
> 5. **Choose a database file** — arguably N/A; there is one OPFS database.
>
> Everything not ported rejects with a named "not available in the web app yet"
> message from `wasm-bridge.js`, so nothing fails silently.
>
> **Two known issues if resuming:** the app only works in one browser tab at a time
> (OPFS holds its sync handles in one worker) and a second tab fails with a cryptic
> error; and `package.json`'s electron-builder config still points at the old
> `build/icon.png`.
>
> **Three commits here are not web-specific** and improve the desktop app too, if you
> want them without the rest: `e87ed54` (settings export/import), `e427733` (emoji →
> SVG icon sprite), `55041e2` (stop `build-web.sh` rewriting `Cargo.lock`).

Branch: `client-side-refactor`. Goal: a 100% client-side PWA built from static
files, uploadable to an ordinary web host, with the SQLite database and the big
export files living on the user's own machine.

This document records what has been **verified by spike** versus what is still
assumption, because several of the load-bearing questions had non-obvious answers.

---

## Verified findings

Everything in this section was measured on this machine, not inferred.

### SQLite works in wasm, and `rusqlite` comes with it

The critical question was whether the ~8300-line DB layer ports as-is or has to
be rewritten against raw FFI. It ports as-is.

`sqlite-wasm-rs` 0.5.5 compiles `sqlite3.c` for `wasm32-unknown-unknown` and is
already API-compatible with `libsqlite3-sys` — it exports the bindgen symbols,
`ErrorCode`/`Error`, `SQLITE_STATIC`/`SQLITE_TRANSIENT`, and the `Default` impls
for the vtab structs. So `rusqlite` binds to it through a **one-line shim crate**
(`vendor/libsqlite3-sys-wasm`) applied with `[patch.crates-io]`.

Runtime smoke test (`rusqlite` 0.31, built with `wasm-pack --target nodejs`):

| Behaviour the app depends on | Result |
| --- | --- |
| SQLite version | **3.53.0** (vs 3.45 bundled natively today) |
| FTS5 `content = ''`, `contentless_delete = 1` | creates ✓ |
| `tokenize = 'unicode61 remove_diacritics 1'` — `cafe` finds `café` | 1 match ✓ |
| Column filter `{cols} : (expr)` does not leak across columns | ✓ |
| `DELETE FROM …_fts WHERE rowid = ?` on a contentless index | ✓ |
| Transaction + prepared-statement loop, 1000 rows | ✓ |
| `create_scalar_function` (the `functions` feature) | ✓ |

`-DSQLITE_ENABLE_FTS5` and `-DSQLITE_ENABLE_COLUMN_METADATA` are in the crate's
compile flags. `contentless_delete` needs 3.43+; 3.53 clears it.

Two dead ends, recorded so they are not retried:

- **Overriding `libsqlite3-sys`'s build script** (`[target.wasm32-unknown-unknown.sqlite3]`)
  fails: that crate generates its bindings *inside* the build script, so skipping
  the script also removes `bindgen.rs`.
- **Adding the `Default` impls in the shim** violates the orphan rule. The shim
  must stay a pure re-export — which is fine, because `sqlite-wasm-rs` already
  has them.

### The Analytics API only half survives the move to a browser

**This entry was wrong for most of the project, and the way it was wrong is the
lesson.** It originally concluded that both endpoints allow cross-origin calls,
based on this preflight measurement:

| Endpoint | `OPTIONS` preflight | `Allow-Origin` |
| --- | --- | --- |
| `login.microsoftonline.com/…/oauth2/token` | 200 | `*` |
| `analytics.digitalcx.com` | 204 | `*` |

**A permissive preflight does not mean the response is readable.** What matters is
the header on the *actual* response, and there the two differ — measured with the
real POST bodies, then confirmed in a real browser:

| Request | `Access-Control-Allow-Origin` on the real response | From a page |
| --- | --- | --- |
| `GET analytics.digitalcx.com/…/interactions` | `*` | **works** — a bad token returns a real `403` body |
| `POST …/oauth2/token`, `grant_type=authorization_code` | `*` | works |
| `POST …/oauth2/token`, `grant_type=client_credentials` | **absent** | **`TypeError: Failed to fetch`** |

Same URL, opposite answers by grant type. Entra ID grants CORS to the
browser-safe sign-in grant and withholds it from `client_credentials`, because
that grant carries a client secret and a browser cannot hold one safely. No
configuration changes it, and the SOP mandates that grant.

Two corollaries worth keeping:

- **CORS is enforced by the browser, never the server.** The desktop uses
  `reqwest`, where CORS does not exist, so the identical request to the identical
  host succeeds there. "But the desktop manages it" is not evidence of anything.
- **Always test the request you will actually send.** A preflight probe with the
  wrong method or grant is worse than no probe, because it produces a confident
  wrong answer.

**Consequence for the design:** the token cannot be minted in the page, so it is
supplied — by a small server-side relay (`tools/token-proxy/`, the default) or by
pasting one. Both mean the **client secret never enters the browser at all**, which
inverts the security note this entry used to carry: there is no client-secret field
in the web build, and that is stricter than the desktop's `0600` file, not weaker.

### Storage forces the database into a Web Worker

From `sqlite-wasm-vfs` 0.2's VFS matrix:

| | MemoryVFS | **SyncAccessHandlePool (OPFS)** | RelaxedIdb |
| --- | --- | --- | --- |
| Storage | RAM | **OPFS** | IndexedDB |
| Contexts | All | **Dedicated Worker only** | All |
| Full durability | ✓ | **✓** | ✗ |
| COOP/COEP required | no | **no** | no |

Three consequences:

1. The DB **must** live in a dedicated worker. Not a preference — the OPFS sync
   access handle API only exists there.
2. **No COOP/COEP headers needed**, which is what makes plain static hosting
   viable. No web-host configuration required.
3. Single connection, `SQLITE_THREADSAFE=0`, no threads. The 20+
   `tauri::async_runtime::spawn_blocking` sites collapse into "the worker is the
   serialization point".

### OPFS persistence works, and it is fast

Spiked in a real browser (release build, dedicated module worker, `localhost`).
A database written to OPFS survived a **full page reload** with its FTS index
intact:

| Measurement | 20k rows (dev build) | 110k rows (release build) |
| --- | --- | --- |
| Insert + FTS index | 1132 ms | **1394 ms** (~79k rows/sec) |
| Database size | 3.5 MB | 20.2 MB |
| Rows after reload | 20 000 | **110 000** |
| FTS `cafe` match after reload | 33 ms | **45 ms** |
| FTS → rowid join | ✓ | ✓ |
| Contentless delete on the persisted index | ✓ | ✓ (and durable across reload) |
| Cold start: wasm init + VFS install + queries | 283 ms | 302 ms |

110k interactions is the size of the real database `CLAUDE.md` benchmarks
against, so this is at scale rather than a toy. No console errors.

`crossOriginIsolated=false` throughout — **confirming no COOP/COEP headers are
needed**, so this deploys to an ordinary static host with no configuration.

### WAL is not available in OPFS — plan around it

`PRAGMA journal_mode=WAL` silently reports back `delete`. The sahpool VFS is a
single-connection rollback-journal store; WAL needs shared-memory that OPFS does
not provide.

Consequence for the import path: **`begin_import_run`'s `wal_autocheckpoint`
tuning is a no-op on wasm** and must be gated to the native target with the
reason written down, or someone will later "fix" the missing pragma. The rest of
the run-scoped design — deferred finalize, scoped summary rebuild, one FTS
`'optimize'` per run — is unaffected and is where the measured 7× actually came
from, so the architecture holds.

Also note `PRAGMA journal_mode` returning something other than what was asked is
not an error in SQLite. Any code that assumes WAL took effect should check.

### `std::time` traps on wasm — a clean compile proves nothing about runtime

The best illustration of why this migration needs runtime verification rather
than build output. `Instant::now()` and `SystemTime::now()` do not return an
error on `wasm32-unknown-unknown` — they reach an `unreachable` instruction,
which is a **trap**, not a panic. `panic = "abort"` means `catch_unwind` cannot
contain it, and a trap poisons the module instance, so recovery means reloading
the entire worker.

Probed side by side in one module:

| Call | Result |
| --- | --- |
| `std::time::Instant::now()` | **`RuntimeError: unreachable`** |
| `std::time::SystemTime::now()` | **`RuntimeError: unreachable`** |
| `clock::Instant::now()` + `elapsed()` (the seam) | OK |
| `clock::now_unix_secs()` (the seam) | OK — `1785527710`, i.e. 2026 |

The exposure was **21 sites across 7 core functions**: `open_db`,
`import_csv_into`, `finalize_import_run_into`, `purge_old`, `repair_fts_index`,
`now_iso`, `window_day_hours`. `open_db` reads the clock, so *opening a
database* would have taken the module down — and the crate compiled cleanly
either way.

`src/clock.rs` is now the only place the core reads a clock. Natively it is a
straight re-export of `std::time::Instant`, so the desktop build and all 57 tests
keep std's exact behaviour; on wasm it is a `Date::now()`-backed stand-in with the
same `now()`/`elapsed()` surface, so no call site changed.

- **`Date::now()` rather than `performance.now()`** trades monotonicity for
  needing no `web-sys` dependency and no Window-vs-WorkerGlobalScope branch. Fair
  here: every use is import/query instrumentation in whole milliseconds, nothing
  correctness depends on. A backwards clock adjustment can make an interval read
  as zero (it is clamped) but never negative.
- Millisecond resolution is real: a sub-millisecond span reports `0`.

**This is also the first confirmed member of the host seam**, and the one a
paper-designed trait would most likely have missed — the 42 commands make file
I/O, dialogs and HTTP obvious, and say nothing about the clock. It is the
evidence for doing the vertical slice before designing the abstraction.

### The real core runs in the browser, against real data

Vertical slice, release build, dedicated module worker, OPFS. Not a
reimplementation — this drives the actual `open_db`, `import_csv_from_reader` and
`get_sessions_into`, fed the real 8.4 MB portal `InteractionLog` CSV.

| | |
| --- | --- |
| `open_db` (real schema, migrations, index drops, FTS repair) | ✓ `journal_mode=delete` |
| Import, real CSV, pipe-delimited | **2445 rows in 403 ms** (rows 334, summary 25, FTS optimize 37) |
| `interactions` / `interactions_fts` | 2445 / 2445 |
| `entity_index` (lifted out of `recognition_details` JSON) | 2328 |
| `session_summary` (scoped rebuild) | 537 |
| Re-import the same file | **inserted 0, skipped 2445**, FTS still 2445 |
| Persistence across a full page reload | ✓ reopened with 2445 rows |
| Release `.wasm` size | 2.7 MB |

Searches, all through `build_session_filter_query` + `get_sessions_into`:

| Query | Result | Mode |
| --- | --- | --- |
| `openingstijden`, scope user + entities | 23 sessions, 5 ms | `fts` |
| `filter: genai` | 20 | `none` |
| `filter: low_recog`, threshold 60 | 4 | `none` |
| empty query | 287 (page of 50) | `none` |
| `belgie` → **België** | **4** | `fts` |
| `oke` → **Oké** | **117** | `fts` |
| `www.efteling.nl` | 0 | **`fts_exact`** |

- **Diacritic folding is confirmed by positive control, not by absence.** `cafe`
  and `WIJN` both returned 0, which proves nothing on its own — this dataset is
  shuttle/ticketing traffic. `belgie` matching all 4 occurrences of `België` and
  `oke` matching `Oké` is the actual evidence.
- **The punctuation term selected `fts_exact`**, so the per-OR-group exact
  re-check path activates correctly even where it finds nothing.
- **The re-import is the `Ok(1)` gate working.** `skipped: 2445` with the FTS
  count unchanged is exactly the invariant `a_duplicate_row_is_never_indexed_twice`
  pins natively.
- **287 vs 537 is not a discrepancy.** `base_conditions` always starts with
  `s.has_real_user_input = 1`; `SELECT COUNT(*) … WHERE has_real_user_input = 1`
  returns 287 too.
- **One scare that was not a bug:** the first run reported `purged: 2445`. That is
  `purge_old(conn, max_age_days.unwrap_or(90))` behaving correctly — the export is
  ~128 days old, so the default 90-day retention removed everything it had just
  inserted. Identical on desktop. It incidentally confirms `purge_old`, one of the
  seven formerly-trapping clock users, now works.

Two refactors made this possible, both behaviour-preserving (57 native tests
green after each):

- `get_sessions_into(conn, args)` extracted from the Tauri command, which was
  ~130 lines of real logic wrapped in ~10 lines of plumbing. Both hosts now call
  the same function; the command adds `spawn_blocking` and the state lock.
- `import_csv_from_reader(conn, reader, source, …)` split out of
  `import_csv_into`, which opened a path with `fs::File::open` — fine natively,
  never going to work in a browser. The path version still exists and delegates.

Dead-code warnings on the wasm target dropped 113 → 71 as the core became
reachable; the remainder are the commands not yet ported.

### Capability probes must run in the worker

The spike's main-thread probe reported `syncAccessHandle=false` while the worker
used it successfully moments later. `createSyncAccessHandle` is not exposed on
the main thread in this browser. The real feature-detect has to run **inside the
worker** and report back — a main-thread check will wrongly conclude OPFS is
unavailable and send users down a fallback path they do not need.

### The 21 MB `get_data` crossing is not a problem — but pick the right strategy

Spiked in a worker against the **real export files** (12.5 MB Articles + 6.8 MB
Dialogs + 2.7 MB Entities = 22 MB), with the payload shaped exactly as `AppData`
is today (every field an untyped `serde_json::Value`). Because the wasm module
lives in the worker, the payload makes **two** hops, and both are measured:
wasm → JS, then worker → main thread.

Clean cold-start run, release build:

| Stage | Time |
| --- | --- |
| `fetch` 22 MB | 33 ms |
| UTF-8 validate + `serde_json` parse of all three files, in wasm | 115 ms |
| Serialize to JSON string in wasm | 114 ms |
| Structured-clone the string worker → main | 16 ms |
| `JSON.parse` on the main thread | 55 ms |
| **Bytes → usable JS data, end to end** | **≈365 ms** |

Counts came back right: 3295 articles, 808 dialogs, 66 tDialogs, 40 convVars,
59 ctxVars — matching the figures in the Collections notes.

**Strategy comparison** (payload already parsed and resident in wasm; "to usable"
means the renderer can index it):

| Strategy | To usable | Notes |
| --- | --- | --- |
| **B: JSON string → `JSON.parse`** | **185–439 ms** | recommended; ≈ what Tauri IPC does today |
| A: `serde-wasm-bindgen` object graph | 412–1531 ms | **2–3.5× slower than B, consistently** |
| C: keep in wasm, hand over one page | 15–19 ms | 20–80× faster, but needs renderer changes |

**`serde-wasm-bindgen` is the slow option here, which is the opposite of the
obvious guess — do not "optimise" B into A later.** Two reasons: `JSON.parse` is
heavily optimised native code, whereas serde-wasm-bindgen constructs the object
graph property by property across the boundary (chatty interop at fine grain
inside a single call); and the resulting graph then costs **106–446 ms** to
structured-clone from worker to main thread, against **12–25 ms** for a string.

Absolute numbers vary 2–3× run to run with heap state, so treat these as ratios
plus an order of magnitude. The ranking was stable across every run.

Two further notes:

- **`Serializer::json_compatible()` is mandatory** wherever serde-wasm-bindgen
  *is* used (the smaller commands). Its default emits a JS `Map` for every serde
  map, so a `serde_json::Value::Object` would arrive as a `Map` and every
  `data.articles[0].Id` in the renderer would silently break.
- **Whether `extract_articles`/`extract_dialogs` should stop `.cloned()`-ing the
  subtree is unresolved.** A move-based variant measured anywhere from 3× faster
  to no different, dominated by allocator/GC ordering effects. Not worth changing
  on this evidence — measure it in isolation if it ever matters.

Consequence for step 3: `get_data` ports as-is with a JSON-string return. No
restructuring needed. The remaining opportunity is that `search-worker.js`
currently receives its **own** structured-clone copy of the same 22 MB — if the
wasm worker and the search worker become one, that hop disappears entirely.

### Toolchain gap

Apple clang cannot emit wasm32. `brew install llvm` (22.1.8) is installed and
verified; `.cargo/config.toml` points `cc` at it. Cargo's `[env]` yields to a
real environment variable, so CI or a wasi-sdk machine overrides it without
editing the file.

---

## Interop audit (objective 3)

Current surface: **42** `#[tauri::command]` functions, **43** `invoke()` call
sites, **109** `tauri::` references inside `lib.rs`.

The existing shape is already the right one, which is the main reason this is
tractable:

- `window.electronAPI` (`frontend/index.html:8733`) is a **single chokepoint**.
  It becomes the worker RPC layer with identical method names, so the 43 call
  sites barely change.
- `frontend/search-worker.js` already returns `Int32Array` index buffers and the
  renderer resolves only the visible page. That is exactly the anti-chatty
  pattern, already load-bearing.

Rules to hold to:

- **One message per user action, never per row.** Every existing command is
  already coarse; keep it that way.
- **Return page-sized results.** `get_sessions` returns a page today; do not let
  the port turn it into "send everything, filter in JS".
- **Transfer, don't copy.** Index buffers cross as transferable `ArrayBuffer`s.
- **Cross large payloads as a JSON string, not as a serde-wasm-bindgen object
  graph.** Measured: 2–3.5× faster, and ~20× cheaper to structured-clone out of
  the worker. `get_data`'s 22 MB costs ≈365 ms bytes-to-usable this way. Details
  and the full strategy table are in the findings above.

---

## Target architecture

One crate, two targets — **not** a workspace split. Done and building:

```
src-tauri/
  Cargo.toml             deps split into cfg(not(wasm32)) / cfg(wasm32) tables
  build.rs               returns early for wasm32 (no Tauri context to generate)
  src/
    lib.rs               the core: SQLite, search, import, AI export + all tests  [dual-target]
    tauri_host.rs        42 commands, dialogs, notify watcher, run()               [native only]
    analytics_api.rs     reqwest + native TLS                                      [native only]
    wasm.rs              wasm-bindgen exports, OPFS, worker RPC — still to write   [wasm only]
vendor/
  libsqlite3-sys-wasm/   one-line shim, applied per-invocation (see below)
frontend/                existing UI + db-worker.js + manifest + service worker
dist/                    static build output — this is what gets uploaded
```

### Why one crate rather than `crates/core` + `crates/host-wasm`

Two concrete findings pushed this, both discovered while implementing:

1. **Child modules see their parent's private items.** Moving the *host* code
   down into `src/tauri_host.rs` needed **zero** visibility changes — one
   `use super::*` reaches the whole core. Moving the *pure* code out to a
   separate crate would instead have required `pub` on ~100 items and, worse,
   carving up the four monolithic test modules (2400 lines) so each test
   travelled with the code it covers. The cheap direction is to move the host.
2. **`[patch.crates-io]` is not target-conditional.** The `libsqlite3-sys` shim
   is required for wasm and *breaks* native (it would replace the real bundled
   SQLite the 57 native tests run against). A manifest `[patch]` applies to every
   target, so it cannot live in `Cargo.toml`. **It works as a command-line
   override instead** — `--config 'patch.crates-io.libsqlite3-sys.path="…"'` —
   which is per-invocation and therefore exactly target-conditional. Verified.
   This is what removed the need for two build roots, and it is why the wasm
   build must always go through the build script rather than a bare `cargo build`.

`rusqlite` compiling on **both** targets is what makes any of this work: the bulk
of `lib.rs` is target-agnostic SQL and needed no changes at all.

**Result:** 2092 lines moved into `tauri_host.rs`, `lib.rs` down to 6219. Native
`cargo test` 57 passed / 0 failed / 4 ignored; `cargo check --bins` clean;
`cargo build --lib --target wasm32-unknown-unknown` **succeeds**.

The wasm build currently emits ~113 `never used` warnings. That is expected and
deliberately not silenced: nothing calls the core on wasm until `wasm.rs` exposes
the command surface. A blanket `allow(dead_code)` would mask genuinely dead code
later, so these stay until step 4 resolves them.

### Why dual-target rather than replacing Tauri

The 51 Rust tests across `tests`, `perf`, `fts_semantics`, `conv_search`, and
`search_perf` need bundled SQLite and a real filesystem — several are gated on
`CAI_TEST_DB` pointing at a copy of a real database. They cannot run under
`wasm-bindgen-test`. Keeping the native target keeps them running, and
`CLAUDE.md` treats them as load-bearing (`a_deferred_import_run_finalized_once_matches_a_full_rebuild`,
`an_id_search_on_a_real_database_matches_an_exhaustive_scan`, and the rest).

The native desktop app is then a free by-product, not extra work.

---

## Steps

### 1. Git safety branch — done

`client-side-refactor`, branched from a clean `main`.

### 2. WASM toolchain — done

- `rustup target add wasm32-unknown-unknown`
- `brew install llvm`, wired through `.cargo/config.toml`
- `wasm-pack` 0.15.0
- `vendor/libsqlite3-sys-wasm` + `[patch.crates-io]`, proven by the spike above

`wasm-pack --target web` suits this project: it emits an ES module + `.wasm` with
no bundler and no npm runtime, matching the existing "no bundler, no framework"
constraint. Module workers cover Chrome/Edge/Safari 15+/Firefox 114+.

### 3. Target split — done

Host code moved to `src/tauri_host.rs` behind `#[cfg(not(target_arch = "wasm32"))]`,
deps split into target tables, `build.rs` gated. No behaviour change: the move was
mechanical and the native suite proves it (57 passed / 0 failed).

### The host trait is not needed — don't add one

The plan called for a `Host` trait so both hosts implemented one contract. On
inspection every seam it would have carried is **already** solved by something
simpler, and a trait would be the "unnecessary abstraction" `CLAUDE.md` warns
against:

| Seam | Already handled by |
| --- | --- |
| Clock | `clock.rs` — a target-gated module |
| File **read** | `import_csv_from_reader(conn, reader: impl Read, …)` |
| File **write** | `write_ai_export(…, out: &mut impl Write)` |
| Progress events | Emitted by the *host*, around phases — never from the core |
| HTTP | Confined to `analytics_api`; a module port, not a core seam |
| DB handle | `thread_local` vs `Arc<Mutex<DbState>>` — entirely host-side |

Generic `impl Read` / `impl Write` parameters already give the core host-agnostic
I/O without dynamic dispatch, and the progress emits were never in the core to
begin with. So the trait would have had **no members that aren't already covered**.
The three patterns to reuse when porting the rest are: target-gated module for
ambient capabilities, generic reader/writer for I/O, and `<name>_into(conn, …)`
extraction for command bodies.

### Ported so far

`get_sessions`, `import_interactions_csv`, `get_date_range`,
`get_db_daily_stats`, `get_db_hour_coverage`, `get_context_options`,
`get_session_interactions`, `begin_import_run`, `finalize_import_run`,
`record_imported_window` — each extracted to a `*_into` core function called by
both hosts, and each verified in the browser against the real CSV:

| Command | Runtime result |
| --- | --- |
| `get_date_range` | `{"min":"2026-03-25","max":"2026-03-25"}` |
| `get_db_daily_stats` | total 2445, 1 day |
| `get_db_hour_coverage` | `hours=3584` — bits 9/10/11, i.e. 09–11 UTC |
| `get_context_options` | 150 options (Channel/web 379, DeviceOS/Android 255) |
| `get_session_interactions` | 2 rows in 1 ms |
| `begin_import_run` + `finalize_import_run` | both run, `FinalizeResult` returned |

Wasm dead-code warnings: 113 → 71 → **59** as the surface grows.

### Still to do in this step

1. **The remaining ~30 commands.** Mechanical now: `flag_session` (145 lines) and
   the ten flagged-* commands, `export_conversations_for_ai` (209 — needs a
   `Vec<u8>` sink instead of a `File`), `delete_interactions_by_dates` (75),
   `get_data` (120 — the content-export path, needs bytes from JS).
2. **`compact_database` needs a real seam.** It reads the database file size with
   `fs::metadata`, which has no wasm equivalent — the OPFS pool reports size
   instead. This is the one command that genuinely differs rather than just
   needing extraction.
3. **`analytics_api.rs`**: the OAuth + fetch logic is portable once `reqwest` drops
   `rustls-tls` for its fetch backend; `validate_csv_header` is already pure. Only
   the `AppHandle` paths are native.
4. **`cancel_session_search`** has no meaning on wasm — there is no second thread
   to interrupt. Decide whether the renderer hides the control or the export
   becomes chunked.

Feature-gate the parts that cannot cross:

| Native | wasm |
| --- | --- |
| `rusqlite` + `bundled` | `rusqlite` + `[patch]` shim + `sqlite-wasm-rs` |
| `reqwest` + `rustls-tls` | `reqwest` default features (fetch backend) — drop `rustls` |
| `notify` file watcher | **no equivalent** — see casualties |
| `spawn_blocking` | worker is the serialization point |
| native dialogs | File System Access API |
| WAL + `wal_autocheckpoint` tuning | **no-op** — `journal_mode` is `delete`; gate it native-only |

### 4. Local database and file storage

- SQLite in OPFS via the `sahpool` VFS, inside `frontend/db-worker.js`.
- Big files: `showOpenFilePicker` → stream into OPFS → Rust reads through OPFS
  sync handles in the worker. The 8 MB CSV import path stays streaming; do not
  materialise a 13 MB JSON as a JS string only to hand it back to Rust.
- The Analytics API temp-CSV mechanism maps onto an OPFS temp directory, keeping
  `import_interactions_csv` as the single import pipeline. Preserve the
  `INSERT OR IGNORE` dedupe and the sweep-on-open crash recovery.
- Retention/purge/compaction (`VACUUM`) work unchanged; OPFS reports quota, so
  surface it where the current UI shows database size.

### 5. Static bundling and PWA

- `dist/` = `index.html` + `search-worker.js` + `db-worker.js` + `pkg/` (wasm +
  JS glue) + `vendor/vis-network.min.js` + icons + `manifest.json` + `sw.js`.
- `manifest.json`: `display: standalone`, `start_url: "."`, **relative** paths so
  it works from a subdirectory, icons from the existing `build/` set.
- Service worker: precache the shell and the `.wasm`; cache-first for
  fingerprinted assets, network-first for `index.html`. The `.wasm` is the
  largest asset and the one that most needs to be cached for offline use.
- Installable on Windows and Mac via Chrome/Edge; Safari 17+ supports Add to Dock.

### 6. Build script

`./build.sh` (or `make dist`): `wasm-pack build --release --target web` → copy
frontend assets into `dist/` → stamp a cache-busting build id into `sw.js` →
print the output size. One command, no bundler.

---

## Open risks

1. ~~OPFS VFS in a real browser worker is not yet spiked.~~ **Resolved** — see
   "OPFS persistence works" above. 110k rows persisted across reload at
   ~79k rows/sec.
2. ~~`get_data`'s 21 MB crossing is unmeasured.~~ **Resolved** — ≈365 ms bytes to
   usable via a JSON string. Ports as-is; see the strategy table above.
3. **File System Access API is Chromium-only.** `showOpenFilePicker` /
   `showDirectoryPicker` do not exist in Safari or Firefox. On Mac that means
   Chrome or Edge, not Safari. Needs a stated browser requirement and a
   feature-detect with a clear message rather than a broken picker.
4. **The `notify` file watcher has no browser equivalent.** The
   `data-folder-updated` event and its live-reload behaviour cannot be ported.
   Options: drop it, or offer a manual "reload data" affordance. Dropping a
   feature is a product call, not a refactoring detail — flagged for decision.
5. **Client secret exposure**, above.
6. **Native `cargo build` must keep working** throughout the split, or the 51
   tests stop being a safety net exactly when they are most needed.
