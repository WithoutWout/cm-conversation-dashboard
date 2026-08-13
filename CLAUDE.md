# CLAUDE.md

## Project overview

Tauri desktop dashboard for inspecting and navigating CM.com Conversational AI Cloud content exports. Reads two JSON files from a user-selected folder and renders a searchable, filterable UI in a single window.

**Stack:** Tauri v2 (Rust backend + vanilla JS frontend), vanilla JS, no bundler, no framework.

Keep changes simple, scoped, and in line with the current architecture. Avoid unnecessary abstraction or complexity.

Libraries may be used, but must be vendored locally (e.g. `frontend/vendor/`) so the app works fully offline. Never load dependencies from a CDN.

**CM.com Analytics API:** `CM_Analytics_API_SOP.md` (gitignored, local-only) is the single source of truth for the Analytics API — OAuth2 token generation, the interactions endpoint, and its limits. The client lives in `src-tauri/src/analytics_api.rs`; consult the SOP before changing it. See `## Analytics API import` below.

---

## File structure

```
src-tauri/
  src/
    lib.rs          — Tauri commands for content data, links, updates, and Conversations DB features
    main.rs         — Entry point, calls lib::run()
    analytics_api.rs — Analytics API client: config storage, OAuth2 token cache,
                       one-request-at-a-time fetch, CSV validation, temp files
    self_update.rs  — Portable-exe self-update: install-kind detection, the
                      rename swap and its rollback, stale-backup cleanup
  tauri.conf.json   — App config, window setup, frontendDist: ../frontend, updater pubkey
  Cargo.toml        — Rust dependencies (tauri, serde, reqwest, notify, tauri-plugin-opener, tauri-plugin-dialog, tauri-plugin-updater)
  capabilities/
    default.json    — Capability grants: core:default, opener:default, dialog:default
frontend/
  index.html        — Entire renderer: HTML + embedded <style> + embedded <script>
  search-worker.js  — Worker-side content/entity filtering, sorting, and search matching
  tests/
    extract.js      — pulls named functions out of index.html so tests run the real source
    collections.test.js, export-integrity.test.js,
    conv-search.test.js, update-modal.test.js,
    settings-backup.test.js, metadata-filter.test.js  — `npm run test:frontend`
package.json        — scripts: tauri dev / tauri build / test:frontend
```

Data files (read-only, never committed, placed in a user-selected folder):

- `*ArticlesExport*.json` — matched by pattern `"ArticlesExport"`
- `*DialogsExport*.json` — matched by pattern `"DialogsExport"`
- `*EntitiesExport*.csv` — matched when present for Entities enrichment/search

---

## Tauri security rules

- The renderer has no direct Node or filesystem access — all backend calls go through Tauri commands via `window.__TAURI__.core.invoke()`.
- `withGlobalTauri: true` is set in `tauri.conf.json`, making `window.__TAURI__` available.
- `open_url` only calls `opener::open_url` after validating the URL starts with `https://` or `http://`.
- Never add new Tauri commands without validating input on the Rust side.
- Keep capability grants in `capabilities/default.json` minimal.

---

## Tauri commands (Rust → JS)

| Command               | JS call via `window.electronAPI` | Description |
| --------------------- | -------------------------------- | ----------- |
| `get_data`            | `getData(selectedFolder)`        | Returns content data: articles, dialogs, tDialogs, entities, conversation/context vars, files, sourceFiles, dataSource |
| `open_url`            | `openUrl(url)`                   | Opens a URL with `opener::open_url` (https/http only) |
| `open_preview_window` | `openPreviewWindow(url)`         | Opens a validated URL in an in-app preview window |
| `select_data_folder`  | `selectDataFolder()`             | Opens a native folder picker, returns `{ ok, canceled, path }` |
| `check_for_updates`   | `checkForUpdates()`              | Reads the release's `latest.json` through `tauri-plugin-updater`, returns `{ status, version, message, notes, canSelfUpdate, mode, blockedReason }`. See `## Self-update` |
| `install_update`      | `installUpdate()`                | Downloads the verified artifact, installs it, restarts. Portable Windows copies swap their own `.exe`; everything else uses the plugin's installer |
| `get_version`         | `getVersion()`                   | Returns the app version string from `package_info()` |
| `save_collection_export` | `saveCollectionExport(defaultName, content)` | Opens a native Save dialog (`.json` filter, defaulted filename) and writes `content` to the chosen path, returns `{ ok, canceled, path }` |
| `export_settings_backup` | `exportSettingsBackup(defaultName, payload)` | Merges the Analytics API credentials into the renderer's payload and writes the backup (`0600`). See `## Settings backup` |
| `import_settings_backup` | `importSettingsBackup()`          | Picks a backup, restores the Analytics API credentials from it, returns everything else — `{ ok, canceled, settings, appVersion, schemaVersion, analyticsRestored }` |

| `get_analytics_config`     | `getAnalyticsConfig()`            | Analytics API settings **without the client secret** — returns `hasSecret` only |
| `save_analytics_config`    | `saveAnalyticsConfig(args)`       | Writes `analytics-api.json` to the app data dir (`0600`); a blank secret keeps the stored one |
| `test_analytics_connection`| `testAnalyticsConnection()`       | Requests an OAuth2 token only, returns `{ ok, message, trace }` |
| `fetch_analytics_window`   | `fetchAnalyticsWindow(startUtc, endUtc)` | Downloads one window to a temp CSV, returns `{ tempPath, delimiter, rowCount, bytes, durationMs, trace }`; rejects with `{ kind, message, retryable, trace }`. The `trace` is a step-by-step account carried on both outcomes — see `### Diagnosing a failed import` |
| `cleanup_analytics_temp`   | `cleanupAnalyticsTemp(paths?)`    | Deletes the given temp CSVs, or sweeps the whole temp dir when called with no argument |
| `get_db_hour_coverage`     | `getDbHourCoverage(sinceDate?)`   | Per UTC day, a bitmask of the 24 hours the day is **covered** for — the union of hours holding interactions and hours an API window explicitly requested. Distinguishes a partially imported day from a complete one. `sinceDate` bounds an otherwise full-table aggregate against `idx_timestamp`; the Import modal passes the retention floor, Manage Database omits it because its calendar browses everything stored |
| `record_imported_window`   | `recordImportedWindow(startUtc, endUtc)` | Marks every UTC hour a successfully imported API window covered. Called once per downloaded window, *after* its rows are in. See `## Coverage: asked-for vs present` |
| `begin_import_run`         | `beginImportRun()`                | Opens an import run: resets the touched-session set, sets the `pending_finalize` crash marker, raises `wal_autocheckpoint` |
| `finalize_import_run`      | `finalizeImportRun(maxAgeDays)`   | Closes a run: purge, scoped summary rebuild, FTS merge, planner stats, WAL restore — once, instead of once per file. Safe no-op when no run is open |
| `compact_database`         | `compactDatabase()`               | `VACUUM`s the database, returning pages freed by deletions and schema migrations to the filesystem. Returns `{ bytesBefore, bytesAfter, durationMs }` |

There are also Conversations DB commands exposed through `window.electronAPI` for importing CSV interaction logs, selecting/opening a SQLite database, searching sessions, loading chat interactions, context and metadata options (`get_context_options` / `get_metadata_options`, both thin wrappers over `tag_options`), daily stats, deleting imported dates, and managing flagged conversations/folders. Keep conversation search separate from content search.

`import_interactions_csv(filePath, maxAgeDays, delimiter?, deferFinalize?)` takes an optional single-character `delimiter`, defaulting to `|` (the portal export format). The Analytics API path sniffs the delimiter from the response header and passes it through; the manual path omits it. `deferFinalize` defaults to false; both real callers pass `true` and bracket their loop with `begin_import_run` / `finalize_import_run` — see `## Why import stays fast as the database grows`.

### Export for AI (`export_conversations_for_ai`)

The Conversations toolbar's **Export for AI** button writes the *entire current search result set* (not just the visible page — the export SQL has no `LIMIT`) to a `.jsonl` file, one JSON object per session, for pasting into an LLM or analysing with a script.

- Takes the same `GetSessionsArgs` as `get_sessions`, so the export is exactly what the user is looking at. The renderer reuses `lastConvSearchArgs`.
- **Schema v4 is header + body, and that split is the point.** `write_ai_export` emits one `record_type: "export_header"` line (`schema_version`, `exported_at`, `session_count`, `search_context`, `legend`) followed by one `record_type: "session"` line per conversation (`session_index`, `session`, `feedback_targets`, `chat_trace`). Everything constant lives in the header — v3 repeated `schema_version`/`exported_at`/the whole `search_context` on every line, which on a few thousand sessions is a lot of tokens spent saying the same thing, and it invited a model to over-weight the filter metadata. Don't move them back.
  - `session_count` in the header + contiguous `session_index` from 0 is how a reader detects a truncated file. `session_index: 0` survives `prune_empty_json` because numbers are never "empty" — don't switch it to a string.
  - The `legend` documents every field a cold reader would otherwise guess at (the `recognition_quality` 0–100 scale, what `dialogs[].status: "End"` means, how certain a `target_resolution` is, that missing keys mean "no value"). It costs ~200 tokens once. If you add a field to a turn, add a legend line for it.
- `chat_trace` is `compact_turn` per interaction in `log_id` order; `feedback_targets` pre-resolves each thumbs up/down to the answer it was about (via `originatingInteractionId`, falling back to the previous bot output — `feedback.target_resolution` records which, so a reader knows whether the join is certain or inferred).
- **The compaction is deliberate.** `compact_turn`/`compact_triggered_content`/`compact_entity_matches` keep only the fields that explain *why* an answer was given (recognition type/quality, triggered articles + dialog nodes, entity matches) and drop raw passthrough noise (`contexts`, `pages`, `faqsFound`, raw `articles`/`recognitionDetails`). `strip_html_text` flattens answer HTML and `prune_empty_json` removes empty keys, so the file carries signal rather than schema. `compact_turn_keeps_fix_signals_and_drops_noisy_raw_fields` pins this — don't reintroduce raw columns.
- `is_feedback_target` is written **only when true**. It used to be the one field exempt from pruning, so `false` appeared on every turn to convey nothing — and `feedback_targets` already enumerates them.
- **`turn_kind`, not `role`.** A CAI row usually holds a question *and* its answer, so the old `role: "user" | "assistant" | "turn"` looked like the chat-message convention while mostly meaning "both". Values are self-describing: `user_and_bot` (the common case) / `user_only` / `bot_only` / `feedback` / `system`.
- **Exported timestamps carry an explicit `Z`** via `utc_iso`. The DB stores naive UTC by design (see the import notes above), but an unmarked `2026-03-25T09:30:22` reads as local time to anything consuming the file. `now_iso` already marks itself.
- **The wait is reported, not just disabled.** The whole export is one long `invoke`, and on a few thousand conversations it runs for seconds with nothing on screen moving — a greyed-out button does not distinguish "working" from "stuck". `AI_EXPORT_PROGRESS_EVENT` marks the two phase boundaries and the renderer turns them into a `.import-toast.progress` — a **non-dismissing** toast with a spinner, since a toast that disappears after six seconds while the work continues is exactly the "did it stop?" question it exists to answer. `clearProgressToast()` removes only that class, so it is safe in a `finally` that runs after a result toast was already shown.
  - The toast is raised **by the events, not by the click**, so nothing appears while the save dialog is still waiting on the user — at that point no work has started. The button's own spinner covers that stretch.
- **Order is: save dialog → query → size warning → write.** The save dialog must stay first so it opens instantly. `suggested_ai_export_name` is the search-term slug alone (`openingstijden.jsonl`, falling back to `conversation-analysis-export.jsonl` for an empty or punctuation-only query) — deliberately derived from nothing but `args`. An earlier version appended the most common `first_user_message`, which forced the full result query to run *before* the dialog and left the user staring at a spinner where they expected an immediate dialog; don't reintroduce a name that depends on the result set.
- Above `AI_EXPORT_LARGE_TOKENS` (from the summed `interaction_count` × `AI_EXPORT_EST_BYTES_PER_TURN`) the user gets a confirm dialog after the query but before the write, because a search can easily match more than any model can read. That pre-estimate is deliberately crude and labelled "very roughly"; the toast afterwards reports **actual** bytes and `estimatedTokens` from the real file size. Query and write are separate `spawn_blocking` phases so the warning can sit between them — the write phase re-locks the DB.
- `search_context` is built once outside the write loop — it is identical for the whole export, so don't rebuild (and re-clone every arg) per session.
- **The output file must stay wrapped in a `BufWriter`.** `serde_json::to_writer` emits many small writes per record; against an unbuffered `fs::File` each one is a syscall. Measured on 5k sessions / 40k turns: 8.5 s unbuffered vs 2.4 s buffered — ~73% of the export was syscall overhead. The remaining ~2.4 s is the per-session query and JSON build, which is the floor without restructuring the read.
- `write_ai_export` is split out of the command and takes `&mut impl Write` specifically so the on-disk format is testable without a save dialog. `exported_jsonl_puts_constants_in_a_header_and_never_repeats_them` asserts the real bytes: header first, no constant repeated on a session line, contiguous indexes, `Z`-marked timestamps, no `is_feedback_target: false`, no `role`.

## Events (Rust → renderer)

| Event                 | Payload              | Description |
| --------------------- | -------------------- | ----------- |
| `data-folder-updated` | `{ reason, folder }` | Emitted by `notify` file watcher when export files change |
| `ai-export-progress`  | `{ phase, sessionCount?, interactionCount? }` | Phase boundaries inside `export_conversations_for_ai` — `"querying"` once the save dialog is answered, `"writing"` once the result set is known |
| `update-progress`     | `{ phase, downloaded, total? }` | Download/install progress for `install_update`. `total` is absent when the server sends no `Content-Length` |

---

## Frontend bridge (`index.html`)

The renderer uses `window.electronAPI` as its sole interface to the backend. At startup, a shim in `index.html` wraps Tauri's `invoke` behind `window.electronAPI`:

