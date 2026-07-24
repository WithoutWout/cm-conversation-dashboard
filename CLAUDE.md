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
  tauri.conf.json   — App config, window setup, frontendDist: ../frontend
  Cargo.toml        — Rust dependencies (tauri, serde, reqwest, notify, tauri-plugin-opener, tauri-plugin-dialog)
  capabilities/
    default.json    — Capability grants: core:default, opener:default, dialog:default
frontend/
  index.html        — Entire renderer: HTML + embedded <style> + embedded <script>
  search-worker.js  — Worker-side content/entity filtering, sorting, and search matching
package.json        — scripts: tauri dev / tauri build
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
| `check_for_updates`   | `checkForUpdates()`              | Fetches GitHub releases API, returns `{ status, version, message }` |
| `get_version`         | `getVersion()`                   | Returns the app version string from `package_info()` |
| `save_collection_export` | `saveCollectionExport(defaultName, content)` | Opens a native Save dialog (`.json` filter, defaulted filename) and writes `content` to the chosen path, returns `{ ok, canceled, path }` |

| `get_analytics_config`     | `getAnalyticsConfig()`            | Analytics API settings **without the client secret** — returns `hasSecret` only |
| `save_analytics_config`    | `saveAnalyticsConfig(args)`       | Writes `analytics-api.json` to the app data dir (`0600`); a blank secret keeps the stored one |
| `test_analytics_connection`| `testAnalyticsConnection()`       | Requests an OAuth2 token only, returns `{ ok, message }` |
| `fetch_analytics_window`   | `fetchAnalyticsWindow(startUtc, endUtc)` | Downloads one window to a temp CSV, returns `{ tempPath, delimiter, rowCount, bytes, durationMs }`; rejects with `{ kind, message, retryable }` |
| `cleanup_analytics_temp`   | `cleanupAnalyticsTemp(paths?)`    | Deletes the given temp CSVs, or sweeps the whole temp dir when called with no argument |
| `get_db_hour_coverage`     | `getDbHourCoverage()`             | Per UTC day, a bitmask of which of the 24 hours hold interactions — distinguishes a partially imported day from a complete one |

There are also Conversations DB commands exposed through `window.electronAPI` for importing CSV interaction logs, selecting/opening a SQLite database, searching sessions, loading chat interactions, context options, daily stats, deleting imported dates, and managing flagged conversations/folders. Keep conversation search separate from content search.

`import_interactions_csv(filePath, maxAgeDays, delimiter?)` takes an optional single-character `delimiter`, defaulting to `|` (the portal export format). The Analytics API path sniffs the delimiter from the response header and passes it through; the manual path omits it.

## Events (Rust → renderer)

| Event                 | Payload              | Description |
| --------------------- | -------------------- | ----------- |
| `data-folder-updated` | `{ reason, folder }` | Emitted by `notify` file watcher when export files change |

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
| `renderDialogCard(item, q)`       | Full dialog/tDialog card HTML with expandable node list |
| `renderNodeHtml(node, dialog, q)` | Individual node HTML: Recognition/Output badge, answer, user options, routing |
| `applyAllFilters()`               | Lightweight wrapper that triggers worker search for All Results |
| `applyArticleFilters()`           | Lightweight wrapper that triggers worker search for Articles |
| `applyDialogFilters()`            | Lightweight wrapper that triggers worker search for Dialogs |
| `applyEntityFilters()`            | Lightweight wrapper that triggers worker search for Entities |
| `jumpToDialog(id, isTDialog)`     | Switches to Dialogs tab, sets search to the ID, scrolls to and opens the matching card |
| `openExportModal()`               | Opens Share Content using the current active tab's filtered items |
| `_renderExportGrouped(items)`     | Groups Share Content by Articles, Dialogs, Transactional Dialogs, sorted by id, with dialog → article refs |
| `buildItemUrl(kind, id)`          | Returns full CM.com URL for an item |
| `toggleContentSelectMode()`       | Toggles Collections multi-select on the Content tab, re-rendering the active panel with/without checkboxes |
| `buildCollectionExportRows(collection)` | Walks a collection's items, applies reachability + smart-filter exclusion, returns `{ rows, excludedCount, totalCandidates }` |
| `openCollectionsModal()`          | Opens the Collections modal (Collections list + Smart Filters tabs) |
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
- Modal "Show search-matching content only" should use the same answer/node sections that caused worker result inclusion.

---

## Analytics API import

The Conversations toolbar's **Import** button opens `#convImportModal`, which offers two sources that both end at the same place: **Analytics API** (automated) and **CSV file** (the original manual `doImport()` path, unchanged). `CM_Analytics_API_SOP.md` is authoritative for anything about the API itself.

Responsibilities are split deliberately — keep them separate when extending this:

| Layer | Where | Owns |
| ----- | ----- | ---- |
| UI | `index.html` (`_impRenderModal` and friends) | modal, calendar, progress, cancel/retry |
| Scheduler | `index.html` (`buildImportQueue`, `_impRunQueue`, `_impFetchWindow`) | queueing, pipelining, subdivision, cancellation |
| API client | `analytics_api.rs` | token cache, one-at-a-time fetch, response validation, temp files |
| Import service + DB | `import_interactions_csv` (unchanged) | parsing, dedupe, FTS, context index, session summary |

- **The API returns the same CSV the manual workflow downloads.** The API path writes the response to a temp `.csv` and hands it to `import_interactions_csv`, so there is exactly one import pipeline and one duplicate-detection mechanism. Do not add a parallel importer.
- **Duplicate detection is `INSERT OR IGNORE` on the `log_id` primary key**, so re-importing a day is always safe and idempotent. Skipping already-imported days is therefore an optimisation, not a correctness requirement — never build a separate dedupe mechanism.
- **The database stores raw UTC** (the portal CSV's `03/25/2026 09:30:22` is the same instant as the API's `2026-03-25T09:30:22.605Z`), so `get_db_daily_stats` groups by UTC date. `parse_ts` normalizes both formats to `YYYY-MM-DDTHH:MM:SS` so rows from either source are byte-identical — every `DATE(timestamp_start)` and range comparison depends on that.
- **Windowing:** the picker is local time (**Now** = local now), but `buildImportQueue` snaps chunks to UTC days so each request maps 1:1 to a DB day. A full day is `00:00:00Z` → `23:59:59Z` — **strictly under 24h**, because a span of exactly 24 hours is rejected. `validate_window` in `analytics_api.rs` enforces this, along with the SOP's 90-day retention limit. A local range straddles UTC-day boundaries, so picking one local day legitimately produces two chunks.
- **Pipeline:** while day *N* imports, day *N+1* downloads. Only ever one API request is in flight — the JS scheduler serialises downloads and a `tokio::sync::Semaphore(1)` in `AnalyticsState` enforces it at the client layer regardless. `_impStartFetch` returns a promise that never rejects (`{ ok, parts | error }`) because a download is started one iteration before it is awaited.
- **Timeout subdivision:** the SOP warns full-day requests often time out. On a retryable error the window is halved (12h → 6h → …), sequentially, bounded by `IMP_MAX_SPLIT_DEPTH` and a one-hour floor — a window is only split while *both* halves would stay at or above an hour. Worst case is ~6 requests per day, not an exponential fan-out.
- **`paginateData` is deliberately not sent.** The SOP requires confirming the pagination mechanism first, so instead the client fails loudly on anything paginated-looking rather than importing a partial day. Confirm the mechanism against the official spec before implementing it.
- **Temp files** live in `app_cache_dir()/analytics-tmp` and are deleted the moment each part's import returns, in a `finally` so failure and cancellation clean up too. The dir is swept on app start and on modal open (crash recovery). `cleanup_analytics_temp` is path-confined to that directory.
- **Credentials** live in `app_data_dir()/analytics-api.json` (`0600` on unix). The client secret never crosses the IPC bridge — `getAnalyticsConfig` returns `hasSecret` only, and saving with a blank secret keeps the stored one.
- **Skipping is decided per hour, not per day** (`get_db_hour_coverage` → `_impWindowCovered`). Because a local-time range leaves a *partial* UTC day behind (e.g. only the 22:00–23:59 tail), a day-level "has rows?" check would silently skip that day's other 22 hours. A chunk is skipped only when every UTC hour it touches already holds data. The calendar shows this in three states: green outline = every hour imported, orange outline = partly imported (will be fetched again), no outline = nothing yet. Never regress this to a day-level check.
- Skipped days stay in the queue marked `skipped` rather than being dropped, so the user can see what was left alone; they count toward overall progress.
- Caveat worth knowing: an hour with genuinely zero interactions reads as "missing", so a quiet night can make a complete day look partial and be re-fetched. That errs toward re-downloading, which `INSERT OR IGNORE` makes harmless — the opposite error would lose data.

## Collections

Lets users multi-select Articles/Dialogs on the Content tab and export them as `[{ trigger, content }]` JSON for CM.com HALO's knowledge tool.

