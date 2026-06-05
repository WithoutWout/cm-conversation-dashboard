# GitHub Copilot Instructions

## Project overview

Tauri desktop dashboard for inspecting and navigating CM.com Conversational AI Cloud content exports. It reads two JSON files from a user-selected folder and renders a searchable, filterable UI in a single window.

**Stack:** Tauri v2 (Rust backend + vanilla JS frontend), vanilla JS, no bundler, no framework.

Libraries may be used, but must be vendored locally (e.g. `frontend/vendor/`) so the app works fully offline. Never load dependencies from a CDN.

---

## File structure

```
src-tauri/
  src/
    lib.rs     — Tauri commands for content data, links, updates, and Conversations DB features
    main.rs    — Entry point, calls lib::run()
  tauri.conf.json — App config, window setup, frontendDist: ../frontend
  Cargo.toml  — Rust dependencies (tauri, serde, reqwest, notify, tauri-plugin-opener, tauri-plugin-dialog)
  capabilities/
    default.json — Capability grants: core:default, opener:default, dialog:default
frontend/
  index.html  — Entire renderer: HTML + embedded <style> + embedded <script>
  search-worker.js — Worker-side content/entity filtering, sorting, and search matching
package.json  — scripts: tauri dev / tauri build
```

Data files (read-only, never committed, placed in a user-selected folder):

- `*ArticlesExport*.json` — matched by pattern `"ArticlesExport"`
- `*DialogsExport*.json` — matched by pattern `"DialogsExport"`
- `*EntitiesExport*.csv` — matched when present for Entities enrichment/search

---

## Tauri security rules

- The renderer has no direct Node or filesystem access — all backend calls go through Tauri commands via `window.__TAURI__.core.invoke()`.
- `withGlobalTauri: true` is set in `tauri.conf.json`, making `window.__TAURI__` available.
- `open_url` only calls `opener::open_url` after validating that the URL starts with `https://` or `http://`.
- Never add new Tauri commands without validating input on the Rust side.
- Capability grants in `capabilities/default.json` must be kept minimal.

---

## Tauri commands (Rust → JS)

| Command              | JS call via `window.electronAPI` | Description                                                                     |
| -------------------- | -------------------------------- | ------------------------------------------------------------------------------- |
| `get_data`           | `getData(selectedFolder)`        | Returns content data: articles, dialogs, tDialogs, entities, conversation/context vars, files, sourceFiles, dataSource |
| `open_url`           | `openUrl(url)`                   | Opens a URL with `opener::open_url` (https/http only)                           |
| `open_preview_window`| `openPreviewWindow(url)`         | Opens a validated URL in an in-app preview window                               |
| `select_data_folder` | `selectDataFolder()`             | Opens a native folder picker, returns `{ ok, canceled, path }`                  |
| `check_for_updates`  | `checkForUpdates()`              | Fetches GitHub releases API, returns `{ status, version, message }`             |
| `get_version`        | `getVersion()`                   | Returns the app version string from `package_info()`                            |

There are also Conversations DB commands exposed through `window.electronAPI` for importing CSV interaction logs, selecting/opening a SQLite database, searching sessions, loading chat interactions, context options, daily stats, deleting imported dates, and managing flagged conversations/folders. Keep conversation search separate from content search.

## Events (Rust → renderer)

| Event                 | Payload              | Description                                               |
| --------------------- | -------------------- | --------------------------------------------------------- |
| `data-folder-updated` | `{ reason, folder }` | Emitted by `notify` file watcher when export files change |

---

## Frontend bridge (`index.html`)

The renderer uses `window.electronAPI` as its sole interface to the backend. At startup, a shim in `index.html` wraps Tauri's `invoke` behind `window.electronAPI`:

```js
// Wraps Tauri invoke() behind the window.electronAPI surface
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
  // Conversations DB commands are also mapped here; keep them behind
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

Total: ~66 items. These are **Transactional Dialogs** — always use this term in the UI; never "Transfer Dialog".

---

## CM.com URL patterns

Base URL constant (in `index.html`):

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
let gQuery = "" // current search query string
let searchCase = false // Aa toggle
let searchWord = false // \b toggle
let searchRegex = false // .* toggle
let searchContent = false // ¬T toggle — when true, search responses only
let searchExcludeNonDefault = false // ND toggle — excludes non-default response matches only when a query is active
let allFilterPill = "all" // filter in All Results tab
let aFilter = "all" // filter in Articles tab
let dFilter = "all" // filter in Dialogs tab
let allSort, aSort, dSort // persisted content sort choices
let allPage, aPage, dPage // current pagination pages
let allArticles = [] // raw article data
let allDialogsCombined = [] // dialogs only (no tDialogs)
let allCombinedItems = [] // articles + dialogs + tDialogs merged (each with _kind)
let filteredAll, filteredArticles, filteredDialogs // Int32Array result indexes
let allEntities = []
let filteredEntities // Int32Array entity result indexes
let matchingEntityNames = new Set() // entity names matched by current search query
let dialogMap = new Map() // id → dialog object
let tDialogMap = new Map() // id → tDialog object
let articleMap = new Map() // id → article object
let cmBaseUrl // CM.com context URL
let haloBaseUrl // HALO/other context URL for conversation links
let openMode // "popup" | "browser"
let otherOpenMode // "popup" | "browser"
```