```js
const invoke = window.__TAURI__?.core?.invoke
const listen = window.__TAURI__?.event?.listen
window.electronAPI = {
  getData: (selectedFolder) =>
    invoke("get_data", { args: { selected_folder: selectedFolder || null } }),
  openUrl: (url) => invoke("open_url", { url }),
  openPreviewWindow: (url) => invoke("open_preview_window", { url }),
  selectDataFolder: () => invoke("select_data_folder"),
  onDataFolderUpdated: (handler) =>
    listen ? listen("data-folder-updated", handler) : Promise.resolve(() => {}),
  checkForUpdates: () => invoke("check_for_updates"),
  getVersion: () => invoke("get_version"),
  saveCollectionExport: (defaultName, content) =>
    invoke("save_collection_export", { defaultName, content }),
  fetchAnalyticsWindow: (startUtc, endUtc) =>
    invoke("fetch_analytics_window", { args: { startUtc, endUtc } }),
  // Conversations DB and Analytics API commands are also mapped here; keep them behind
  // window.electronAPI rather than adding direct renderer filesystem access.
}
```

---

## Data schemas

### ArticlesExport (`data.articles[]`)

```js
{
  Id: number,
  Culture: string,
  Questions: [{ Text: string, IsFaq: boolean }],
  Outputs: [{
    Type: "Answer" | "DialogStart" | "TDialogStart",
    Text: string,           // present on Answer
    DialogId: number,       // present on DialogStart
    TDialogId: number,      // present on TDialogStart
    DialogStartNodeId: number,
    Links: [],
    Images: [],
    Videos: []
  }],
  Categories: []
}
```

### DialogsExport (`data.dialogs[]`)

```js
{
  id: number,
  name: string,
  description: string,
  versionId: string,
  nodes: [{
    id: number,
    type: "Recognition" | "Output",
    name: string,
    output: { items: [{ type: "Answer" | "DialogStart" | "TDialogStart", data: { text, dialogId, tDialogId, entryPointId } }] },
    links: [{ childNodeId: number, condition: { data: { questions: [{ text }], isFallback: boolean } } }]
  }]
}
```

### tDialogs (`data.tDialogs[]`)

```js
{ id: number, name: string }
```

~66 items total. These are **Transactional Dialogs** — always use this term in the UI; never "Transfer Dialog".

---

## CM.com URL patterns

Base URL constant in `index.html`:

```js
const CM_DEFAULT_URL = ""
```

Deep-link patterns:

- Article: `{baseUrl}/articles/{id}`
- Dialog: `{baseUrl}/dialogs/{id}`
- Dialog node: `{baseUrl}/dialogs/{dialogId}?currentNode={nodeId}`

`cmBaseUrl` is read from `localStorage["cm-base-url"]` or falls back to `CM_DEFAULT_URL` (empty string). CM.com links are only rendered when a context URL has been configured in Settings.

---

## Renderer architecture (`index.html`)

Single `<script>` block at the bottom. No modules. Most renderer state is module-level `let`. Content/entity filtering and sorting run in `frontend/search-worker.js`; the renderer receives Int32Array index buffers and resolves only the visible page of items.

### State variables

```js
let gQuery = ""                    // current search query string
let searchCase = false             // Aa toggle
let searchWord = false             // \b toggle
let searchRegex = false            // .* toggle
let searchContent = false          // ¬T toggle — when true, search responses only
let searchExcludeNonDefault = false // ND toggle — excludes non-default response matches only when a query is active
let allFilterPill = "all"          // filter in All Results tab
let aFilter = "all"                // filter in Articles tab
let dFilter = "all"                // filter in Dialogs tab
let allSort, aSort, dSort          // persisted content sort choices
let allPage, aPage, dPage          // current pagination pages
let allArticles = []               // raw article data
let allDialogsCombined = []        // dialogs only (no tDialogs)
let allCombinedItems = []          // articles + dialogs + tDialogs merged (each with _kind)
let filteredAll, filteredArticles, filteredDialogs // Int32Array result indexes
let allEntities = []
let filteredEntities               // Int32Array entity result indexes
let matchingEntityNames = new Set() // entity names matched by current search query
let dialogMap = new Map()          // id → dialog object
let tDialogMap = new Map()         // id → tDialog object
let articleMap = new Map()         // id → article object
let cmBaseUrl                      // CM.com context URL
let haloBaseUrl                    // HALO/other context URL for conversation links
let openMode                       // "popup" | "browser"
let otherOpenMode                  // "popup" | "browser"
let collectionSelectMode = false   // Content-tab multi-select toggle
let collectionSelection = new Set() // stable keys: "article:<Id>" | "dialog:<id>"
let cmCollections = loadCollections()      // in-memory mirror of localStorage "cm-collections"
let cmExportFilters = loadExportFilters()  // in-memory mirror of localStorage "cm-export-filters"
```

### Key functions

| Function                          | Purpose |
| --------------------------------- | ------- |
| `buildSearchRegex(q)`             | Renderer highlight regex builder; worker has the authoritative search compiler |
| `hl(text, q)`                     | HTML-escapes text and wraps matches in `<mark>` |
| `esc(s)`                          | HTML-escapes a string (use for all dynamic content inserted into innerHTML) |
| `strip(t)`                        | Strips HTML tags from text |
| `aKind(a)`                        | Returns `"dialog"`, `"tdialog"`, or `"plain"` for an article based on Outputs |
| `triggerSearch()`                 | Sends the current query, filters, toggles, context filters, and sort choices to `search-worker.js` |
| `handleSearchResults(msg)`        | Receives worker result index arrays, updates counts/pagination, and lazily renders the active tab |
| `cmLink(type, id)`                | Returns an `<a class="action-link">` HTML string; `type` is `"article"` or `"dialog"` |
| `articleDialogLinkBadges(links)`  | Renders clickable Dialog Link/Transactional Dialog chips for article cards |
| `dialogLinkedArticles(item)`      | Finds Articles that link to a Dialog for card/export relationship displays |
| `renderArticleCard(art, q)`       | Full article card HTML with badges, expandable questions, output section |
| `_applyResponseClamps(container)` | Adds the fade + "Show full response" toggle to any overflowing `.response-box` in an info modal |
| `_linkedFromSectionHtml(kind, id)`| "Linked from Articles" — which Articles route into this Dialog / Transactional Dialog |
| `renderDialogCard(item, q)`       | Full dialog/tDialog card HTML with expandable node list |
| `renderNodeHtml(node, dialog, q)` | Individual node HTML: Recognition/Output badge, answer, user options, routing |
| `applyAllFilters()`               | Lightweight wrapper that triggers worker search for All Results |
| `applyArticleFilters()`           | Lightweight wrapper that triggers worker search for Articles |
| `applyDialogFilters()`            | Lightweight wrapper that triggers worker search for Dialogs |
| `applyEntityFilters()`            | Lightweight wrapper that triggers worker search for Entities |
| `jumpToDialog(id, isTDialog)`     | Switches to Dialogs tab, sets search to the ID, scrolls to and opens the matching card |
| `openExportModal()`               | Opens Share Content using the current active tab's filtered items; resets the per-open filter/removals |
| `getExportItemsForCurrentView()`  | The set that is rendered **and** copied — tab results minus removals, minus the modal's filter |
| `_renderExportBody()`             | Single entry point for re-rendering Share Content: list/table/grouped + counts + footer |
| `_renderExportGrouped(items)`     | Groups Share Content by Articles, Dialogs, Transactional Dialogs, sorted by id, with dialog → article refs |
| `buildItemUrl(kind, id)`          | Returns full CM.com URL for an item |
| `toggleContentSelectMode()`       | Toggles Collections multi-select on the Content tab, re-rendering the active panel with/without checkboxes |
| `buildCollectionExportRows(collection)` | Walks a collection's items, applies reachability + smart-filter exclusion, returns `{ rows, excludedCount, totalCandidates }` |
| `openCollectionsModal()`          | Opens the Collections modal (sidebar of collections + Smart filters, detail pane per collection) |
| `exportCollection(collectionId)`  | Builds export rows for a collection and saves them to a JSON file via `saveCollectionExport` |

### Rendering pipeline

1. Data loads via `window.electronAPI.getData(dataFolderPath)`
2. Maps (`dialogMap`, `tDialogMap`) populated
3. Combined item arrays assembled (each item gets `._kind = "article" | "dialog" | "tdialog"`)
4. Data is posted to `search-worker.js`, which precomputes indexed answer/node/entity search fields
5. `triggerSearch()` asks the worker for filtered/sorted Int32Array indexes
6. The active panel renders its paginated slice using `renderArticleCard`, `renderDialogCard`, or `renderEntityCard`; inactive tabs are marked dirty and render lazily

### Pagination

- Page size: `PAGE_SIZE = 50`
- `pagHtml(cur, total, callbackName)` renders numbered page buttons
- Pagination links use `onclick="goAllPage(n)"` etc. (inline handlers, intentional)

---

## Search types

Three distinct search types:

1. **Content search** — searches Dialogs and Articles and their content. Main search bar under the Content tab.
2. **Conversations search** — searches conversations and their context (e.g. filter by context). Can be very resource-intensive; use debounce, lazy loading, worker offloading, and only load necessary data when the user presses the search button or Enter.
3. **Chat search** — searches within a single chat. A chat is first found and opened via Conversations search; Chat search then operates within that opened conversation.

### Content search semantics

- `search-worker.js` is the source of truth for result inclusion. Renderer helpers may mirror parts of search only for snippets, highlights, and modal display.
- Plain search supports space-separated AND terms, `|` OR groups, quoted exact phrases, case sensitivity (`Aa`), whole word (`\b`), and regex (`.*`).
- Invalid regex mode returns an explicit `invalid_regex` result from the worker; the renderer must show that as an error state, not as a valid zero-result search.
- When content context filters and a text query are both active, the same answer output must satisfy both the context filter and the text query.
- `¬T` means **Responses only**. When enabled, search excludes IDs, titles/names, descriptions, node names, and entity enrichment.
- `ND` means **Exclude non-default responses from search**. It only affects matching when a text query is active and must not hide items for an empty query.
- A response is user-facing unreachable only when it is not the default response and it has no context condition. Non-default responses with context are reachable for users in that context and should not be labeled "non-default" or "unreachable" in result cards.
- Contextual/non-default query hits should show a compact snippet or reason on result cards so users can see why an item matched without opening the modal.
- The info modals' **Matches only** toggle (`modalMatchFilter`, `toggleModalMatchFilter`) must use the same answer/node sections that caused worker result inclusion. It is hidden entirely when no query is active — it can do nothing then, and a permanently greyed-out control reads as broken rather than inapplicable.

### Conversations search semantics

`build_session_filter_query` in `lib.rs` is the single source of truth. Every text, ID and entity search resolves to a list of SELECTs producing `(session_uuid, match_log_id)`, UNIONed into one `search_sessions` CTE — that list is what lets one query look in more than one place at once (message text *and* the entities that text triggered) and lets a single OR group carry its own extra WHERE clause. `conv_search` in `lib.rs` is the test module; every test there is a bug that was invisible in the SQL and only showed up in the rows that came back.

**A column filter always applies to a parenthesized expression.** In FTS5 `interaction_value : a OR b` binds the filter to `a` alone and lets `b` match any column — which is why a **U**-scoped (user) search kept returning conversations where only the bot had said the word. `fts_columns()` is the only place a colspec is built and it always emits `{cols} : (expr)`. "Both" is a column filter too: with none at all, a plain query also matched `article_ids`/`dialog_paths` and a number found conversations by their dialog path. Those two columns are only in scope when `queryIds` is on.

**Search terms are never emitted as FTS5 barewords.** `www.efteling.nl` or `qa-1234` unquoted is an FTS5 **syntax error**, so the whole search failed rather than the one term. Terms are tokenized (`fts_tokens`) and re-emitted as a quoted phrase, with the trailing `*` kept for single-word prefix matching.

- **A query FTS cannot express falls back to `LIKE`, never to nothing.** When a term produced no tokens at all (a lone `€`, `?`) the old code left the search CTE unbuilt, so the query silently returned *every* session — indistinguishable from a search that matched everything on purpose. A group with an inexpressible term now goes down the LIKE path as a whole.

**Punctuation gets an exact re-check on top of the index.** The tokenizer drops it, so `www.efteling.nl` and `www efteling nl` are the same FTS query. `term_needs_exact_check` flags terms carrying punctuation and the search adds a `LIKE '%term%'` on the stored text. Whitespace deliberately does not count: a quoted phrase already matches as adjacent tokens, and requiring byte equality there would throw away the diacritic-insensitive matching the tokenizer gives for free (`cafe` finding `café`).

- **The exact re-check is per OR group, which is why groups get their own SELECT.** Folded into one MATCH it would either be skipped for rows that matched a different group or wrongly applied to them. The single-MATCH fast path is still taken when no group needs a re-check, which is the common case.

**`#ID` search is FTS-narrowed and then decided exactly.** The FTS lookup is a *filter*, not the answer: `IdTarget::matches` compares whole ids, so `qa-123` no longer also matches `qa-1234` and `dn-6391-4` no longer matches `dn-6391-42`. Measured on a real 110k-interaction database: **5.0 s → 3 ms** for an Article id and **1.4 s → 1 ms** for a Dialog id. `an_id_search_on_a_real_database_matches_an_exhaustive_scan` (needs `CAI_TEST_DB`) pins the narrow+decide pair against a boundary-exact full scan; `search_perf::search_cost` is the timing harness.

- **Dialog and Node are one filter, and the `-` is what tells them apart.** `IdTarget::parse` reads `6391` as a Dialog and `6391-4` as one of its nodes; an Article has no nodes, so a `-` there is a typo rather than a node search. The old UI made the user pick between three pills to say something the query text already said.
- A Dialog matches an interaction that answered from one of its nodes (`article_ids`, `dn-<dialog>-<node>`) **or** merely walked through it (`dialog_paths`, `<dialog>:<node>/<node>/…`). `path_has_node` checks the dialog and the node together, so a second path in the same cell cannot contribute the dialog while another contributes the node.