- **Selection**: a toggleable "Select" mode (`collectionSelectMode`, `#contentSelectModeBtn`) reveals a checkbox on Article/Dialog cards (not Transactional Dialogs — they have no `nodes`/content of their own). Selection state is `collectionSelection`, a `Set` of stable keys (`"article:<Id>"` / `"dialog:<id>"`), read back via `.has(key)` at HTML-string-build time inside `renderArticleCard`/`renderDialogCard` — required because every card list is fully rebuilt via `innerHTML =` on every search/filter/sort/pagination change, so DOM-attached state would not survive. "Select page" (`selectAllVisibleContent`) only adds the checkboxes currently rendered in the DOM; "Select all" (`selectAllFilteredContent`) instead walks the active tab's full `filteredArticles`/`filteredDialogs`/`filteredAll` index buffer — the same current search/filter result set `getActiveExportItems()` uses for Share Content — so it selects every matching item across all pages, not just the visible one.
- **Collections** (`cmCollections`, `localStorage["cm-collections"]`) are named groups of item keys, created/extended via the "+ Add to Collection" popover in the select bar. Managed (rename/delete/view items/export) via the Collections modal (`#collectionsBtn`).
- **Export algorithm** (`buildCollectionExportRows(collection)`, and its per-kind helpers `_articleExportRows`/`_dialogExportRows`): for each selected item, emits one row per *reachable* Answer — the default answer, plus every non-default answer that has real context (reusing `articleAnswerHasContext`/`dialogAnswerHasContext` — the same reachability rule as `## Content search semantics`). An item can legitimately contribute 0 rows: Articles that route into a Dialog/TDialog instead of answering directly, or dialog nodes whose Recognition links only lead to other routing-only nodes (common in real data — e.g. a dialog can be entirely a router into other dialogs). The Collections modal surfaces this rather than failing silently.
- For dialogs, a trigger comes from either of two sources, both resolved to reachable Answer item(s) on a **target** node via the shared `emitReachableAnswers` step in `_dialogExportRows`:
  - a non-fallback Recognition link's `condition.data.questions[]`, targeting `link.childNodeId` (mid-conversation, internal to the dialog); or
  - a referencing **Article**'s `Questions[]`, via `_articlesRoutingIntoDialog(dialogId)` — any Article with a reachable `DialogStart` Output (`DialogId` matching, `IsDefault` or has real context, same reachability rule as Answer outputs) targeting `DialogStartNodeId` (the dialog's entry point). This runs against the full loaded dataset regardless of whether that article is itself in the collection, since it only supplies the human-readable trigger phrase for content the dialog otherwise has no entity attached to. A dialog that is purely an internal router (every Recognition link only leads to further `DialogStart` hand-offs, never a direct Answer) can still produce real export rows this way — confirmed against production data.
- Multiple trigger phrases on one row are joined with `" | "` (e.g. `"Entity | Other Entity"`) — an Article's full `Questions[]` list can be large (dozens of phrases) since every entity that reaches that Article funnels into the same dialog entry.
- **Smart filters** (`cmExportFilters`, `localStorage["cm-export-filters"]`) are global, user-managed exclusion patterns (plain case-insensitive substring by default, or regex per-pattern) applied at export time via `_rowMatchesExclusion(row, patterns)`. Matching is whole-row: if any tested value on a row matches an enabled pattern, the entire row is dropped. Each pattern has a `field` (`"entity"` default | `"content"` | `"context"`, chosen via a `<select class="sort-select">` in the Smart Filters tab) selecting what gets tested: Entity checks each trigger phrase (`row.phrases`, original behavior); Content checks the answer text (`row.content`); Context checks a flattened, sorted `"name:val1,val2 ..."` string built by `_rowContextText(contextVars, escGroup, isArticle)` from the same `ContextVariables`/`contextVariables` + escalation-group fields `articleAnswerHasContext`/`dialogAnswerHasContext` already read for reachability (resolved to readable names via `ctxVarMap`, mirroring — without touching — the `ctxSet` normalization inside `answerPassesContextFilters`). Filters saved before `field` existed have no `field` key and default to `"entity"` for backward compatibility.
- **Merging** (`_mergeRowsByContent`, called inside `buildCollectionExportRows` after exclusion filtering, before the final `trigger`/`content` rows are built): rows with byte-identical `content` — regardless of source (two Articles, an Article and a dialog node, two dialog nodes, etc.) — are combined into one row, unioning their trigger phrases (deduped, first-seen order). Runs *after* exclusion so a smart-filter-dropped row's phrases never leak into a surviving row just because they happened to share content.
- `esc()` must **not** be applied to `trigger`/`content` values — that's for `innerHTML` rendering; `JSON.stringify` handles export escaping.
- `buildCollectionExportRows(collection, opts)` returns `{ rows, excludedRows, excludedCount, totalCandidates }`. `excludedRows` (unmerged — one entry per raw exclusion event, not deduped) is `{ trigger, content, matchedFields }[]`, where `matchedFields` is `["<field>: <pattern>", ...]` from `_rowMatchingPatterns(row, patterns)` (the patterns that matched, which `_rowMatchesExclusion` just checks the length of). This powers the Collections modal's **Filtered Out** tab (`renderCollectionsExcludedBody`, `#collectionsExcludedBody`) — a per-collection picker (`_collectionsExcludedViewId`) over what a currently-enabled smart filter is dropping and why, so a filter meant to catch one thing doesn't silently eat something else too.
- **"View content"** (`toggleCollectionContentView`/`_renderCollectionContentList`) is a per-collection, live-searchable preview of the collection's actual computed export rows (post-reachability, post-exclusion, post-merge) — distinct from **"View items"**, which shows/manages the raw source Articles/Dialogs. The search `<input>` is only built once per panel-open; typing re-renders just the results list underneath it (`#collection-content-list-<id>`), not the input itself, so the cursor position isn't lost mid-edit the way a full-panel `innerHTML` rebuild on every keystroke would. Matches highlight via the existing `hl()` helper, searching both `trigger` and `content`.

