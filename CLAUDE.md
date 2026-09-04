# CLAUDE.md

## Project overview

Tauri desktop dashboard for inspecting and navigating CM.com Conversational AI Cloud content exports. Reads two JSON files from a user-selected folder and renders a searchable, filterable UI in a single window.

**Stack:** Tauri v2 (Rust backend + vanilla JS frontend), vanilla JS, no bundler, no framework.

Keep changes simple, scoped, and in line with the current architecture. Avoid unnecessary abstraction or complexity.

Libraries may be used, but must be vendored locally (e.g. `frontend/vendor/`) so the app works fully offline. Never load dependencies from a CDN.

**CM.com Analytics API:** `CM_Analytics_API_SOP.md` (gitignored, local-only) is the single source of truth for the Analytics API — OAuth2 token generation, the interactions endpoint, and its limits. The client lives in `src-tauri/src/analytics_api.rs`; consult the SOP before changing it. See `docs/import.md`.

---

## Where the details live

`CLAUDE.md` is loaded in full at the start of every session, so it holds only
what is true of the whole app: the architecture, the command surface, the
terminology and the conventions. The reasoning behind each **feature** — which
is where the bugs and the load-bearing decisions are — lives in `docs/`, one
file per area.

**Read the matching doc before changing anything it covers.** These are not
background reading: each one records decisions that were arrived at by
measurement or by a bug, and several of them exist because the obvious
implementation was tried first and was wrong.

| Doc | Read it before touching |
| --- | --- |
| `docs/search.md` | content search, conversations search (`build_session_filter_query`), the Context · Metadata tag filters, entity cross-references |
| `docs/insights.md` | the Insights dashboard — the chooser, the two units, the charts, the scope cache, the copy formats |
| `docs/import.md` | the conversation data modal, the Analytics API client, import performance, day coverage, the shared calendar |
| `docs/collections.md` | Collections, the export algorithm, smart filters, the Article/Dialog info modals |
| `docs/chat-rendering.md` | `parseCmOutput`, chat turns, redacted values, per-message metadata |
| `docs/loading-states.md` | spinners, `gateLoading`, `yieldToPaint`, modal resize, entrance motion |
| `docs/settings-and-updates.md` | the settings backup file, and the portable-exe self-update |
| `docs/ai-export.md` | `export_conversations_for_ai` and its `.jsonl` schema |

A rule that belongs to one feature belongs in that feature's doc. A rule that
would change how you write *any* part of this app belongs here.

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
    settings-backup.test.js, metadata-filter.test.js,
    msg-meta-place.test.js, loading-gate.test.js,
    insights.test.js                                 — `npm run test:frontend`
package.json        — scripts: tauri dev / tauri build / test:frontend
docs/               — the per-feature reference; see `Where the details live`
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

| Command               | JS call via `window.backend` | Description |
| --------------------- | -------------------------------- | ----------- |
| `get_data`            | `getData(selectedFolder)`        | Returns content data: articles, dialogs, tDialogs, entities, conversation/context vars, files, sourceFiles, dataSource |
| `open_url`            | `openUrl(url)`                   | Opens a URL with `opener::open_url` (https/http only) |
| `open_preview_window` | `openPreviewWindow(url)`         | Opens a validated URL in an in-app preview window |
| `select_data_folder`  | `selectDataFolder()`             | Opens a native folder picker, returns `{ ok, canceled, path }` |
| `check_for_updates`   | `checkForUpdates()`              | Reads the release's `latest.json` through `tauri-plugin-updater`, returns `{ status, version, message, notes, canSelfUpdate, mode, blockedReason }`. See `docs/settings-and-updates.md` → "Self-update" |
| `install_update`      | `installUpdate()`                | Downloads the verified artifact, installs it, restarts. Portable Windows copies swap their own `.exe`; everything else uses the plugin's installer |
| `get_version`         | `getVersion()`                   | Returns the app version string from `package_info()` |
| `save_collection_export` | `saveCollectionExport(defaultName, content)` | Opens a native Save dialog (`.json` filter, defaulted filename) and writes `content` to the chosen path, returns `{ ok, canceled, path }`. A wrapper over `save_with_dialog` |
| `save_export_text`    | `saveExportText(defaultName, format, content)` | The same, for any format in `export_format` (`svg` `csv` `tsv` `html` `md` `txt` `json`). An unknown format is an `Err` *before* the dialog opens |
| `save_export_bytes`   | `saveExportBytes(defaultName, format, content)` | The binary half — `content` is a plain number array taken as `Vec<u8>`. Used for the 2× chart PNG; see `docs/insights.md` → "Saving to a file" |
| `export_settings_backup` | `exportSettingsBackup(defaultName, payload)` | Merges the Analytics API credentials into the renderer's payload and writes the backup (`0600`). See `docs/settings-and-updates.md` → "Settings backup" |
| `import_settings_backup` | `importSettingsBackup()`          | Picks a backup, restores the Analytics API credentials from it, returns everything else — `{ ok, canceled, settings, appVersion, schemaVersion, analyticsRestored }` |