**Entities are a search field, not a subset of the text.** The recognizer stores the entity it matched, not the wording that triggered it, so "mag ik een fles rood meenemen" is found by searching the entity `WIJN`. The **E** toggle sits with **U**/**B** and is independent of them; turning both message toggles off sends `queryScope: "none"`, which means entity-only. Nothing at all selected falls back to searching the text — an empty result set would read as "no matches" rather than "you switched it off".

- **The default is U + E** — what someone asked for, and what the bot understood it as. Both the markup (`class="conv-scope-btn active"`) and the initial state say so, and they must agree: `_syncConvScopeFromButtons` reads the buttons, so a disagreement at startup silently wins for whichever the first click resolves to. `setConvSearchScope(user, bot, entity)` is the only programmatic way in, and it goes through the same read-back.
- **The Entities tab has a 💬 Conversations button** (`entityConvButton` → `searchConvsForEntity`) on every entity card and in the entity modal, which is the only way to answer "which conversations actually fired this entity?". It switches to entity-only deliberately: the entity's *name* is a label the recognizer assigned, so searching the message text for it returns a different — usually much smaller — set.
- **The opened chat filters to the turns that matched.** `chatMatchEntities` mirrors the E toggle when a session is opened (the way `chatSearchRegex` already mirrors `.*`), and `turnMatches` then also tests `rowEntityFields(row)` — display name, internal name, matched text, entity id, cached on the row because `recognition_details` is a sizeable blob and a long chat re-renders on every filter change. Without it an entity-only search opens every result on "no messages match".
- **The entity chip that caused the match is marked** (`.is-hit`, accent-coloured like `<mark>`). And a GenAI row — which normally hides its recognition data because that data explains something other than the answer, see `## Chat rendering` — shows the entity anyway *when it is the hit*. Suppressing it there would leave the turn in the results with no visible reason at all.

- `entity_index` (`log_id`, `session_uuid`, `entity_id`, `name`, `matched`) lifts `recognition_details.entityMatches` out of the JSON at import time; searching it is a scan of a small narrow table instead of a JSON parse of every interaction. `name` is the `displayName` (falling back to the internal `name`), lowercased on the way in so the search side never has to. A bare number also matches `entity_id`.
- **It carries no secondary index on purpose.** The search is a substring `LIKE`, which no index can serve, and every extra b-tree is a per-row tax on import; `WITHOUT ROWID` keeps it to one write per entity match. On a real database it holds ~103k rows for 110k interactions and an entity-only search costs ~8 ms.
- **Deletion must remove entity rows too** — `purge_old` and `delete_interactions_by_dates` both do, alongside the FTS cleanup.
- The one-time backfill in `open_db` is gated on `META_ENTITY_INDEX_BUILT`, not on "is the table empty?": a database whose interactions genuinely triggered no entities would otherwise re-run the whole-table `json_each` scan on every launch. `the_entity_backfill_matches_what_an_import_would_have_indexed` asserts the SQL backfill and `entity_index_rows` agree, so an older database searches the same as a freshly imported one.

**Chat search matches what the session list matched.** The FTS index is built with `remove_diacritics`, so searching `cafe` returns conversations containing `café`; the in-chat search was plain JS `includes` and found nothing in them, which reads as the chat being broken rather than as two searches with different rules. `foldDiacritics` folds both sides.

- **It folds one character at a time, and that is the whole trick.** The folded string is exactly as long as the input, so an index into it is an index into the original — which is what lets `_chatMarkSegment` find matches in the folded copy and write `<mark>` into the real text. A whole-string `normalize("NFD").replace(/\p{M}/gu, "")` would be shorter than its input and misplace every mark after the first accent. Anything that is not a base-plus-marks decomposition (a lone surrogate half included) passes through untouched, which is what preserves the length.
- Ranges are merged before insertion, longest-first at a given start: two terms can cover the same span, and `<mark>` nested in `<mark>` renders wrong. `opening | openingstijden` produces one mark, not `[opening][stijden]`.
- Regex mode is deliberately **not** folded — the user wrote a pattern, and the backend's regex path does not fold either.

### Metadata filtering (Context · Metadata)

Both filter popovers — the Content one (`#contentCtxModalOverlay`) and the Conversations one (`#ctxModalOverlay`) — carry **two tabs**: Context and Metadata. Context says what the user's session was in; metadata (`OutputMetaData` / the `OutputMetadata` column) says what the bot's answer was *marked with* — `entryType`, `nochat`, `transaction`, `attractionIdentifier`, `restaurantName`. On the real export that is **40 distinct keys and 196 chips**.

One button and one popover, not two funnels in an already-crowded toolbar: the two tag sets are indexed the same way and filter with identical semantics, so the only thing that varies is which list is shown and which array a click lands in. `CTX_KINDS` / `CONTENT_CTX_KINDS` hold everything that differs, and `_buildTagChipsHtml` renders both (its `unit` parameter is the only real difference — sessions vs items).

- **The button's badge counts both tabs**, and each tab carries its own count, because a filter left on in the tab you are not looking at still narrows the results. **Clear all** clears both, or the badge would stay lit.
- **`escalationGroup` is never offered as a metadata chip.** It already has chips on the Context tab, where it belongs — it is a declared context variable with its own `Id`. Listing it twice would let a user set two filters that look independent and describe the same thing. (This is the filtering side only; `## Collections` documents separately why the *tag* is not a reachability condition.)
- **An empty value renders as `(empty)`, not as a blank chip.** "Set with no value" is a real state, distinct from `not set`, and a chip nobody can see is a chip nobody can click.
- **A nested value is split into `key.subkey`, never rendered whole.** `abortTransactionAction` holds `{"label":"Aanvraag stoppen","topicName":"Stoppen_Faciliteitenkaart"}`; as one chip that is an unreadable blob wrapping over four lines, and it is not a filter anyone would click. Flattened it becomes `abortTransactionAction.topicName = Stoppen_Faciliteitenkaart`, which is a question someone actually asks. **It also collapses three spellings of one value into one chip** — the compact form, the form CM stored with its newlines replaced by `__` (its line-break marker, see `## Chat rendering`), and the same object with its keys in the other order. On the real export that turns `abortTransactionAction`'s blobs into two clean keys, and `Stoppen` correctly aggregates across four items instead of appearing four times.
  - `unmark_json_breaks` / `_unmarkJsonBreaks` restore `__` **only outside a double-quoted span**, which is safe by construction: `_` is not valid JSON syntax there, so it can only be the marker. Inside a string it must survive — `Stoppen_TD_Algemeen` is a real value. `restoring_line_breaks_never_edits_inside_a_string` pins this.
  - **Only objects expand.** A JSON *array* has no stable member names, so splitting it would invent filter keys; it stays one pair. An empty object keeps its key so `not set` still distinguishes it from absent.
  - Bounded by `META_MAX_DEPTH` (3) and `META_MAX_LEAVES` (24) so a large embedded document can't turn one answer into hundreds of chips.
  - **Three copies exist** — `flatten_metadata_entry` (lib.rs), `flattenMetaEntry` (search-worker.js), `_flattenMetaEntry` (index.html) — because there is no module boundary to share one across. The frontend test compares the renderer's and the worker's directly *and* compares every chip's count against the matcher; the Rust one is covered by the backfill test.
- **`.ctx-chip` clamps to one line with the full value in its `title`.** A backstop for the merely-long, now that nested values are split before they get here.
- **Two filters on different keys must be satisfied by one answer**, matching how context works — an item whose answer A is `nochat=true` and whose answer B is `entryType=choice_prompt` does not match a filter asking for both.
- **Metadata is ANDed at the item level in the worker, not folded into `matchArticleCombined`/`matchDialogCombined`.** That path exists so a text query and a *context condition* are satisfied by the same answer; metadata is a tag on the answer, not a condition on reaching it, so requiring the text hit and the tag to coincide would exclude items the user is looking for.

**Conversations:** `metadata_index (name, value, session_uuid)` is a separate table with the same shape and the same two indexes as `context_index` — separate rather than a `kind` column because adding one would change that table's primary key on every existing database. `build_session_filter_query` builds both filters from one loop over `[(context_filters, "context_index"), (metadata_filters, "metadata_index")]`, so they cannot drift. `get_context_options` and `get_metadata_options` are both thin wrappers over `tag_options(table)`. Deletion is already handled: `cleanup_orphan_contexts{,_touched}` walk `TAG_TABLES`.

- The one-time backfill is gated on `META_METADATA_INDEX_BUILT`, not on "is the table empty?", for the same reason as `entity_index` — a database whose answers carried no metadata would re-run the whole-table scan on every launch.
- **`backfill_metadata_index` is a Rust pass, not one SQL statement** like the entity backfill beside it. A value can itself be a JSON object expanded into one row per leaf, and the sub-keys are not known in advance, so `json_each` cannot express it. Routing the migration through the same `metadata_index_rows` the import path uses is also stronger than asserting two implementations agree: there is only one. It streams inside one transaction and logs its row count and timing, and a failure leaves the flag unset so the next open retries.
- **The two sides encode the same information differently**, and only the parsers need to know: the content export spells it as a plain object (`OutputMetaData: {escalationGroup: "…"}`), the interaction log as an array of `{key, value}` pairs. `metadata_index_rows` reads the array; `metaSetOf` in the worker reads the object.
- **Metadata values keep their case**, unlike entity names — a value is a configured constant the user reads back off a chip, and `Polles Keuken` should not become `polles keuken`.
- Tests: `the_metadata_backfill_matches_what_an_import_would_have_indexed`, `the_spellings_of_one_nested_value_collapse_to_the_same_pairs`, `restoring_line_breaks_never_edits_inside_a_string`, `only_objects_are_expanded`, `a_metadata_filter_narrows_to_its_own_table`, and `frontend/tests/metadata-filter.test.js` — which asserts **every chip's count against the set the matcher returns**, over the real export when it is checked out beside the app (203 chips across 41 keys). A chip saying "9 items" that filters down to 4 is worse than no chip at all.

### Entity → Article / Dialog cross-references

`getEntityForChip(phrase)` is the single source of truth for "which entity is this phrase?", and it resolves by entity name, then by an entity *word*, then by the longest token in the phrase. `entityRefIndex()` is the **exact inverse** of it, built in one pass over the export and cached until `loadData`.

- The two directions used to disagree, and that is what "the entity view doesn't show all its Articles" was: a chip resolved a phrase by word or token and labelled itself `Entity: WIJN`, while `entityArticleXrefs` listed only Articles whose phrase was *verbatim* the entity name. On the real export that is **2013 entities with Article links instead of 1167**. `every chip's entity lists the Article it labels` in `frontend/tests/conv-search.test.js` pins the inverse property.
- **The worker's "Used in Articles" / "Used in Dialogs" pills are fed from the same index.** `buildEntityXrefSets` used to recompute the relationship itself with a name-equality check, so the pills filtered on a different rule than the cards they were filtering. It now just receives two name lists over the init message; the resolution stays on the main thread, where `getEntityForChip` lives.
- One entity per phrase (the first that resolves), matching what the chip displays, and an item is listed once however many of its phrases resolve to it.
- `_chipEntityCache` memoizes phrase → entity because the same phrases recur constantly and the index build asks for every phrase in the export; `null` is a real answer and is cached too. Building the index costs ~48 ms on the real export (3295 Articles, 808 Dialogs, 2233 entities), once per data load.

---

## Chat rendering

The chat view turns raw `interactions` rows into turns (`buildChatTurns`) and renders each row's `output_text` through `parseCmOutput`. Both have had bugs that read as "the chat is broken" rather than "the renderer is wrong", so the rules below are load-bearing.

### CM.com output formatting (`parseCmOutput`)

- **`_` is a line break, never emphasis.** A single `_` is `<br>`, a run of two or more (`__`, or `_  _`) is `<br><br>`. **`**text**` is the only bold marker.**
- **Do not reintroduce `__text__` → `<strong>`.** That rule existed, ran *before* the line-break rules, and therefore bolded whole paragraphs *and* swallowed the two breaks on either side — so answers rendered bold and run together with no spaces. It fired on **93 of 400** distinct sampled answers. `_` marking a break and `__` marking bold cannot both be true; the break wins.
- **List markers.** `*` followed by whitespace becomes `• `; `**` is never a list marker (both the leading and trailing star are guarded with a lookaround). The `*` that prefixes each `%{DialogOption(...)}` / `%{Image(n)}` token is consumed *with* the token, so what survives to the bullet step is a real content bullet. Deleting all markers outright — the old behaviour — flattened genuine lists into unmarked lines.
- **Anchors and `{{variable}}` chips are placeholder-protected** before the underscore pass, or a `_` inside a href or inside `{{opening_hours_to}}` becomes a `<br>` and breaks the element.
- Order inside the function matters and is: card.ask → CTA → DialogOption → other `%{}` tokens → bullets → markdown links → `<a>` links + HTML escaping → (botMode) protect → bold → line breaks → restore.

### `{{variable}}` is a redacted bot value

The bot side of the same story as `#Variable#` below: the answer was personalised for the user, but the log keeps the template (`Hoi {{name}}!`, `{{attraction\_name}}`, `{{emailAddress}}`, `{{opening_hours_from}}`). `templateVarChip(name)` renders each one as an inline `.tpl-var` chip instead of raw braces, and normalises the export's escaped `\_` back to `_`.

- **`.tpl-var` is deliberately near-padding-free** (2px, no margin, dotted underline rather than a bordered pill). A wider chip pushes the following punctuation away from it — `Hoi {{name}} !` — which reads as a typo. Verified visually before settling on it.
- `rawName` arrives already HTML-escaped, so the fallback branch (an unparseable token) must not escape it a second time. The sanitised branch is restricted to `[\w .-]`, where `esc()` is a no-op.
- `.tpl-var` and `.redacted-value` share one visual language on purpose — both mean "the log never stored this value" — but differ in weight, because one sits mid-sentence and the other is a whole message.

### Search highlighting never enters a tag

`chatHl` splits `body` on `/(<[^>]+>)/` and highlights only the text segments. `body` carries anchors (`href`, `onclick="previewUrl('…')"`) and `.tpl-var` chips with title text, so matching across the raw HTML injects `<mark>` into an attribute and breaks the element — searching `efteling` used to corrupt every link in the answer.

### `#Variable#` is a redacted user turn, not an internal value

CM logs a user's typed value as the variable name it was stored in (`#Voornaam#`, `#E-mailadres#`, `#Toelichting klacht#`, `#Vraag#`) whenever the field is PII. `isInternalValue` used to classify these as internal, which did two things:

1. dropped the user turn entirely, and
2. — the damaging one — stopped the `botRows` loop from breaking, so **every following bot row was absorbed into the previous turn**.

On the facility-card transactional dialog that produced a turn reading `User: "Ja"` followed by six consecutive bot bubbles, with the bot appearing to ask for a first name and then immediately a last name and no user input between them. It looked like message ordering was broken. 2,953 of 18,295 sessions in a real database were affected.

- `isInternalValue` covers only `continue`, `dialogId:nodeId`, and empty — genuine system values.
- `redactedUserLabel(v)` extracts the field name; `userBubbleBody(value, plan)` renders it as a lock chip (`.redacted-value`) instead of a text bubble, and is used by **both** chat render paths (`renderChatThread` and `renderFlaggedThread`) so they cannot drift.
- The **User** chat filter pill now includes these turns, which is correct — a user turn did happen.

### GenAI bubbles show no recognition data

`renderBubbleDetail` suppresses **recognition quality**, **entity matches**, and **dialogs** when the row is GenAI (`chatRowIsGenAi(row) || recognitionType === "GenerativeAI"`). A GenAI answer did not come from Conversational AI Cloud recognition, so those fields describe something else — rows with `all_interaction_types = ["QA","GenerativeAI"]` carry a populated `entityMatches`, `recognition_quality: 0.0`, and a `dialog_paths` of `{"DropOut": "…"}` (the dialog the user dropped *out of*), all of which read as an explanation of the answer. **GenAI source articles** (`faqs_found`) and **Recognition type** stay — those are genuinely about the GenAI answer. When nothing remains the empty state says so explicitly.

The low/zero-recognition bubble highlighting already excluded GenAI rows the same way; keep the two checks in agreement.

---

## Loading states

**A blank pane is not a neutral state — it looks exactly like a finished, empty one.** On a small database the reads below flash by; on a large one they are real seconds, and every one of them used to show nothing at all. Each now paints a spinner *before* it awaits and marks the controls that act on the not-yet-loaded thing inert.

- `paneLoadingHtml(label, note)` / `setPaneLoading(id, …)` render the shared `.pane-loading` block. The `note` line says *why* something is slow ("Counting interactions per day across the database"), which is the difference between waiting and wondering.
- `setBusy(ids, on)` toggles `.is-busy` — `opacity` plus `pointer-events: none`. A class rather than `disabled` so it covers non-form elements (the chat filter row, the calendar) with one rule. A control that looks live but does nothing reads as a bug.
- `setConvOverlay(visible, label, cancellable)` owns the sessions-pane overlay. It existed for search; opening a database reuses it with its own sentence and **no Cancel button**, because there is nothing safe to cancel halfway through a migration. Every caller goes through this function, so the label can't be left reading the previous operation's.

Where they are wired, and why each one mattered:

- **`selectSession`** — the thread kept showing the *previous* conversation until the interactions arrived, with the new card already highlighted. It now clears to a spinner first, and a second click while the first is in flight can't paint over the newer selection (`activeSessionUuid !== uuid` guard, with the busy flags released in a `finally` that covers that early return).
- **`openDbAtPath`** — the longest call in the app and the one that said nothing: `set_db_path` applies schema migrations, repairs the FTS index if stale, and runs the one-time entity and metadata backfills.
- **The data modal's Import and Stored data tabs** — both open on aggregate queries. The import calendar in particular would have rendered as "nothing imported", which is a *wrong* answer rather than a missing one.

**`yieldToPaint()` is a race, and the timeout half is the point.** WebKit will not repaint between a class change and a Tauri `invoke` that occupies the IPC channel, so the loading state needs two frames before the call goes out. But `requestAnimationFrame` **does not fire in a window that isn't being composited** — minimised, occluded, or on another space — so awaiting it bare parks the caller forever behind a spinner that never resolves. On screen the frames win and the paint happens; off screen the timer wins after `PAINT_YIELD_MS` and the work proceeds without one, which is correct: nobody is watching. `loadSessions` awaited the bare frames before this, so a minimised window could hang it indefinitely.

**`animateModalResize(box, mutate)`** grows a modal between two heights instead of snapping. The data modal's three tabs are a two-month calendar, a spinner and a column of settings — very different heights — so switching jerked the whole dialog under the pointer and took the tab bar with it.

- Everything between the two measurements is synchronous, so the intermediate layout is never painted. `height: auto` before measuring the target is what makes the box report its *natural* height when a previous animation still has an explicit one set; `max-height: 88vh` still applies, so an over-tall tab measures already clamped.
- **The explicit height is released on a timer, not `transitionend`.** A tab switched again mid-animation cancels the event, and a box left pinned would clip everything taller. `MODAL_RESIZE_MS` must stay in step with the `.modal-box.resizing` transition.
- Each tab's prepare paints its loading state *synchronously* before awaiting, so it is part of the height being animated to rather than a second jump straight after; the later content render wraps its own `_cdataResize`.
- `prefers-reduced-motion` skips the animation **and clears any height left pinned by an earlier one**, so the escape hatch can't strand the box.
- The Settings modal uses the same helper. Its tab handler is scoped to `#settingsModal` — the data modal's panels carry the same `.settings-tab-panel` class, and the unscoped query was clearing their active state.
- **The Import tab's own source tabs (Analytics API / CSV file) animate too**, and they are a separate mechanism: they re-render `#convImportBody` directly rather than toggling a panel class, so they never passed through `_cdataResize`. It is the biggest height change in the modal — a full calendar and date pickers against two paragraphs.
  - `_impSetupHtml` emits the tab strip and then `_impSourcePanelHtml()` inside a `.import-source-panel` wrapper. The split exists so the fade lands on the panel and **not** on the tab strip: the control you just clicked should stay put.
  - **`panel-in` is added by `impSetSource` after the render, never baked into the markup.** `_impRenderModal` also runs on every time-input change, every calendar click and the skip checkbox; a class in the HTML would replay the animation on all of them. No cleanup is needed because the next render replaces the element outright.
  - Clicking the already-active source returns early — it used to re-render for nothing.

### Entrance motion

Everything under `/* Shared entrance motion */` animates an **arrival** — something that previously appeared between one frame and the next. The rule is that none of it may gate a click, a render or a request: the element is laid out and interactive on the first frame and the animation only softens how it lands. Anything that would make the user wait for motion to finish does not belong here.

Three keyframes carry all of it, alongside the existing `tab-panel-in`:

| | Used for |
| --- | --- |
| `fade-in` | main tabs, the Content/Conversations/Flagged switch, loading affordances, the copy confirmation |
| `pop-in` | the tag filter and Add to Collection popovers — scale + rise, so a menu grows out of the control that opened it |
| `toast-out` | the exit that `toast-in` never had |

- **The big containers get opacity only, never a transform.** A `.panel` can hold thousands of nodes, and a transform would additionally make it the containing block for anything `position: fixed` inside it. `pop-in` is fine on a popover because both `.ctx-modal-box` call sites compute their position from the *button's* rect and write it inline **before** the box is shown, so the transform has nothing to disturb.
- **The animations restart because the elements are `display: none` in between**, not because anything re-adds a class. That is what keeps a search, a sort or a pagination click from replaying the panel fade — `.active` never leaves. Verified by `getAnimations()`: no animation on the hidden element, a fresh `running` one at `currentTime: 0` the moment it is shown.
- **An expanded card body and the chips behind a "+N more" fade, but the box still snaps to its full height.** Animating the height would mean measuring content that is only sized once revealed, and the page would visibly settle afterwards. `.chip-overflow` is `display: contents`, so the animation has to go on `> *` — the parent has no box to animate.
- **A toast only fades out when nothing is replacing it.** Every toast occupies the same corner, so animating one out while a successor appears leaves two messages stacked. `_dismissToast` removes the toast outright when another is already in the DOM, and `showImportToast` keeps removing its predecessor instantly; the fade is for the last toast on screen — the 6-second auto-dismiss and `clearProgressToast` when it stands alone. `TOAST_OUT_MS` must stay in step with `.import-toast.leaving`.
- **`_showCopyFeedback` restarts its fade explicitly** (`classList.remove` → forced reflow → `add`). Copying the same thing twice in a row otherwise confirms itself silently, because the text does not change and a running animation does not replay.
- **A fade on a loading affordance is not a delay.** `.pane-loading` and both overlays are in the DOM on exactly the frame they always were; the 140 ms only takes the flash off a fast read.
- `prefers-reduced-motion` disables every one of these in the single media query at the end of the block — including `toast-in`, which predates it. `.import-toast.leaving` gets `opacity: 0` rather than nothing, because the timer that removes it does not know about the media query.

#### No scrollbar may arrive mid-animation

A scrollbar that appears and disappears during an animation is worse than no animation: where scrollbars take real width — Windows, and macOS set to always show them — every element inside the container reflows narrower and then jumps back, so the motion reads as a glitch. Two rules keep that from happening, and both are invisible on a Mac with overlay scrollbars, which is why this is easy to ship broken.

- **A box animating its height clips the body inside it for the duration.** `.modal-box.resizing`'s own `overflow: hidden` covers the *box*; the element that actually scrolls is `.modal-body`, and for the length of the animation the box is deliberately shorter than its content — so the body became scrollable and then wasn't. `.modal-box.resizing .modal-body` clips instead; the content is growing into place anyway. The rule is duplicated under `#settingsModal` purely to outrank `#settingsModal .modal-body`'s ID specificity — check that with `getComputedStyle`, not by eye, if you touch either.
- **Containers that cross the scrollable threshold reserve the gutter** (`scrollbar-gutter: stable`, as `.chat-thread` already did): both height-animated modal bodies, `.list-wrap` (one per content tab — whether it scrolls depends on the result count, so switching tabs or just searching shifted every card sideways), `.sessions-list` (spinner ↔ a full page of results), and `.ctx-modal-body` (the chip list is fetched *after* the popover opens, so the scrollbar used to land mid-pop-in).
- Transforms are exempt by construction — they do not affect layout, so `pop-in`, `toast-out` and `tab-panel-in` cannot produce a scrollbar. Only the height animation and genuine content swaps can.