### Key functions

| Function                          | Purpose                                                                                                    |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `buildSearchRegex(q)`             | Renderer highlight regex builder; worker has the authoritative search compiler                             |
| `hl(text, q)`                     | HTML-escapes text and wraps matches in `<mark>`                                                            |
| `esc(s)`                          | HTML-escapes a string (use for all dynamic content inserted into innerHTML)                                |
| `strip(t)`                        | Strips HTML tags from text                                                                                 |
| `aKind(a)`                        | Returns `"dialog"`, `"tdialog"`, or `"plain"` for an article based on Outputs                              |
| `triggerSearch()`                 | Sends the current query, filters, toggles, context filters, and sort choices to `search-worker.js`          |
| `handleSearchResults(msg)`        | Receives worker result index arrays, updates counts/pagination, and lazily renders the active tab          |
| `cmLink(type, id)`                | Returns an `<a class="action-link">` HTML string; `type` is `"article"` or `"dialog"`                      |
| `articleDialogLinkBadges(links)`  | Renders clickable Dialog Link/Transactional Dialog chips for article cards                                 |
| `dialogLinkedArticles(item)`      | Finds Articles that link to a Dialog for card/export relationship displays                                 |
| `renderArticleCard(art, q)`       | Full article card HTML with badges, expandable questions, output section                                   |
| `renderDialogCard(item, q)`       | Full dialog/tDialog card HTML with expandable node list                                                    |
| `renderNodeHtml(node, dialog, q)` | Individual node HTML: Recognition/Output badge, answer, user options, routing                              |
| `applyAllFilters()`               | Lightweight wrapper that triggers worker search for All Results                                            |
| `applyArticleFilters()`           | Lightweight wrapper that triggers worker search for Articles                                               |
| `applyDialogFilters()`            | Lightweight wrapper that triggers worker search for Dialogs                                                |
| `applyEntityFilters()`            | Lightweight wrapper that triggers worker search for Entities                                               |
| `jumpToDialog(id, isTDialog)`     | Switches to Dialogs tab, sets search to the ID, scrolls to and opens the matching card                     |
| `openExportModal()`               | Opens Share Content using the current active tab's filtered items                                          |
| `_renderExportGrouped(items)`     | Groups Share Content by Articles, Dialogs, Transactional Dialogs, sorted by id, with dialog -> article refs |
| `buildItemUrl(kind, id)`          | Returns full CM.com URL for an item                                                                        |

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

There are three distinct search types in the app:

1. **Content search** — Searches Dialogs and Articles and their content. This is the main search bar under the Content tab.
2. **Conversations search** — Searches conversations and their context (e.g. filter by context). This search can be very resource-intensive and should be treated accordingly (e.g. debounce, lazy loading, worker offloading, only load necessary data when user presses the search button or 'Enter').
3. **Chat search** — Searches within a single chat. A chat is first found and opened via the Conversations search; the Chat search then operates within that opened conversation.

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

## UI structure

```
<header>
  brand | file tags | Export IDs button | Settings button (gear)

<div.global-search-bar>
  search input | [Aa] [\b] [.*] [¬T] [ND] | context filter button

<div.tab-bar>
  All Results (with sub-stats: art · dlg · t.dlg)
  | Articles (with sub-stats: resp · dlg-lnk)
  | Dialogs (with sub-stats: dlg · t.dlg · nodes · recog)
  | Entities (with sub-stats: entities · words)

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
  CM.com Context URL input
  Other/HALO URL input
  Open CM.com links: radio (popup / browser)

<div#exportModal>
  List / Table / Grouped tabs | copy as links / table / plain text
```

Content result relationship displays:

- Article cards show clickable Dialog Link / Transactional Dialog chips inline; avoid separate "Directs to ..." text when the target can be part of the chip.
- Dialog cards can show "Uses articles" relationship rows with clickable `qa-...` chips.
- Share Content `Grouped` view always groups by Articles, Dialogs, Transactional Dialogs, then sorts by id. Dialog rows that reference articles should visibly read as dialog -> article relationships, for example `dn-123 -> qa-456`, with clickable chips in the UI.

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

| Key                    | Value                                  |
| ---------------------- | -------------------------------------- |
| `cm-base-url`          | CM.com context URL override (string)   |
| `halo-base-url`        | HALO/other context URL override (string) |
| `cm-open-mode`         | `"popup"` or `"browser"`               |
| `cm-other-open-mode`   | `"popup"` or `"browser"`               |
| `cm-dismissed-version` | Last update version the user dismissed |
| `cm-data-folder`       | Last selected content export folder    |
| `cm-sort-all`          | All Results sort choice                |
| `cm-sort-articles`     | Articles sort choice                   |
| `cm-sort-dialogs`      | Dialogs sort choice                    |
| `cm-flow-direction`    | Dialog graph layout direction          |
| `cm-view`              | Last selected main view                |
| `conv-db-path`         | Last selected conversations database   |
| `conv-low-recog-threshold` | Low recognition threshold          |
| `conv-data-retention-days` | CSV import retention window        |
| `chat-copy-format`     | Chat copy format preference            |

---

Example `cm-base-url` value: https://www.cm.com/en-gb/app/aicloud/dbd80c7c-e9b1-44d2-9762-fb5ad1664b7f/Efteling/EFTELING/nl/

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