| `get_analytics_config`     | `getAnalyticsConfig()`            | Analytics API settings **without the client secret** — returns `hasSecret` only |
| `save_analytics_config`    | `saveAnalyticsConfig(args)`       | Writes `analytics-api.json` to the app data dir (`0600`); a blank secret keeps the stored one |
| `test_analytics_connection`| `testAnalyticsConnection()`       | Requests an OAuth2 token only, returns `{ ok, message, trace }` |
| `fetch_analytics_window`   | `fetchAnalyticsWindow(startUtc, endUtc)` | Downloads one window to a temp CSV, returns `{ tempPath, delimiter, rowCount, bytes, durationMs, trace }`; rejects with `{ kind, message, retryable, trace }`. The `trace` is a step-by-step account carried on both outcomes — see `docs/import.md` → "Diagnosing a failed import" |
| `cleanup_analytics_temp`   | `cleanupAnalyticsTemp(paths?)`    | Deletes the given temp CSVs, or sweeps the whole temp dir when called with no argument |
| `get_db_hour_coverage`     | `getDbHourCoverage(sinceDate?)`   | Per UTC day, a bitmask of the 24 hours the day is **covered** for — the union of hours holding interactions and hours an API window explicitly requested. Distinguishes a partially imported day from a complete one. `sinceDate` bounds an otherwise full-table aggregate against `idx_timestamp`; the Import modal passes the retention floor, Manage Database omits it because its calendar browses everything stored |
| `record_imported_window`   | `recordImportedWindow(startUtc, endUtc)` | Marks every UTC hour a successfully imported API window covered. Called once per downloaded window, *after* its rows are in. See `docs/import.md` → "Coverage: asked-for vs present" |
| `begin_import_run`         | `beginImportRun()`                | Opens an import run: resets the touched-session set, sets the `pending_finalize` crash marker, raises `wal_autocheckpoint` |
| `finalize_import_run`      | `finalizeImportRun(maxAgeDays)`   | Closes a run: purge, scoped summary rebuild, FTS merge, planner stats, WAL restore — once, instead of once per file. Safe no-op when no run is open |
| `compact_database`         | `compactDatabase()`               | `VACUUM`s the database, returning pages freed by deletions and schema migrations to the filesystem. Returns `{ bytesBefore, bytesAfter, durationMs }` |
| `get_conversation_insights` | `getConversationInsights(args, unit, sections)` | Volume comes back as `dayHours` — one row per UTC day per UTC hour — which the renderer folds into the day series, the hour histogram and the heatmap *in the display timezone*. No SQL here knows what a timezone is. The Insights aggregates for the **sections that were asked for**, over the **same `GetSessionsArgs`** `get_sessions` takes. `unit` is `"conversations"` (default) or `"interactions"` — see `docs/insights.md` → "The two readings"; `sections` is `{volume, quality, content}` — see `docs/insights.md` → "The chooser". Never the two tag sections. Returns `null` when the read was cancelled |
| `release_insight_scope` | `releaseInsightScope()`         | Frees the resolved result set the Insights temp tables hold. Called when the modal closes — see `docs/insights.md` → "The result set is resolved once, not once per read" |
| `get_insight_tags`    | `getInsightTags(args, unit, keys)` | The Context and Metadata sections, read *after* the dashboard paints — see `docs/insights.md` → "Why it is two reads, not one". `keys` is `{context, metadata, contextOn, metadataOn}` — which key each section is charting, and whether it was asked for at all |
| `get_insight_tag_values` | `getInsightTagValues(args, unit, kind, name)` | One tag key's values, for switching the Context or Metadata chart |
| `cancel_db_query`     | `cancelDbQuery()`                | Interrupts whatever the conversations database is running — a session search or an Insights read. A no-op when nothing is running |

`get_entity_options` returns every entity the imported conversations have triggered — `{name, entityId, count}` — feeding both the conversation-search Entity pills and the only entity ids this app has (the EntitiesExport CSV carries none). See `docs/search.md` → "Entity pills are a filter, not a search".

There are also Conversations DB commands exposed through `window.backend` for importing CSV interaction logs, selecting/opening a SQLite database, searching sessions, loading chat interactions, context and metadata options (`get_context_options` / `get_metadata_options`, both thin wrappers over `tag_options`), daily stats, deleting imported dates, and managing flagged conversations/folders. Keep conversation search separate from content search.