## The conversation data modal

`#convDataModal` is the one place data enters or leaves the database. Three tabs — **Import**, **Stored data**, **Database** — behind a single toolbar button (`#convDataBtn`, labelled *Data*).

It replaced two modals reached from two adjacent toolbar buttons (*Import* and *Manage DB*). They already shared the 620px box, the two-month calendar and the day-coverage colours, and they answer neighbouring halves of one question — "what have I got, and what am I missing?" — so getting from one to the other meant closing a modal and hunting for another button.

- **`_cdataSetTab(tab)` is the only way a tab changes**, and it re-renders the shared chrome (`_cdataRenderChrome`: title, the database subtitle, and the footer) before handing off to that tab's own prepare function. The footer is one element; each tab renders its own buttons into it, so the modal can never show one tab's actions over another tab's body.
- **The default tab is Import, except with no database connected — then it is Database.** There is nothing to import *to* yet, so a first-run click lands where the user has to act. Clicking through to Import in that state says so rather than offering a Start button that would fail.
- **While an import runs, the other two tabs are disabled** and the modal cannot be closed. Deleting or swapping the database file underneath a running import is a footgun with no upside, and the progress view lives on the Import tab — so for the duration, that tab *is* the modal.
- `openManageDbModal()` and `impClose()` survive as thin aliases; every old call site means "open on Stored data" or "close the data modal".
- **The `manageDbDeleteBtn` is created by the footer render**, not by the markup, so it can be replaced by a fresh disabled one without the body having changed. `_mdbSyncDeleteButton()` re-applies the state the current selection implies and is called from both `_cdataRenderChrome` and `renderManageDbBody`.

**Which database is open is stated in three places, and the toolbar is not one of them.** It used to be a green line across the top of the Conversations view, above every search. It now appears in the data modal's header (`#convDataDbLabel`), on the Database tab's path line, and in Settings → Conversations (`#settingsConvDbPath`, with a button that opens the modal on the Database tab) — because it is a value you set once and then want to forget. `updateConvDbStatus()` is the single writer for all three.

## Analytics API import

The data modal's **Import** tab offers two sources that both end at the same place: **Analytics API** (automated) and **CSV file** (the original manual `doImport()` path, unchanged). `CM_Analytics_API_SOP.md` is authoritative for anything about the API itself.

Responsibilities are split deliberately — keep them separate when extending this:

| Layer | Where | Owns |
| ----- | ----- | ---- |
| UI | `index.html` (`_impRenderModal` and friends) | modal, calendar, progress, cancel/retry |
| Scheduler | `index.html` (`buildImportQueue`, `_impRunQueue`, `_impFetchWindow`) | queueing, pipelining, subdivision, cancellation |
| API client | `analytics_api.rs` | token cache, one-at-a-time fetch, response validation, temp files |
| Import service + DB | `import_interactions_csv` (unchanged) | parsing, dedupe, FTS, context index, session summary |

