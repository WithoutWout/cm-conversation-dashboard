# Client-side WASM/PWA migration plan

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

### The Analytics API survives the move to a browser

This was expected to be the feature that could not work client-side. It can.
Both endpoints answer CORS preflight with `Access-Control-Allow-Origin: *`:

| Endpoint | Preflight | `Allow-Origin` |
| --- | --- | --- |
| `login.microsoftonline.com/digitalcx.onmicrosoft.com/oauth2/token` | 200 | `*`, `POST` + `content-type` allowed |
| `analytics.digitalcx.com` | 204 | `*`, `GET` + `authorization` allowed |

So the OAuth2 client-credentials flow and the interactions fetch both run from
the page with no proxy and no server.

**One deliberate security downgrade to surface in the UI:** the client secret
moves from `app_data_dir()/analytics-api.json` at `0600` to browser-origin
storage. It is no longer protected by file permissions, and any script running on
the app's origin can read it. The current design's promise that "the client secret
never crosses the IPC bridge" cannot be kept in a browser — say so in Settings
rather than changing it silently.

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

```
crates/
  core/                  pure logic + SQL + rusqlite — no tauri, no notify   [dual-target]
  host-wasm/             wasm-bindgen exports, OPFS, worker RPC              [wasm only]
vendor/
  libsqlite3-sys-wasm/   one-line shim, applied via [patch.crates-io]
src-tauri/               existing desktop host, depends on crates/core       [native only]
frontend/                existing UI + db-worker.js + manifest + service worker
dist/                    static build output — this is what gets uploaded
```

`rusqlite` compiling on **both** targets is what makes the split cheap: the bulk
of `lib.rs` is target-agnostic SQL and moves into `crates/core` unchanged. Only
the host seam needs adapting.

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

### 3. Crate split

1. Create the workspace; move `lib.rs`'s SQL/logic into `crates/core` with no
   behaviour change. Land this with the 51 tests passing natively — that is the
   proof the move was mechanical.
2. Define a host trait for the seam: file read/write, folder pick, save dialog,
   HTTP, progress events, clock.
3. `src-tauri` implements it with what it does today; `host-wasm` implements it
   with OPFS + File System Access API + `fetch` + `postMessage`.

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