---

## UI structure

```
<header>
  brand | file tags | Export IDs button | Collections button | Settings button (gear)

<div.global-search-bar>
  search input | [Aa] [\b] [.*] [¬T] [ND] | context filter button

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
  entity list | pagination

<div#settingsModal>
  Content tab: CM.com Context URL input, Open CM.com links radio (popup / browser)
  Conversations tab: Halo Studio URL, low recognition threshold, chat copy format,
                     Analytics API (client ID / secret / customer key / project key /
                     culture / environment / activeSessionOnly / Test connection)

<div#convImportModal>
  Source tabs (Analytics API / CSV file)
  Setup:   From + To date fields (click to choose which end you're picking) and
           time inputs | Now shortcut
           always-visible two-month calendar — green outline = fully imported,
           orange outline = partly imported
           summary (N days · M fully imported · K to download, UTC request window)
           "Skip days already imported in full" checkbox
  Running: current operation | progress bar | "N of M days completed"
           per-day list with status chips | collapsible Details log
           Cancel import — or, when stopped, Retry/Resume from <date>

<div#exportModal>
  List / Table / Grouped tabs | copy as links / table / plain text

<div#collectionsModal>
  Collections / Smart Filters / Filtered Out tabs
  Collections: name, item/row counts, View items / View content (searchable) / Rename / Export / Delete
  Smart Filters: field selector (Entity/Content/Context) + pattern + regex-flag add row, list with Field/Regex/Enabled toggles
  Filtered Out: collection picker, list of excluded rows with which pattern(s) matched each
```

Content result relationship displays:

- Article cards show clickable Dialog Link / Transactional Dialog chips inline; avoid separate "Directs to ..." text when the target can be part of the chip.
- Dialog cards can show "Uses articles" relationship rows with clickable `qa-...` chips.
- Share Content `Grouped` view always groups by Articles, Dialogs, Transactional Dialogs, then sorts by id. Dialog rows that reference articles should visibly read as dialog → article relationships, e.g. `dn-123 -> qa-456`, with clickable chips in the UI.

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
| `cm-collections`           | JSON array of `{ id, name, itemKeys, createdAt, updatedAt }` |
| `cm-export-filters`        | JSON array of `{ id, field, pattern, isRegex, enabled }` (`field`: `"entity"` \| `"content"` \| `"context"`, missing = `"entity"`) — global smart-exclusion patterns for Collections export |

Analytics API credentials are deliberately **not** in localStorage — they live in `app_data_dir()/analytics-api.json`, written by Rust with `0600` perms, so the client secret never reaches the renderer.

Example `cm-base-url` value: `https://www.cm.com/en-gb/app/aicloud/dbd80c7c-e9b1-44d2-9762-fb5ad1664b7f/Efteling/EFTELING/nl/`

---

## GitHub repository

GitHub account: **WithoutWout** (not `wouttonio`)
Repository: `WithoutWout/cm-conversation-dashboard`
Release URL pattern: `https://github.com/WithoutWout/cm-conversation-dashboard/releases/latest`

- Always use `WithoutWout` as the GitHub username, never `wouttonio`.
- The `check_for_updates` Tauri command fetches `api.github.com/repos/WithoutWout/cm-conversation-dashboard/releases/latest`.

---

## Coding conventions

- All HTML built via string concatenation — always use `esc()` for any dynamic value.
- CSS variables for theming: `--bg`, `--surface`, `--surface2`, `--border`, `--text`, `--muted`, `--accent`, `--green`, `--blue`, `--orange`, `--red`, `--teal`.
- Internal identifiers (`_kind`, `tDialogMap`, `b-tdialog`, CSS class `type-tdialog`) use the short `tdialog`/`tDialog` form — only the user-facing label says "Transactional Dialog".
- Use `querySelector` / `getElementById` for DOM access; event delegation where multiple dynamic elements share a handler.
- `buildSearchRegex` is the single source of truth for search logic — do not duplicate regex construction elsewhere.
- Inline `onclick="..."` attributes are used intentionally for dynamically rendered cards (no event listener cleanup needed in this app).
- Rust commands use `snake_case`; the JS shim maps them to `camelCase` on `window.electronAPI`.