- **The API returns the same CSV the manual workflow downloads.** The API path writes the response to a temp `.csv` and hands it to `import_interactions_csv`, so there is exactly one import pipeline and one duplicate-detection mechanism. Do not add a parallel importer.
- **Duplicate detection is `INSERT OR IGNORE` on the `log_id` primary key**, so re-importing a day is always safe and idempotent. Skipping already-imported days is therefore an optimisation, not a correctness requirement — never build a separate dedupe mechanism.
- **Do not replace this with unconditional overwrite (`INSERT OR REPLACE`) to save time.** It was measured as a fix for slow imports and found ~3.5× *slower* than `INSERT OR IGNORE`, not faster: `log_id` is already `INTEGER PRIMARY KEY`, so the "check" is the same rowid seek the insert must do regardless, and on a duplicate it skips the secondary indexes and FTS insert entirely — `OR REPLACE` instead deletes and re-inserts, re-maintaining every index, and would also break the `recognition_details` backfill and leave stale FTS rows. Import slowness has a real cause; see below.
- **The database stores raw UTC** (the portal CSV's `03/25/2026 09:30:22` is the same instant as the API's `2026-03-25T09:30:22.605Z`), so `get_db_daily_stats` groups by UTC date. `parse_ts` normalizes both formats to `YYYY-MM-DDTHH:MM:SS` so rows from either source are byte-identical — every `DATE(timestamp_start)` and range comparison depends on that.
- **Windowing:** the picker is **UTC end to end** — the date fields, the time fields, **Now** (`getUTCHours`), the calendar cells, and the default range all are, matching the database and the request windows. `_impUtcDate(dateStr, timeStr)` is the only place a picked date becomes an instant, and it goes through `Date.UTC`. `buildImportQueue` then cuts the range at UTC midnights so each request maps 1:1 to a DB day; one picked day is exactly one request, in any host timezone. A full day is `00:00:00Z` → `23:59:59Z` — **strictly under 24h**. That is *our* invariant, not an API rule: the SOP says a full-day request frequently *times out*, not that it is rejected, and the rule exists so a window maps onto one UTC day the calendar and skip logic can reason about. `validate_window` in `analytics_api.rs` enforces it, along with the SOP's 90-day retention limit (which is real).
- **Pipeline:** while day *N* imports, day *N+1* downloads. Only ever one API request is in flight — the JS scheduler serialises downloads and a `tokio::sync::Semaphore(1)` in `AnalyticsState` enforces it at the client layer regardless. **This cap is self-imposed politeness, not an API constraint** — the SOP documents no rate limit and no concurrency limit, despite earlier comments here and in `analytics_api.rs` claiming it did. Raising it is a legitimate option if downloads ever become the bottleneck; as of the run-scoped-finalize work they were not. `_impStartFetch` returns a promise that never rejects (`{ ok, parts | error }`) because a download is started one iteration before it is awaited.
- **Timeout subdivision vs backoff — two failures, two opposite responses.** The SOP warns full-day requests often time out, so a `Timeout` (408/504) halves the window (12h → 6h → …), sequentially, bounded by `IMP_MAX_SPLIT_DEPTH` and a one-hour floor — split only while *both* halves stay at or above an hour. Worst case ~6 requests per day, not an exponential fan-out. A `RateLimited` (429) or `ServerError` (5xx) instead **waits and retries the same window unchanged**, honouring `Retry-After` and otherwise backing off exponentially with jitter (`IMP_MAX_BACKOFF_ATTEMPTS`, `IMP_MAX_BACKOFF_MS`). All three used to be one `Timeout` kind, which meant the app responded to "you are sending too many requests" by splitting the window and sending *more*. `rate_limiting_is_not_mistaken_for_a_timeout` pins the distinction.
- **A token is never handed out with less life left than a request may take.** `TOKEN_SKEW_SECS` is `FETCH_TIMEOUT_SECS + 60`, and that relationship is the point — `a_token_is_never_handed_out_with_less_life_than_a_fetch_needs` asserts it. It was 120s against a 300s request timeout, so any cached token with 121–300s of life started a request it could not finish. **That is the "the first download fails, then the retry works" bug**: a request is authorised once, when it is sent, so a token dying mid-transfer does not come back as a clean `401` we could refresh and retry — it comes back as a reset connection or a truncated body, which classifies as `Network` and is not retryable. The day failed; pressing Retry worked because by then the token was stale enough to be replaced up front. `usable_lifetime` floors the reserve at half the token's life so a short-lived token is still worth caching.
- **One `reqwest::Client` for the process, built once.** The client *is* the connection pool, so constructing a fresh one per request threw the pool away every time and paid a full TLS handshake per window. It carries no global timeout — the token and fetch call sites want very different ones and set them per request. The pool brings back one failure mode it did not have: a keep-alive connection the server closed while idle fails on first use, so a `Network` error from `request_csv` is retried **once** before it is reported.
- **The status check comes before pagination detection.** A pagination header on an *error* response used to be reported as "the response appears paginated", hiding the real HTTP failure behind a message about a feature that isn't implemented.
- **Cancel does not wait for the in-flight download.** With a 300 s request timeout, awaiting it held "Cancelling…" on screen for minutes; the leftover fetch is cleaned up fire-and-forget when it lands, with the temp-dir sweep on modal open as the backstop.
- **`paginateData` is deliberately not sent.** The SOP requires confirming the pagination mechanism first, so instead the client fails loudly on anything paginated-looking rather than importing a partial day. Confirm the mechanism against the official spec before implementing it.
- **Temp files** live in `app_cache_dir()/analytics-tmp` and are deleted the moment each part's import returns, in a `finally` so failure and cancellation clean up too. The dir is swept on app start and on modal open (crash recovery). `cleanup_analytics_temp` is path-confined to that directory.
- **Credentials** live in `app_data_dir()/analytics-api.json` (`0600` on unix). The client secret never crosses the IPC bridge — `getAnalyticsConfig` returns `hasSecret` only, and saving with a blank secret keeps the stored one.
- **Skipping is decided per hour, not per day** (`get_db_hour_coverage` → `_impWindowCovered`). A range can start or end mid-day — picking 12:00 → 18:00 leaves the rest of that day missing — so a day-level "has rows?" check would silently skip the other 18 hours. A chunk is skipped only when every UTC hour it touches is already covered. (This mattered far more when the picker was local time and *every* multi-day import left two partial days behind; it is still the correct rule.) The calendar shows this in three states: green outline = every hour imported, orange outline = partly imported (will be fetched again), no outline = nothing yet. Never regress this to a day-level check.
- Skipped days stay in the queue marked `skipped` rather than being dropped, so the user can see what was left alone; they count toward overall progress.

### Diagnosing a failed import

A failed overnight import on someone else's Windows machine has to be diagnosable from what is on screen. The Rust `log` output records everything too, but it lands in a file most users will never find.

- **`fetch_analytics_window` returns a `trace` on success *and* on failure**, and `_impLogTrace` appends it, indented, to the Details log. A `Trace` is collected through the whole call and attached at the single exit point in `fetch_window`, so no error can escape without the history that explains it. It carries: whether the cached token was reused and how much life it had left, the token endpoint's status and timing, the full request URL, the response status/content-type/content-length and timing, the flattened error chain, and the first 300 bytes of any unexpected body.
- **`error_chain` is what makes a network failure readable.** `reqwest::Error`'s own `Display` is the outer wrapper (`error sending request for url (…)`); the sentence naming the actual cause — a TLS rejection, a refused connection, a proxy, a reset mid-body — is two or three `source()` hops down.
- **Nothing sensitive is traced.** The client secret never appears; the client id and the customer/project keys go through `mask()`, which shows two characters at each end.
- **`_impNormErr`'s fallback serialises what it got.** `String(e)` turned any object without a `message` into the literal `[object Object]` — precisely the "it gave errors" with nothing to act on.
- The log opens with `_impLogEnvironment()`: version, platform, the local-clock offset (log times are local, every window in it is UTC), the source and settings for the run, and the database. A pasted log has to stand on its own. **Copy log** is in the Details `<summary>` so it is reachable while the panel is still closed.
- Row-level parse failures are reported too. They don't fail the import, so without this a day could report "done" having silently dropped rows.
- Settings → **Test connection** shows the token exchange's own trace on failure, so it is a real first diagnostic rather than a yes/no.

### Coverage: asked-for vs present

Coverage used to be inferred purely from row presence, which cannot tell **"we asked and the API had nothing"** apart from **"we never asked"**. An hour with genuinely zero interactions — a quiet night, a maintenance window — therefore read as a permanent gap: the day never reached 24/24, stayed orange, and was re-downloaded on every run forever. Observed in the wild on 24 July 2026, where two full-day fetches both returned exactly 19,470 rows and neither contained anything in 02:00–02:59Z.

The `imported_windows` table (`day` PRIMARY KEY, `hours` bitmask) records which UTC hours were actually **requested**. `record_imported_window` ORs a window's hours in, and `hour_coverage` returns the **union** of hours-with-rows and hours-requested.

- **The union is what makes this backward compatible.** Row presence still counts on its own, so manually imported portal CSVs — which have no request window — keep marking their hours exactly as before, and existing databases need no backfill migration. A day imported before this existed simply behaves as it always did until it is fetched again once.
- **Record after the import returns, never before.** A window marked covered ahead of its rows would claim hours a later failure never actually stored. `_impImportParts` records per part, inside the same loop that already deleted the temp file.
- **`_impFetchWindow` attaches `startUtc`/`endUtc` to each part** it returns, so the window survives timeout subdivision — three sub-windows record three hour ranges, which is exactly right.
- **Deletion must forget the window, or a deleted day reads as fully imported and can never be re-downloaded.** `delete_interactions_by_dates` drops the rows for the days it deletes. `purge_old` is finer-grained because the retention cutoff falls mid-day: it deletes whole days before the cutoff day, then clears only the below-cutoff bits on the cutoff day itself (`hours = hours & ~((1 << cutoff_hour) - 1)`).
- A window is always inside one UTC day by construction, so `window_day_hours` rejects a cross-day span rather than recording it against the wrong day.
- Tests: `an_hour_the_api_answered_with_nothing_still_counts_as_covered` is the load-bearing one. `a_fetched_window_with_zero_rows_still_reports_coverage`, `a_partial_window_covers_only_the_hours_it_requested`, `purging_clears_coverage_only_for_the_hours_it_removed`, and `deleting_a_day_forgets_that_it_was_ever_fetched` cover the edges.

The Manage Database calendar still gates its outline on row count, so a fetched-but-empty day shows nothing there — correct, since that modal is about what is stored and deletable, not about what was requested.

### The shared day calendar

`calMonthHtml(monthDate, isFirst, cfg)` renders one month grid and is the **only** place a calendar month is built. The Import modal (`_impMonthHtml`) and the Manage Database modal (`_mdbMonthHtml`) are both thin wrappers over it, so the two calendars cannot drift apart visually. Everything modal-specific arrives through `cfg`: `keyFor(y, m, day)` (which date a cell means — both modals pass the shared `_calDayKey`), `classify(key)` (its classes and tooltip), `clickFn`/`dataAttr`, and `prevFn`/`nextFn`. `_calRangeCls(key, lo, hi)` is the shared two-click range/hover-preview classifier.

The CSS is shared under `.day-cal*` (renamed from `.import-cal*` when the Stored data calendar adopted it) — including the green/orange coverage outlines, which are inset shadows rather than borders so marking a day never shifts the grid by a pixel. Both calendars also share the class-only hover update (`_impUpdateCalClasses` / `_mdbUpdateCalClasses`): a full re-render on every `mousemove` fights the pointer.

**Both calendars mean a UTC day**, via the shared `_calDayKey(y, m, day)` — plain string formatting, no `Date`, no timezone maths. The grid coordinates already *are* the calendar date, so there is nothing to convert; routing through a `Date` is what reintroduces the local offset. `_mdbKey` is a thin alias kept for readability at the Stored data call sites. What still differs between the wrappers:

| | Import tab | Stored data tab |
| --- | --- | --- |
| Disabled | future, or older than the API's 90-day retention | future only — the DB may hold anything |
| Range colour | accent | red (`.day-cal.danger`) |

Both are UTC for the same reason: the data is. `DATE(timestamp_start)` is UTC, `delete_interactions_by_dates` matches on it, and the request windows the importer builds are UTC days — so on both tabs the day you click is exactly the set of rows that appears or disappears. Don't "unify" either one to local time.

The Import picker used to be local time. The mismatch showed up in two ways worth remembering, because each looks like its own separate bug:

- **One picked day became two requests.** A local day spans two UTC days (at UTC+2, local 25 Mar is `24T22:00Z → 25T21:59Z`), so a contiguous selection always left two ragged UTC edges — the first day fetched only its tail, the last only its head. Both then rendered orange (partly imported) indefinitely and were re-fetched on the next run. Harmless thanks to `INSERT OR IGNORE`, but it read as the importer failing to finish.
- **The outlines described a day you hadn't selected.** `keyFor` was local while `_impDbHours`/`_impDbDays` are keyed by the UTC date straight out of `get_db_hour_coverage`, so a cell's coverage colour and tooltip answered "is UTC day N complete?" while clicking it queued local day N. At UTC+2 the two overlap 22 of 24 hours, which is why it looked *almost* right rather than obviously wrong.

### Stored data and Database tabs