`import_interactions_csv(filePath, maxAgeDays, delimiter?, deferFinalize?)` takes an optional single-character `delimiter`, defaulting to `|` (the portal export format). The Analytics API path sniffs the delimiter from the response header and passes it through; the manual path omits it. `deferFinalize` defaults to false; both real callers pass `true` and bracket their loop with `begin_import_run` / `finalize_import_run` — see `docs/import.md`.

`export_conversations_for_ai(args, options)` writes the current result set as `.jsonl`. `options.matchedTurnsOnly` narrows each conversation to the turns the search matched, reusing `SessionFilterQuery::match_rows`; it is a struct of its own rather than a field on `GetSessionsArgs`, which is the Insights scope-cache fingerprint. Its schema and the reasoning behind it are in `docs/ai-export.md`.

## Events (Rust → renderer)

| Event                 | Payload              | Description |
| --------------------- | -------------------- | ----------- |
| `data-folder-updated` | `{ reason, folder }` | Emitted by `notify` file watcher once the folder settles after export files change — see `One drop, one notification` |
| `ai-export-progress`  | `{ phase, sessionCount?, interactionCount? }` | Phase boundaries inside `export_conversations_for_ai` — `"querying"` once the save dialog is answered, `"writing"` once the result set is known |
| `update-progress`     | `{ phase, downloaded, total? }` | Download/install progress for `install_update`. `total` is absent when the server sends no `Content-Length` |
| `db-migrating`        | `{ phase, done? }`   | Phase updates for the one-time migrations `open_db` runs — `"answerIndex"`, `"contexts"` (with a running row count), `"compacting"`. Silent on every open that has nothing to migrate. See `docs/import.md` |

### One drop, one notification

The renderer answers `data-folder-updated` by opening a modal, so every extra event is a dialog the user has to dismiss again. An export drop is two or three files and each file reaches the watcher as several events — a create, one or more writes as the bytes land, then a metadata update — so the event stream has to be collapsed before it is emitted.

**It is a trailing-edge debounce, not a throttle, and that is the fix.** The old rule reported the *first* event immediately and then ignored everything for 700 ms — which meant a drop still being written produced a fresh notification every 700 ms for as long as it took. Now the watcher records the burst and a settle thread reports once the folder has been quiet for `WATCH_QUIET_PERIOD`.

- **`WATCH_MAX_WAIT` is the safety valve.** Each write pushes `last_seen` forward, so a folder that never goes quiet — a slow copy over a network share — would wait for silence forever. Reporting late beats never reporting; `a_folder_that_never_goes_quiet_still_reports` pins it.
- **`state.pending` doubles as "is a settle thread running?"**, which is what stops a stream of events spawning a thread each. The thread clears it under the same lock that decided to report, so a write landing at that instant starts a fresh burst instead of being folded into one already on its way out.
- **`generation` is bumped on every `configure_folder_watch`.** Without it, a settle thread left over from the previous folder would wake up, find the *new* folder's pending burst, and announce a change against the wrong directory.
- `burst_has_settled` is split out of the closure so the rule is testable; `notifications_for` in the tests replays a scripted event stream through both halves. `one_export_drop_is_one_notification` uses a realistic burst and returns 3 under the old throttle, so it is not vacuous — and `two_separate_drops_are_two_notifications` is its counterweight, since a debounce that swallowed the second drop would have replaced spam with silence.

---

## Frontend bridge (`index.html`)

`window.backend` is the renderer's sole interface to Rust. At startup, a shim in `index.html` wraps Tauri's `invoke` behind it:

```js
const invoke = window.__TAURI__?.core?.invoke
const listen = window.__TAURI__?.event?.listen
window.backend = {
  getData: (selectedFolder) =>
    invoke("get_data", { args: { selected_folder: selectedFolder || null } }),
  openUrl: (url) => invoke("open_url", { url }),
  openPreviewWindow: (url) => invoke("open_preview_window", { url }),
  selectDataFolder: () => invoke("select_data_folder"),
  onDataFolderUpdated: (handler) =>
    listen ? listen("data-folder-updated", handler) : Promise.resolve(() => {}),
  onDbMigrating: (handler) =>
    listen ? listen("db-migrating", handler) : Promise.resolve(() => {}),
  checkForUpdates: () => invoke("check_for_updates"),
  getVersion: () => invoke("get_version"),
  saveCollectionExport: (defaultName, content) =>
    invoke("save_collection_export", { defaultName, content }),
  fetchAnalyticsWindow: (startUtc, endUtc) =>
    invoke("fetch_analytics_window", { args: { startUtc, endUtc } }),
  // Conversations DB and Analytics API commands are also mapped here; keep them behind
  // window.backend rather than adding direct renderer filesystem access.
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

1. Data loads via `window.backend.getData(dataFolderPath)`
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

The semantics of all three — and of the Context · Metadata tag filters that
compose with them — are in **`docs/search.md`**. That file is the authority;
nothing about search inclusion should be inferred from the renderer.

## UI structure

The orientation map for the whole window. It says what is on screen and where;
*why* each part behaves as it does is in that area's doc — see
`Where the details live`.

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

<div.conv-toolbar>
  Data (opens #convDataModal) | Insights (opens #insightsModal) | Export for AI

<div#insightsModal>
  header row 1: hero count + what it counts | Conversations / Interactions
                toggle | Choose data | Copy dashboard | ✕
  header row 2: what the slice holds | chips describing the search this is
                | one quiet UTC badge
  body, on open — the chooser, and nothing is read until it is answered:
    what the current search matched
    Count: Conversations / Interactions, each with what it means
    Sections: Volume · Quality · Context · Metadata · Content, each with
              what it answers and what it costs (fast / medium / slower)
    Build N sections
  body, once built (one scrolling canvas, not tabs; section headings are sticky):
    stat tiles (conversations · interactions · median length · GenAI ·
                thumbs down · zero recognition · under threshold)
    Volume   — per day · by hour (UTC) · day × hour heatmap · length
    Quality  — lowest recognition score · feedback (conversations only)
               · how the answer was found · Dialog outcome
    Context  — one key button (searchable picker) + its values
    Metadata — one key button (searchable picker) + its values
    Content  — entities · opening questions (conversations only) ·
               Articles · Dialogs · Dialog nodes · cultures
    Not loaded — one button per section left out, to add it in place
  every card: title · what it counts · Image / Data copy buttons
  while reading: a Cancel button under the spinner; the unit toggle stays live

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
| `cm-display-timezone`      | IANA zone the chat, session list, date filter and Insights charts are read in. `""` or absent means follow the system. Import and Stored data stay UTC — see `docs/insights.md` → "Reading this in your own timezone" |
| `conv-data-retention-days` | CSV import retention window |
| `chat-copy-format`         | Chat copy format preference |
| `cm-collections`           | JSON array of `{ id, name, itemKeys, createdAt, updatedAt }`, plus optional `excludedItemKeys`, `excludedContent` and `disabledFilterIds` curation lists (all default `[]`) |
| `cm-export-keep-unreachable` | `"1"` to export non-default responses that have no context (or context `"any"`); anything else, including absent, keeps the default reachability rule on |
| `cm-insights-unit`         | `"interactions"` to open Insights counting matching interactions; anything else, including absent, counts conversations |
| `cm-insights-export-caption` | `"0"` to leave the caption off an exported chart image; anything else, including absent, includes it |
| `cm-insights-sections`     | JSON `{volume, quality, context, metadata, content}` — which sections the Insights chooser opens pre-selected. Read key by key, so an older or hand-edited file cannot introduce one; all-false falls back to the default |
| `cm-export-filters`        | JSON array of `{ id, field, pattern, isRegex, enabled }` (`field`: `"entity"` \| `"content"` \| `"context"`, missing = `"entity"`) — global smart-exclusion patterns for Collections export |

Analytics API credentials are deliberately **not** in localStorage — they live in `app_data_dir()/analytics-api.json`, written by Rust with `0600` perms, so the client secret never reaches the renderer. A settings backup does export them, and still does not break that rule; see `docs/settings-and-updates.md` → "Settings backup".

Example `cm-base-url` value: `https://www.cm.com/en-gb/app/aicloud/dbd80c7c-e9b1-44d2-9762-fb5ad1664b7f/Efteling/EFTELING/nl/`

---

## GitHub repository

GitHub account: **WithoutWout** (not `wouttonio`)
Repository: `WithoutWout/cm-conversation-dashboard`
Release URL pattern: `https://github.com/WithoutWout/cm-conversation-dashboard/releases/latest`

- Always use `WithoutWout` as the GitHub username, never `wouttonio`.
- `check_for_updates` reads `releases/latest/download/latest.json`, not the GitHub API — see `docs/settings-and-updates.md` → "Self-update".

---

## Coding conventions

- All HTML built via string concatenation — always use `esc()` for any dynamic value.
- CSS variables for theming: `--bg`, `--surface`, `--surface2`, `--border`, `--text`, `--muted`, `--accent`, `--green`, `--blue`, `--orange`, `--red`, `--teal`.
- Internal identifiers (`_kind`, `tDialogMap`, `b-tdialog`, CSS class `type-tdialog`) use the short `tdialog`/`tDialog` form — only the user-facing label says "Transactional Dialog".
- Use `querySelector` / `getElementById` for DOM access; event delegation where multiple dynamic elements share a handler.
- `buildSearchRegex` is the single source of truth for search logic — do not duplicate regex construction elsewhere.
- Inline `onclick="..."` attributes are used intentionally for dynamically rendered cards (no event listener cleanup needed in this app).
- Rust commands use `snake_case`; the JS shim maps them to `camelCase` on `window.backend`.

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