**Stored data** is calendar-driven cleanup; **Database** is the file picker, the retention setting and compaction. (The CSV export help moved to the Import tab's CSV panel, next to the button it explains, and its trigger is an inline `onclick` because that panel is rebuilt on every render.)

- **Selection is a date range, not a checkbox per day.** Click a start, click an end (click one day twice for a single day); `_mdbFrom`/`_mdbTo`/`_mdbPickPhase`/`_mdbHover` mirror the import picker's state exactly. Ranges are what cleanup actually needs ("everything before March", "that bad import week") and they stay usable at hundreds of days, where the old checkbox list did not.
- **Only days that hold data are ever sent to `delete_interactions_by_dates`.** `_mdbSelectedDays()` intersects the range with `get_db_daily_stats`, so an empty day inside a wide drag contributes nothing and cannot inflate the reported day count. The readout says so explicitly ("plus 3 days with no data") — a wide drag must not look like it will delete more than it will.
- **Nothing is deleted without a full statement of what goes.** The readout gives interactions + day count + range, the list under it names every affected day with its row count (scrolls rather than truncating — hiding a tail before a delete is exactly wrong), and `manageDbDeleteSelected` then arms a separate confirm zone.
- `_mdbDaySpan(lo, hi)` counts calendar days in **UTC**. Subtracting two local midnights across a DST change is an hour short, which turned the day count into `4.958…` — if you touch day arithmetic here, keep it in UTC.
- **Older than retention (Nd)** applies the retention window on demand. `conv-data-retention-days` otherwise only takes effect during an import (`purge_old`), so this is what makes the setting usable as maintenance. It is disabled when nothing is older than the cutoff.
- The Delete button exists only on the Stored data tab — it only ever acts on the calendar selection, and leaving it visible next to unrelated settings invites a misclick.

### Why import stays fast as the database grows

Import cost must be proportional to the size of the import, never to the size of the database. Everything below exists to keep it that way — measured at ~7× on a 107k-interaction / 11.9k-session database, and the gap widens as the DB grows.

**An import is a *run*, not a file.** `begin_import_run` → N × `import_interactions_csv(deferFinalize: true)` → `finalize_import_run`. The tail work (purge, scoped summary rebuild, FTS `'optimize'`, `PRAGMA optimize`) costs the size of the *database*, so paying it per downloaded window meant a 90-day API import paid it 90 times. Measured on a 120k-row database with 90 windows: **21.4 s → 5.0 s**, with the tail itself dropping from 17.3 s to 0.5 s. `perf::import_cost` reproduces the comparison.

- `import_csv_into(..., finalize)` — `true` keeps the original self-contained behaviour (and is the default for any caller that doesn't opt in); `false` skips the tail and lets the touched-session set accumulate via `ensure_touched_sessions` instead of resetting it. Both JS paths bracket their loop: `_impRunQueue` for the API, `doImport` for manual CSVs.
- **Cancel and failure must still finalize.** `_impRunQueue`'s `finally` calls it, because `_impAfterImport` reloads the conversation list immediately afterwards and sessions imported before the stop would otherwise be missing from `session_summary`.
- **`ensure_session_summary` alone cannot repair an abandoned run.** Its two invariants (session count, `MAX(last_log_id)`) both hold when a run added rows only to already-known sessions below the recorded high-water mark — which is what re-importing a partial older day does. So `begin_import_run` writes `app_meta.pending_finalize` and `open_db` does a *full* rebuild if it finds it still set. `an_abandoned_run_is_repaired_on_open` is built in exactly that blind spot and fails without the flag.
- `a_deferred_import_run_finalized_once_matches_a_full_rebuild` is the load-bearing test: N files deferred + one finalize must equal a full `rebuild_session_summary`, byte for byte.
- `begin_import_run` also raises `wal_autocheckpoint` to 20000 pages for the run (restored plus `wal_checkpoint(TRUNCATE)` on finalize), so a large import isn't interrupted by checkpoints every ~4 MiB.

- **`session_summary` is rebuilt incrementally.** `import_csv_into` records the `session_uuid` of every row it actually inserts into `TOUCHED_TABLE` (a per-connection temp table), then calls `rebuild_session_summary_touched`, which recomputes only those sessions. This is exact, not an approximation: a session's summary is derived entirely from that session's own rows, so an untouched summary was already correct. **Do not** call the full `rebuild_session_summary` from the import path — it re-aggregates every session in the database and its two correlated subqueries dominate everything else (~10.5 s vs ~1.6 s on the DB above, and it grows without bound).
- `rebuild_session_summary` (full) is still correct and still used where the touched set isn't known: on database open via `ensure_session_summary`, and after `delete_imported_dates`.
- Both share `session_summary_insert_sql(scope)` so the scoped and full variants can never drift apart. `scoped_summary_rebuild_matches_a_full_rebuild` and `a_real_import_leaves_the_same_summary_as_a_full_rebuild` assert the equivalence — the second drives a real portal CSV through the real import. Point `CAI_TEST_DB` at a copy of a real database to run the same check across every session in it.
- A scoped rebuild must leave `ensure_session_summary`'s two invariants intact (session count, and `MAX(last_log_id)` matching `MAX(log_id)`), or the app would do a full rebuild on every launch.
- **The import path does not sweep orphaned contexts.** An import only ever adds sessions, so it cannot orphan a `context_index` row. Only deletion can — `purge_old` handles it via `cleanup_orphan_contexts_touched`, scoped to the sessions it stripped.
- **`purge_old` does not rebuild the summary itself.** It adds the sessions it purged to the same `TOUCHED_TABLE` and lets the caller do one scoped rebuild covering both the import and the purge. It previously ran a full rebuild *and* the caller ran another.
- **The FTS `'optimize'` call is deliberately kept** — but it now runs once per *run*, not per file. It costs ~1.1 s and grows with the index, and measurement showed dropping it made the real `get_sessions` search query slower by an amount smaller than run-to-run noise. Don't delete it: `purge_old` and the Stored data deletions leave tombstones that only go away on merge.
- **The FTS index is contentless (`content = ''`, `contentless_delete = 1`).** The old standalone table kept a full second copy of `interaction_value`/`output_text`/`article_ids`/`dialog_paths` in `interactions_fts_content` — written on every imported row and *never read back*, because every use in this crate is a `MATCH` plus a rowid join and there is no `snippet()`/`highlight()`/`bm25()` anywhere. Measured on 200k rows: **23% faster to insert, 37% smaller on disk, and search equal or slightly faster** — including immediately after deleting 20% of rows, the tombstone case that was the reason to check. `perf::contentless_vs_standalone` is that gate; re-run it before changing the FTS shape again.
  - **A duplicate rowid no longer raises — it silently double-indexes.** That makes the `Ok(1)` gate in the import loop load-bearing for index correctness, not just for speed, and the FTS insert is wrapped in `let _ =` which would swallow the error anyway. `a_duplicate_row_is_never_indexed_twice` pins it.
  - `content = 'interactions'` (external content) was rejected: `COUNT(*)` would then scan the content table and always equal `COUNT(*) FROM interactions`, making `repair_fts_index`'s staleness check structurally incapable of firing, and a `DELETE … WHERE rowid IN (…)` after the content row is gone becomes a silent no-op. `fts_semantics::contentless_supports_count_delete_and_column_filters` verifies the four behaviours we depend on against the bundled SQLite.
  - `repair_fts_index` detects a pre-migration table via `sqlite_master.sql NOT LIKE '%contentless_delete%'`, drops it, and lets the existing count mismatch reindex. One-time cost measured at 0.3 s / 100k rows and 2.2 s / 500k — brief enough not to need its own progress UI.
  - **Dropping the old table frees pages but does not shrink the file.** Only `VACUUM` does, which is what the **Compact database** button in Data → Database (`compact_database`) is for. It needs ~2× the file size in temp space, so it stays a deliberate user action.
- **Three indexes on `interactions` were removed** (`idx_feedback`, `idx_session_uuid`, `idx_type`), each of which cost a b-tree write per imported row and bought nothing. Verified with `EXPLAIN QUERY PLAN` against the real queries first: every feedback filter is a leading-wildcard `LIKE` (unindexable, and its key was the whole JSON blob); `idx_session_uuid` is a strict prefix of `idx_session_ts`/`idx_session_log`; the only `main_interaction_type` equality is ORed with a `LIKE '%…%'`. `DROP_DEAD_INDEXES` runs in `open_db` because `CREATE INDEX IF NOT EXISTS` means removing the schema lines alone would never help an existing database. `dropping_the_dead_indexes_leaves_session_lookups_indexed` asserts a bare `session_uuid = ?` is still not a `SCAN`.
- **`PRAGMA analysis_limit = 400` is set for the whole connection** in `apply_perf_pragmas`. It defaults to 0 (unlimited), so every post-import `PRAGMA optimize` on a database that already had `sqlite_stat1` was fully scanning every index on `interactions`.
- **The `recognition_details` backfill is probed once per import, not per row.** `SELECT EXISTS(… recognition_details IS NULL OR = '')` decides whether the duplicate-row `UPDATE` can do anything at all; on a mature database it can't, and every duplicate row was paying an indexed seek to rediscover that. Sound because the probe reads pre-import state: a row inserted by this import carries its value in the INSERT. Don't fold this into `ON CONFLICT DO UPDATE` — `changes()` returns 1 for both branches, destroying the `Ok(1)`/`Ok(_)` distinction that gates the FTS insert, the touched-session insert, the context index and both counters.
- **Timings are reported, not guessed at.** `ImportResult.timings` / `FinalizeResult.timings` break the work into rows / purge / summary / FTS optimize / PRAGMA optimize, logged under `target: "import"` and shown in the import modal's Details log. Use them before optimising anything here again — the `read_record`/allocation work that looked obvious measured as pure noise, because the cost is SQLite b-tree writes, not Rust.

## Collections

Lets users multi-select Articles/Dialogs on the Content tab and export them as `[{ trigger, content }]` JSON for CM.com HALO's knowledge tool.

- **Selection**: a toggleable "Select" mode (`collectionSelectMode`, `#contentSelectModeBtn`) reveals a checkbox on Article/Dialog cards (not Transactional Dialogs — they have no `nodes`/content of their own). Selection state is `collectionSelection`, a `Set` of stable keys (`"article:<Id>"` / `"dialog:<id>"`), read back via `.has(key)` at HTML-string-build time inside `renderArticleCard`/`renderDialogCard` — required because every card list is fully rebuilt via `innerHTML =` on every search/filter/sort/pagination change, so DOM-attached state would not survive. "Select page" (`selectAllVisibleContent`) only adds the checkboxes currently rendered in the DOM; "Select all" (`selectAllFilteredContent`) instead walks the active tab's full `filteredArticles`/`filteredDialogs`/`filteredAll` index buffer — the same current search/filter result set `getActiveExportItems()` uses for Share Content — so it selects every matching item across all pages, not just the visible one.
- **Collections** (`cmCollections`, `localStorage["cm-collections"]`) are named groups of item keys, created/extended via the "+ Add to Collection" popover in the select bar. Managed (rename/delete/inspect/export) via the Collections modal (`#collectionsBtn`) — see `### Article and Dialog info modals

`#articleInfoModal` and `#dialogInfoModal` share `.dialog-info-modal-box` and the `navHistory` back-stack with the flow and entity modals.

- **They are sized to their content, not to the screen.** They were `calc(100vw - 32px)` × `calc(100vh - 32px)`; a typical Article is a dozen entity chips and one response, so that was ~90% empty space with the answer stretched across the full display. Now `min(1040px, 94vw)` wide, `height: auto` up to `min(880px, 92vh)`, so a short Article is a short modal and a long Dialog still scrolls. **The flow modal keeps its own full-screen `.flow-modal-box`** — a graph genuinely wants the space.
- **`.response-box` and `.desc-text` cap at `78ch`.** Answers are prose; past ~78 characters a line stops being readable regardless of how wide the window is.
- **Inside these modals the response box is clamped, not independently scrollable.** A scroll region nested in a scrolling body hides text with no outward sign there is more. `_applyResponseClamps` measures after render and, only when it actually overflows, adds a fade and a **Show full response** toggle. Call it after any `innerHTML =` on either body.
- **The entity chip grid collapses past `MODAL_ENTITY_PREVIEW` (12)**, reusing the cards' `.chip-overflow` / `.show-more-btn` / `toggleEntityOverflow` idiom with its own `eo-modal-<Id>` id so the card and the modal can't clash while both are in the DOM. An Article can carry dozens of entity phrases, which pushed the Response — the thing the modal was opened for — below the fold. The slice is taken over the **rendered** chips, not `art.Questions`: with the match filter on, `makeChip` returns `""` for non-matching phrases, and slicing the raw list would spend the preview budget on blanks.
- **Section `h4`s are sticky** within the scrolling body, so a long node list doesn't scroll past its own heading.
- **Back is only rendered when `navHistory` is non-empty** (`_navUpdateBackButtons`, which also covers the flow and entity modals). It used to be permanent and silently fall through to "close", implying a history that didn't exist.
- **A type badge sits before the title** (Article / Dialog / Transactional) — the same visual language as the Share Content rows.
- **Header actions carry one weight.** `cmOpenButton`'s `.btn-open-url` is globally a solid accent button (it is used elsewhere, e.g. the Halo Studio link, so it isn't restyled globally); inside these headers it is scoped down to match `.btn-show-dialog`, so no secondary action outshouts the item's own title. Actions never wrap mid-label — the title truncates first.
- **`_linkedFromSectionHtml` answers "how do people get here?"** — the reverse of an Article's "Linked Dialog", listing the Articles whose `DialogStart`/`TDialogStart` outputs route into this Dialog. For a **Transactional Dialog it is the only thing the export knows**, and it replaced a modal whose entire body was "No additional details available". It is appended only when the match filter is off, so it can never make a "nothing matched" body look like a result.

### Collections modal layout`. The popover states what the click will do (`Add N items to`) and shows per collection how many of the current selection it doesn't have yet (`+3` vs a dimmed `all added`) — adding is idempotent, so without that the difference between a useful click and a no-op was invisible.
- **Export algorithm** (`buildCollectionExportRows(collection)`, and its per-kind helpers `_articleExportRows`/`_dialogExportRows`): for each selected item, emits one row per *reachable* Answer — the default answer, plus every non-default answer that has real context (reusing `articleAnswerHasContext`/`dialogAnswerHasContext` — the same reachability rule as `## Content search semantics`). An item can legitimately contribute 0 rows: Articles that route into a Dialog/TDialog instead of answering directly, or dialog nodes whose Recognition links only lead to other routing-only nodes (common in real data — e.g. a dialog can be entirely a router into other dialogs). The Collections modal surfaces this rather than failing silently.
- For dialogs, a trigger comes from either of two sources, both resolved to reachable Answer item(s) on a **target** node via the shared `emitReachableAnswers` step in `_dialogExportRows`:
  - a non-fallback Recognition link's `condition.data.questions[]`, targeting `link.childNodeId` (mid-conversation, internal to the dialog); or
  - a referencing **Article**'s `Questions[]`, via `_articlesRoutingIntoDialog(dialogId)` — any Article with a reachable `DialogStart` Output (`DialogId` matching, `IsDefault` or has real context, same reachability rule as Answer outputs) targeting `DialogStartNodeId` (the dialog's entry point). This runs against the full loaded dataset regardless of whether that article is itself in the collection, since it only supplies the human-readable trigger phrase for content the dialog otherwise has no entity attached to. A dialog that is purely an internal router (every Recognition link only leads to further `DialogStart` hand-offs, never a direct Answer) can still produce real export rows this way — confirmed against production data.
- **The reachability rule is a setting, and it reports itself.** Non-default responses with no real context are dropped from the export — but that is now `cmExportKeepUnreachable` (`localStorage["cm-export-keep-unreachable"]`, storing the *opt-out* so the default and every existing install behave exactly as before), shown as a checkbox above the pattern list in the Smart filters pane with a live count of what it removes.
  - **`_articleExportRows`/`_dialogExportRows` tag rows `unreachable`, they do not drop them.** Dropping happened inside the row builders, which made the rule un-reportable and un-switchable: the rows were simply absent with nothing to say why. `_buildCollectionExportRows` is now the only place the decision is made, and it routes rejected rows into `excludedRows` with `matchedFields: [UNREACHABLE_REASON]` and `unreachable: true` so the **Filtered out** tab shows them alongside smart-filter drops (muted `.col-matched-chip.rule` vs orange, plus a one-line breakdown naming both causes). `unreachableCount` is returned whether or not the rule is on, so the setting can state its own impact.
  - **`_colFilteredPaneHtml` checks `excludedRows` before the enabled-filter list.** Rows can now be excluded with no smart filter enabled at all; gating on the filter list first hid exactly the rows this was meant to surface.
  - **An unreachable *route* taints the rows behind it.** `_colRouteIndex` tags entries rather than skipping them and `_dialogExportRows` ORs that into each row — a reachable response reached only through an unreachable `DialogStart` is still not reachable. Note the default route is chosen among the `DialogStart` outputs *to that dialog*, so a lone route is always the default.
  - **`OutputMetaData.escalationGroup` / `metadata.escalationGroup` is NOT a context condition** and must never be counted as one. It is the escalation group an answer *belongs to* — an effect, not a condition. The export proves the difference: the tag sits on **2088 of 2138 default answers** (a condition on the default answer would be meaningless), and where an answer carries both the tag and a genuine `escalationGroup` *condition*, the two values **disagree more often than they agree** (16 vs 10). A real condition — escalationGroup included, since it is a declared context variable with its own `Id` — always arrives through `ContextVariables`/`contextVariables`. Counting the tag made every non-default answer with an escalation group look reachable: **619 Article rows and 151 Dialog rows** in a real export, up from 45 and 66.
    - This is only about *reachability*. Filtering **by** escalation group (`answerPassesContextFilters`, the aggregate `_ctxSets`, `_rowContextText`, the context-filter chips) still reads the metadata tag and is deliberately untouched.
  - **There may be no default Answer at all.** `IsDefault` belongs to the *output*, and outputs of different types share the flag, so an Article whose default output is a `DialogStart` routes into a Dialog instead of answering — none of its Answers is the default, and each needs a condition to fire. `_defaultAnswerAmong` returns `null` in that case; the `answers[0]` fallback survives only where *nothing* carries the flag (which occurs nowhere in any export seen so far). Inventing a default affected **149 Articles and 29 Dialog output nodes**, and `_colRouteIndex` had the same flaw when picking a default route.
    - **Article 4937 is the canonical case** and is pinned by tests using its real shape: default output is a `DialogStart`, three non-default Answers with `escalationGroup` metadata and no `ContextVariables`. CM.com CAIC shows all three as unreachable; this tool exported all three, because *both* bugs fired on it independently.
  - **`dialogAnswerHasContext` now discounts `"any"`, matching the Article side.** It used to count any non-empty `contextVariables` entry, so the two disagreed: an Article answer with context `"any"` was correctly unreachable while the equivalent Dialog answer was treated as reachable — labelled as such in the UI and exported by Collections. The dialog shape stores values as one comma-separated `cv.value` string where the Article shape has a `Values` array; flattening it is the only difference between the two checks. This is shared with live content search (the `isUnreachable` badge), which is the point — they describe the same thing.

- Multiple trigger phrases on one row are joined with `" | "` (e.g. `"Entity | Other Entity"`) — an Article's full `Questions[]` list can be large (dozens of phrases) since every entity that reaches that Article funnels into the same dialog entry.
- **Smart filters** (`cmExportFilters`, `localStorage["cm-export-filters"]`) are global, user-managed exclusion patterns (plain case-insensitive substring by default, or regex per-pattern) applied at export time via `_rowMatchesExclusion(row, prepared)` — `prepared` being `_prepareExclusionPatterns(patterns)`, compiled once per build rather than per row. Matching is whole-row: if any tested value on a row matches an enabled pattern, the entire row is dropped. Each pattern has a `field` (`"entity"` default | `"content"` | `"context"`, chosen via a `<select class="sort-select">` in the Smart filters pane) selecting what gets tested: Entity checks each trigger phrase (`row.phrases`, original behavior); Content checks the answer text (`row.content`); Context checks a flattened, sorted `"name:val1,val2 ..."` string built by `_rowContextText(contextVars, escGroup, isArticle)` from the same `ContextVariables`/`contextVariables` + escalation-group fields `articleAnswerHasContext`/`dialogAnswerHasContext` already read for reachability (resolved to readable names via `ctxVarMap`, mirroring — without touching — the `ctxSet` normalization inside `answerPassesContextFilters`). Filters saved before `field` existed have no `field` key and default to `"entity"` for backward compatibility.
- **Merging** (`_mergeRowsByContent`, called inside `buildCollectionExportRows` after exclusion filtering, before the final `trigger`/`content` rows are built): rows with byte-identical `content` — regardless of source (two Articles, an Article and a dialog node, two dialog nodes, etc.) — are combined into one row, unioning their trigger phrases (deduped, first-seen order). Runs *after* exclusion so a smart-filter-dropped row's phrases never leak into a surviving row just because they happened to share content.
- `esc()` must **not** be applied to `trigger`/`content` values — that's for `innerHTML` rendering; `JSON.stringify` handles export escaping.
- `buildCollectionExportRows(collection, opts)` returns `{ rows, excludedRows, excludedCount, totalCandidates }`. `excludedRows` (unmerged — one entry per raw exclusion event, not deduped) is `{ trigger, content, matchedFields }[]`, where `matchedFields` is `["<field>: <pattern>", ...]` from `_rowMatchingPatterns(row, patterns)` (the patterns that matched, which `_rowMatchesExclusion` just checks the length of). This powers the **Filtered out** tab (`_colFilteredPaneHtml`) of whichever collection is open — what a currently-enabled smart filter is dropping and why, so a filter meant to catch one thing doesn't silently eat something else too. Each row lists its matching patterns as chips.

### Curating what actually gets exported

Four things can keep a response out of the file, and the Filtered out tab is the single place that lists all of them with the reason attached. In evaluation order:

1. **Hand-removed** — `excludedContent` (a response, keyed by its exact content string) and `excludedItemKeys` (a whole item). Checked **first, before any rule**: an explicit choice outranks a rule, and more importantly a hand-removal must always carry a working Restore button rather than being attributed to something the user cannot undo from that row.
2. **The reachability rule** — `cmExportKeepUnreachable`, above.
3. **Smart filters** — global patterns, minus the ones this collection has switched off.

- **Exclusions live beside `itemKeys`, never inside it, and that is the whole point.** Removing an item drops it from the collection and a later re-add from the Content tab brings its content straight back; an exclusion is remembered and keeps applying. `re-adding a held-out item keeps it held out` and `re-adding an item keeps its hand-removed response out` pin this.
- **Content is the row's identity** because `_mergeRowsByContent` already merges on it. A consequence worth knowing: holding out item A does not remove content that item B also produces — the row survives, attributed to B. That is correct (B legitimately produces it) but it is the one case where "I removed that" and "it is gone" differ.
- **`disabledFilterIds`** switches a *global* pattern off for one collection. `_colEffectivePatterns(c)` is the only place the effective set is computed, and `_colBuildSignature` includes it, so toggling one busts just that collection's cache. The per-filter row counts in the Filters tab are computed with `{patterns: [f]}` — one filter at a time — so the number answers "what does *this* pattern remove here" rather than being confounded by whatever else is on.
- **The Items tab filters to held-out items** via `_colItemFilter` (`"all" | "held"`), which composes with the search rather than replacing it. The pills carry their own counts, so the collection states how much is held out before you click. They live in the pane HTML and `setColItemFilter` swaps the active class in place — rebuilding them on every keystroke would fight the search box above. The filter resets with `_colQuery` when the selected collection changes, so it never silently carries onto a different one, and its empty state distinguishes "nothing is held out" from "your search matched no held-out item".
- **`_itemExportRowCount(key, collection)` takes the collection** so the Items tab reports what an item contributes *given this collection's curation*. Called without one it still answers for the raw item, which is what the select-mode hover preview wants.
- **`_colStats.zero` skips held-out items.** An item contributing nothing by choice is not the same problem as an item with nothing to contribute, and the note it drives explains the latter.
- **The rendered lists keep what they drew** (`_colVisibleRows` / `_colVisibleExcluded`) and the buttons address rows by index. Content strings are arbitrary text — too long and too quote-laden to put in an inline `onclick`. Any curation click re-renders, so an index is never read against a stale list.

**The export is exactly the preview.** `exportCollection` serializes `buildCollectionExportRows(c).rows` and nothing else, so there is one code path and the file cannot disagree with the screen. `frontend/tests/export-integrity.test.js` (`npm run test:frontend`) drives a fixture where every excluded string is uniquely tagged and scans the produced JSON *bytes* for all of them — content and trigger sides of every mechanism, plus the merge case where a held-out item shares content with a kept one. It also asserts the shape: rows carry exactly `{trigger, content}`, so no internal field (`unreachable`, `manual`, `ctxText`, `phrases`) can reach a customer-facing file. Re-run it before changing anything in this section.

**Search covers all three list tabs** through one `_colQuery`, one `_colSearchToolbar`, and one `_colRowMatches`. Sharing the query across tabs is deliberate: "it is not in the export — was it filtered out?" is the question the Filtered out tab exists to answer, and carrying the term across the tab switch makes it one click. The toolbar renders `value=_colQuery` because the pane is rebuilt on every tab change and an empty box over a filtered list reads as a bug. The Items tab matches an item on **the text of the responses it produces**, not just its name — that is what turns "I don't want this line in the export" into "here is the Article putting it there".

### Why Collections stay fast as they grow

A collection stores **nothing but references** — `itemKeys` is `["article:12", "dialog:34"]` and every export row is derived from the loaded export at read time. That is the right model (collections stay tiny and never go stale against the data), but the derivation was being redone on every render, and the Collections modal renders a lot: the sidebar computes stats for *every* collection, the detail header recomputes the open one, the Items tab recomputed each item **again** via `_itemExportRowCount`, and the export-preview search recomputed the lot on **every keystroke**. Three caches fix that, all invalidated only by `invalidateCollectionCaches()` from `loadData` — they are pure functions of the loaded export, so nothing else *can* invalidate them.

- **`_colRouteIndexCache` — `dialogId -> [{ article, nodeId }]`, built once.** `_articlesRoutingIntoDialog` used to scan all of `articleMap` **per dialog**, so a collection of a few hundred Dialogs re-walked every Article and every Output a few hundred times, per render. The index is built by one pass over the articles and inverted. Insertion order still follows `articleMap`, so emitted rows are byte-identical.
  - **There is deliberately no `DialogId == null` guard** when indexing. The old scan compared `o.DialogId === dialogId`, so a null `DialogId` matched a null lookup; keeping `null` as a valid key preserves that exactly instead of quietly changing which rows can be emitted.
- **`_colItemRowsCache` — item key → raw pre-exclusion rows.** `_itemExportRows(key)` is now the single door to `_articleExportRows`/`_dialogExportRows`; `_itemExportRowCount` and the select-mode hover preview (`_contentPreviewHtml`, which ran the full derivation **on every card hover**) both go through it.
- **`_colBuildCache` — collection id → `{ sig, result }`.** `_colBuildSignature` is a `JSON.stringify` of the enabled patterns plus `itemKeys`, so editing a filter or adding an item simply stops matching and no mutation site has to remember to invalidate. `buildCollectionExportRows(collection, {patterns})` bypasses the cache — an explicit pattern set is a one-off question.
- **Smart-filter patterns are compiled once per build** (`_prepareExclusionPatterns`), not per row. A `new RegExp(...)` and a `pattern.toLowerCase()` used to happen inside the per-row test; `_rowMatchingPatterns` now takes prepared entries and lowercases each haystack once per row rather than once per row × pattern. Measured on 5k rows × 6 patterns: **1645 ms → 29 ms**, with identical matches.
- **Both row lists render in pages of `COL_ROWS_PAGE` (200)** with a "Show N more" control, and the preview search is debounced 120 ms. Thousands of `.col-row` nodes in one `innerHTML` is a visible freeze; the count above the list always states the real total, so nothing is hidden silently. `_colRowsShown` resets whenever the selected collection or tab changes.
- The preview search `<input>` is still built once by `_colContentPaneHtml` and only `#colRowsWrap` re-renders, so the debounce cannot cost the caret.

### Article and Dialog info modals

`#articleInfoModal` and `#dialogInfoModal` share `.dialog-info-modal-box` and the `navHistory` back-stack with the flow and entity modals.

- **They are sized to their content, not to the screen.** They were `calc(100vw - 32px)` × `calc(100vh - 32px)`; a typical Article is a dozen entity chips and one response, so that was ~90% empty space with the answer stretched across the full display. Now `min(1040px, 94vw)` wide, `height: auto` up to `min(880px, 92vh)`, so a short Article is a short modal and a long Dialog still scrolls. **The flow modal keeps its own full-screen `.flow-modal-box`** — a graph genuinely wants the space.
- **`.response-box` and `.desc-text` cap at `78ch`.** Answers are prose; past ~78 characters a line stops being readable regardless of how wide the window is.
- **Inside these modals the response box is clamped, not independently scrollable.** A scroll region nested in a scrolling body hides text with no outward sign there is more. `_applyResponseClamps` measures after render and, only when it actually overflows, adds a fade and a **Show full response** toggle. Call it after any `innerHTML =` on either body.
- **The entity chip grid collapses past `MODAL_ENTITY_PREVIEW` (12)**, reusing the cards' `.chip-overflow` / `.show-more-btn` / `toggleEntityOverflow` idiom with its own `eo-modal-<Id>` id so the card and the modal can't clash while both are in the DOM. An Article can carry dozens of entity phrases, which pushed the Response — the thing the modal was opened for — below the fold. The slice is taken over the **rendered** chips, not `art.Questions`: with the match filter on, `makeChip` returns `""` for non-matching phrases, and slicing the raw list would spend the preview budget on blanks.
- **Section `h4`s are sticky** within the scrolling body, so a long node list doesn't scroll past its own heading.
- **Back is only rendered when `navHistory` is non-empty** (`_navUpdateBackButtons`, which also covers the flow and entity modals). It used to be permanent and silently fall through to "close", implying a history that didn't exist.
- **A type badge sits before the title** (Article / Dialog / Transactional) — the same visual language as the Share Content rows.
- **Header actions carry one weight.** `cmOpenButton`'s `.btn-open-url` is globally a solid accent button (it is used elsewhere, e.g. the Halo Studio link, so it isn't restyled globally); inside these headers it is scoped down to match `.btn-show-dialog`, so no secondary action outshouts the item's own title. Actions never wrap mid-label — the title truncates first.
- **`_linkedFromSectionHtml` answers "how do people get here?"** — the reverse of an Article's "Linked Dialog", listing the Articles whose `DialogStart`/`TDialogStart` outputs route into this Dialog. For a **Transactional Dialog it is the only thing the export knows**, and it replaced a modal whose entire body was "No additional details available". It is appended only when the match filter is off, so it can never make a "nothing matched" body look like a result.

### Collections modal layout

The modal is **master/detail**: the sidebar (`#collectionsNav`) is the only place a collection is chosen and `#collectionsDetail` the only place one is acted on. `_colView` holds the selected collection id, or `COL_FILTERS_VIEW` for the global Smart filters pane; `_colTab` selects the detail tab and `_colQuery` the export-preview search.

It replaced three top-level tabs (Collections / Smart Filters / Filtered Out) whose list rows carried five identically-weighted text buttons. Two problems that shape the current design and shouldn't be reintroduced:

- **A collection was chosen twice** — once in the list, then again from a `<select>` in "Filtered Out". "Filtered out" is now a tab on the collection already open, so there is one selection and it persists across tabs.
- **"View items" and "View content" were toggle panels *inside* a list row**, so opening one pushed every other collection down the page, and both could be open at once. They are tabs now: **Export preview** (the rows that will be written), **Items** (the source Articles/Dialogs), **Filtered out**.

The detail header states the collection in one line (`N items · N export rows · N filtered out`) and weights its actions: **Export JSON** is a `btn-primary`, Rename/Delete are quiet ghost buttons.

- **Export preview** (`_colContentPaneHtml`/`_colRenderRows`) is live-searchable over the actual computed rows (post-reachability, post-exclusion, post-merge). The search `<input>` is built once and only `#colRowsWrap` re-renders per keystroke, so the caret isn't lost mid-edit the way a full-pane `innerHTML` rebuild would. Matches highlight via `hl()`, over both `trigger` and `content`. Trigger phrases render as individual chips rather than one `a | b | c` string — an Article's `Questions[]` can run to dozens of phrases.
- **Items** (`_colItemsPaneHtml`) shows each source item with a kind badge and its own row count. Three distinct states get three distinct messages, because they need different fixes: items that produce **0 rows** (normal — routing-only content), items that no longer exist in the loaded export (`_colItemExists` — stale keys after a data reload), and an empty collection (told how to add). The old UI collapsed the first into a bare orange "N items contribute 0 rows" count and never surfaced the second at all.
- The empty states are load-bearing, not decoration: a collection can legitimately produce zero rows, and without an explanation that is indistinguishable from a bug.

---

## UI structure

```
<header>
  brand | file tags | Export IDs button | Collections button | Settings button (gear)

<div.global-search-bar>
  search input | [Aa] [\b] [.*] [¬T] [ND] | tag filter button (Context · Metadata)

<div.tab-bar>
  All Results (sub-stats: art · dlg · t.dlg)
  | Articles (sub-stats: resp · dlg-lnk)
  | Dialogs (sub-stats: dlg · t.dlg · nodes · recog)
  | Entities (sub-stats: entities · words)

<div.content-select-bar>
  selection count | Select page / Select all / Clear | + Add to Collection popover | Select mode toggle

<div#panel-all>
  filter pills (All / Articles / Dialogs / Transactional Dialogs)
  item list | pagination

<div#panel-articles>
  filter pills (All / Has response / Dialog link)
  item list | pagination

<div#panel-dialogs>
  filter pills (All / Dialogs / Transactional Dialogs / Has responses)
  item list | pagination

<div#panel-entities>
  filter pills (All / Used in Articles / Used in Dialogs)
  entity list (words · Used by Articles/Dialogs · 💬 Conversations) | pagination

<div.conv-sidebar-header>
  [#ID] | search input | search submit | [.*] | [U] [B] [E]
  "Search by:" pills, only in #ID mode: Article ID | Dialog / Node ID
  date range button | tag filter button (Context · Metadata)
  filter pills (GenAI / feedback / Low % / Zero %)

<div#settingsModal>
  header: Settings | Backup… (opens #settingsBackupModal) | ✕
  Content tab: CM.com Context URL input, Open CM.com links radio (popup / browser)
  Conversations tab: connected database + "Manage database…",
                     Halo Studio URL, low recognition threshold, chat copy format,
                     Analytics API (client ID / secret / customer key / project key /
                     culture / environment / activeSessionOnly / Test connection)

<div#settingsBackupModal>
  what the file does and does not contain
  amber warning: the file holds live credentials, treat it as a password
  Export settings / Import settings | status line

<div#convDataModal>
  header: "Conversation data" | connected database filename (green) | ✕
  Import / Stored data / Database tabs
    — the last two are disabled while an import is running
  Import:  Source tabs (Analytics API / CSV file)
    Setup:   From + To date fields (click to choose which end you're picking) and
             time inputs | Now shortcut — all UTC, as the legend states
             always-visible two-month calendar — green outline = fully imported,
             orange outline = partly imported
             summary (N days · M fully imported · K to download, UTC request window)
             "Skip days already imported in full" checkbox
    Running: current operation | progress bar | "N of M days completed"
             per-day list with status chips
             collapsible Details log with a Copy log button
             Cancel import — or, when stopped, Retry/Resume from <date>
    CSV:     what an Interaction Log CSV is | How to export the Interaction Log?
  Stored data: interactions · days stored · date range
             legend | same two-month calendar as Import, range picked in red
             quick actions (Older than retention Nd / Everything stored /
                            Clear / Jump to latest data)
             readout of exactly what Delete removes
             scrolling list of the affected days | confirm zone
  Database:  Create new / Open existing | path | retention days |
             Compact database (VACUUM)

<div#exportModal>
  header: "N items from <tab>" | query chip | amber chip when no CM.com URL set
  List / Grouped / Table tabs | one-line description of what the view produces
  toolbar: filter input | "N of M" | Restore N removed items
  rows: id · type badge · title · matched responses (or dialog → article chips)
        · Copy link · remove (✕ on hover)
  footer: Copy N links / Copy table (N rows) | Copy as plain text | feedback

<div#collectionsModal>
  sidebar: + New collection | one row per collection (name · N items · N rows)
           | pinned "Smart filters" entry with enabled-pattern count
  detail (per collection): name · counts | Rename / Delete / Export JSON
    Export preview — search + trigger chips and response text per row, ✕ to
                     hand-remove a response from the export
    Items          — All items / Held out pills, search (name *and* response
                     text), kind badge, name, N rows, Hold out / Restore,
                     ✕ remove; notes for 0-row/stale items
    Filtered out   — search (incl. reason), excluded rows with the chip that
                     removed them — pattern (orange) / rule (muted) / by hand
                     (accent, with Restore)
    Filters        — which global smart filters apply to this collection, each
                     with the rows it removes here and an Applied toggle
  detail (Smart filters): live "removing N rows across M collections" summary,
    reachability rule checkbox + its own live impact count,
    field selector (Entity/Content/Context) + pattern + regex add row,
    list with Field/Regex/Enabled toggles
  empty state: what a collection is, and the three steps to make one
```

Content result relationship displays:

- Article cards show clickable Dialog Link / Transactional Dialog chips inline; avoid separate "Directs to ..." text when the target can be part of the chip.
- Dialog cards can show "Uses articles" relationship rows with clickable `qa-...` chips.
- Share Content `Grouped` view always groups by Articles, Dialogs, Transactional Dialogs, then sorts by id. Dialog rows that reference articles should visibly read as dialog → article relationships, e.g. `dn-123 -> qa-456`, with clickable chips in the UI.

### Share Content modal

Mirrors the active tab's current result set, then lets you refine *what gets shared* without touching the search behind it.

- **`getExportItemsForCurrentView()` is the only set that matters.** Rendering and all three copy actions read it, so the list on screen and the text on the clipboard cannot disagree. It applies `_exportDropped` (per-row ✕) and `_exportFilter` (the modal's own filter box) on top of `getActiveExportItems()`. Both reset on every open — the modal always *starts* as an honest mirror of the tab.
- **The primary button names its own count** (`Copy 5 links`, `Copy table (5 rows)`). Once a filter or a removal can shrink the set, "Copy all as links" is a claim the button can't back up.
- **Each view gets a one-line description** (`EXPORT_VIEW_HINTS`). "Grouped" does three things at once — sections, ID order, and Dialog → Article relations *replacing* the response column — and none of that is inferable from the word.
- **A type badge sits on every row** because `dn-` prefixes both Dialogs and Transactional Dialogs: in the flat List view the ID alone could not tell them apart. In the Table view the badge rides inside the ID cell rather than taking a fourth column, so `_copyExportTable`'s clipboard output keeps its original three columns.
- **An unset `cmBaseUrl` is stated, not implied.** Without it every "link" copy silently degrades to plain IDs; the header shows an amber chip that opens Settings.
- `_exportRowHtml` is shared by List and Grouped. They were near-identical copies before, which is how Grouped's relation column drifted out of List.
- The copy formats themselves are untouched: rich-HTML links (with grouped `<strong>` section headers), TSV plain text (with `dn-9 -> qa-101, qa-102` relation text in grouped view), and the HTML+TSV table.

---

## Terminology (CM.com Conversational AI Cloud)

Always use these terms in the UI:

| Use                  | Never use                   |
| -------------------- | --------------------------- |
| Article              | Knowledge Base Item         |
| Entities             | Questions, Training Phrases |
| Response             | Answer Output               |
| Dialog               | Flow                        |
| Transactional Dialog | Transfer Dialog, tDialog    |
| Recognition Node     | Recognition                 |
| Output Node          | Output                      |
| Dialog Link          | DialogStart                 |
| CM.com Context URL   | Base URL                    |

---

## localStorage keys

| Key                        | Value |
| -------------------------- | ----- |
| `cm-base-url`              | CM.com context URL override (string) |
| `halo-base-url`            | HALO/other context URL override (string) |
| `cm-open-mode`             | `"popup"` or `"browser"` |
| `cm-other-open-mode`       | `"popup"` or `"browser"` |
| `cm-dismissed-version`     | Last update version the user dismissed |
| `cm-data-folder`           | Last selected content export folder |
| `cm-sort-all`              | All Results sort choice |
| `cm-sort-articles`         | Articles sort choice |
| `cm-sort-dialogs`          | Dialogs sort choice |
| `cm-flow-direction`        | Dialog graph layout direction |
| `cm-view`                  | Last selected main view |
| `conv-db-path`             | Last selected conversations database |
| `conv-low-recog-threshold` | Low recognition threshold |
| `conv-data-retention-days` | CSV import retention window |
| `chat-copy-format`         | Chat copy format preference |
| `cm-collections`           | JSON array of `{ id, name, itemKeys, createdAt, updatedAt }`, plus optional `excludedItemKeys`, `excludedContent` and `disabledFilterIds` curation lists (all default `[]`) |
| `cm-export-keep-unreachable` | `"1"` to export non-default responses that have no context (or context `"any"`); anything else, including absent, keeps the default reachability rule on |
| `cm-export-filters`        | JSON array of `{ id, field, pattern, isRegex, enabled }` (`field`: `"entity"` \| `"content"` \| `"context"`, missing = `"entity"`) — global smart-exclusion patterns for Collections export |

Analytics API credentials are deliberately **not** in localStorage — they live in `app_data_dir()/analytics-api.json`, written by Rust with `0600` perms, so the client secret never reaches the renderer. A settings backup does export them, and still does not break that rule; see `## Settings backup`.

Example `cm-base-url` value: `https://www.cm.com/en-gb/app/aicloud/dbd80c7c-e9b1-44d2-9762-fb5ad1664b7f/Efteling/EFTELING/nl/`

---

## GitHub repository

GitHub account: **WithoutWout** (not `wouttonio`)
Repository: `WithoutWout/cm-conversation-dashboard`
Release URL pattern: `https://github.com/WithoutWout/cm-conversation-dashboard/releases/latest`

- Always use `WithoutWout` as the GitHub username, never `wouttonio`.
- `check_for_updates` reads `releases/latest/download/latest.json`, not the GitHub API — see `## Self-update`.

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

---

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

---

## Coding conventions

- All HTML built via string concatenation — always use `esc()` for any dynamic value.
- CSS variables for theming: `--bg`, `--surface`, `--surface2`, `--border`, `--text`, `--muted`, `--accent`, `--green`, `--blue`, `--orange`, `--red`, `--teal`.
- Internal identifiers (`_kind`, `tDialogMap`, `b-tdialog`, CSS class `type-tdialog`) use the short `tdialog`/`tDialog` form — only the user-facing label says "Transactional Dialog".
- Use `querySelector` / `getElementById` for DOM access; event delegation where multiple dynamic elements share a handler.
- `buildSearchRegex` is the single source of truth for search logic — do not duplicate regex construction elsewhere.
- Inline `onclick="..."` attributes are used intentionally for dynamically rendered cards (no event listener cleanup needed in this app).
- Rust commands use `snake_case`; the JS shim maps them to `camelCase` on `window.electronAPI`.

### Icons

**No emoji in the UI.** They are drawn by the platform's own font, so they arrive
in someone else's colour, weight and metrics — full-colour glyphs in a monochrome
UI, resizable only through `font-size`. All 63 of them were replaced by the
`.icon-sprite` at the top of `<body>`: 23 `<symbol>`s, each drawn with
`<svg class=ui-icon><use href=#i-name /></svg>`.

- **The sprite exists because this app builds HTML by string concatenation.** One
  `<symbol>` definition serves both the static markup and the ~50 call sites
  inside JS string literals; pasting paths at each site was the alternative.
- **Those `<use>` references are deliberately written without attribute quotes.**
  They sit inside JS strings of all three kinds (`'…'`, `"…"`, `` `…` ``), and an
  unquoted attribute value is the only spelling that needs no escaping in every
  one of them. It is valid HTML5 — don't "fix" the missing quotes.
- **`1em` + `currentColor`, never a `font-size`.** An icon inherits the size and
  colour of the text beside it, which is what keeps it correct inside the accent,
  green and orange states that recolour their containers.
- **Weight must match the ~93 Feather-style icons already inline in the file.**
  Those use a 24 grid at `stroke-width: 2`; the sprite uses a 16 grid at `1.35`.
  Same ratio, so the two sets sit together without a seam — keep it if you add one.
- **`#i-gear` is the exception on a 24 grid**, because it is literally the header
  Settings button's path: the gear in "click Settings ⚙ to configure" has to *be*
  that button, not merely resemble it. It sets `stroke-width` locally.
- **Typographic characters are deliberately left alone**: `✕ ✓ → ← ↑ ↓ ↗ ‹ › ▾ ▶`.
  They render identically everywhere, carry no colour of their own, and some (`→`
  especially) end up inside text this app copies to the clipboard, where an
  `<svg>` would simply be lost.
- Verify additions by rendering every symbol and checking `getBBox()` is non-zero
  — a wrong id or a malformed path draws nothing at all, silently.
