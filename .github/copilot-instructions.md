# GitHub Copilot Instructions

## Project overview

Electron desktop dashboard for inspecting and navigating CM.com Conversational AI Cloud content exports. It reads two JSON files from the project root and renders a searchable, filterable UI in a single BrowserWindow.

**Stack:** Electron (main + preload + renderer), vanilla JS, no bundler, no framework, no external CSS.

---

## File structure

```
main.js        — Electron main process: IPC handlers, BrowserWindow setup
preload.js     — contextBridge: exposes electronAPI to renderer
index.html     — Entire renderer: HTML + embedded <style> + embedded <script>
package.json   — entry: main.js, devDep: electron
```

Data files (read-only, never committed, placed in project root):

- `*ArticlesExport*.json` — matched by `findFile("ArticlesExport")`
- `*DialogsExport*.json` — matched by `findFile("DialogsExport")`

---

## Electron security rules

- `nodeIntegration: false` and `contextIsolation: true` — never change these.
- All Node/Electron access goes through the contextBridge in `preload.js`.
- `shell.openExternal` is only called after validating that the URL starts with `https?://`.
- Never add new IPC channels without validating input on the main-process side.

---

## IPC channels

| Channel    | Direction       | Description                                                                   |
| ---------- | --------------- | ----------------------------------------------------------------------------- |
| `get-data` | renderer → main | Returns `{ articles[], dialogs[], tDialogs[], files: { articles, dialogs } }` |
| `open-url` | renderer → main | Opens a URL with `shell.openExternal` (https only)                            |

---

## Data schemas

### ArticlesExport (`data.Articles[]`)

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

### DialogsExport (`data.dialogs.result[]`)

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

### tDialogs (`data.tDialogs[]` or `data.tDialogs.result[]`)

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

`cmBaseUrl` is read from `localStorage["cm-base-url"]` or falls back to `CM_DEFAULT_URL` (empty string). CM.com links are only rendered when a context URL has been configured in Settings.

---

## Renderer architecture (`index.html`)

Single `<script>` block at the bottom. No modules. All state is module-level `let`.

### State variables

```js
let gQuery = "" // current search query string
let searchCase = false // Aa toggle
let searchWord = false // \b toggle
let searchRegex = false // .* toggle
let searchContent = true // ¬T toggle — skip IDs, names, entities; match response/answer text only (on by default)
let allFilterPill = "all" // filter in All Results tab
let aFilter = "all" // filter in Articles tab
let dFilter = "all" // filter in Dialogs tab
let allPage, aPage, dPage // current pagination pages
let allArticles = [] // raw article data
let allDialogsCombined = [] // dialogs only (no tDialogs)
let allCombinedItems = [] // articles + dialogs + tDialogs merged (each with _kind)
let filteredAll,
  filteredArticles,
  filteredDialogs = []
let dialogMap = new Map() // id → dialog object
let tDialogMap = new Map() // id → tDialog object
let cmBaseUrl // CM.com context URL
let openMode // "popup" | "browser"
```

### Key functions

| Function                          | Purpose                                                                                                    |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `buildSearchRegex(q)`             | Central regex builder; respects `searchCase`, `searchWord`, `searchRegex`; returns `null` on invalid regex |
| `hl(text, q)`                     | HTML-escapes text and wraps matches in `<mark>`; uses `g` flag only here                                   |
| `esc(s)`                          | HTML-escapes a string (use for all dynamic content inserted into innerHTML)                                |
| `strip(t)`                        | Strips HTML tags from text                                                                                 |
| `aKind(a)`                        | Returns `"dialog"`, `"tdialog"`, or `"plain"` for an article based on Outputs                              |
| `matchArticle(a, q)`              | Tests article against current search regex across Id, Questions, Outputs                                   |
| `matchDialog(item, q)`            | Tests dialog/tDialog against regex across id, name, description, node content                              |
| `cmLink(type, id)`                | Returns an `<a class="action-link">` HTML string; `type` is `"article"` or `"dialog"`                      |
| `renderArticleCard(art, q)`       | Full article card HTML with badges, expandable questions, output section                                   |
| `renderDialogCard(item, q)`       | Full dialog/tDialog card HTML with expandable node list                                                    |
| `renderNodeHtml(node, dialog, q)` | Individual node HTML: Recognition/Output badge, answer, user options, routing                              |
| `applyAllFilters()`               | Filters `allCombinedItems` → `filteredAll`, re-renders All Results panel                                   |
| `applyArticleFilters()`           | Filters `allArticles` → `filteredArticles`, re-renders Articles panel                                      |
| `applyDialogFilters()`            | Filters `allDialogsCombined + tDialogs` → `filteredDialogs`, re-renders Dialogs panel                      |
| `jumpToDialog(id, isTDialog)`     | Switches to Dialogs tab, sets search to the ID, scrolls to and opens the matching card                     |
| `openExportModal()`               | Context-aware: reads current active tab's filtered items, builds Jira-ready text                           |
| `buildItemUrl(kind, id)`          | Returns full CM.com URL for an item                                                                        |
| `triggerSearch()`                 | Called on every search input change; updates `gQuery`, calls all three `apply*` functions                  |

### Rendering pipeline

1. Data loads via `window.electronAPI.getData()`
2. Maps (`dialogMap`, `tDialogMap`) populated
3. Combined item arrays assembled (each item gets `._kind = "article" | "dialog" | "tdialog"`)
4. `applyAllFilters()` / `applyArticleFilters()` / `applyDialogFilters()` called
5. Each renders its paginated slice using `renderArticleCard` / `renderDialogCard`

### Pagination

- Page size: `PAGE_SIZE = 50`
- `pagHtml(cur, total, callbackName)` renders numbered page buttons
- Pagination links use `onclick="goAllPage(n)"` etc. (inline handlers, intentional)

---

## UI structure

```
<header>
  brand | file tags | Export IDs button | Settings button (gear)

<div.global-search-bar>
  search input | [Aa] [\\b] [.*] toggle buttons

<div.tab-bar>
  All Results | Articles | Dialogs

<div#panel-all>
  stats (Total / Articles / Dialogs / Transactional Dialogs)
  filter pills (All / Articles / Dialogs / Transactional Dialogs)
  item list | pagination

<div#panel-articles>
  stats (Articles / Has Response / Dialog Link)
  filter pills (All / Has response / Dialog link)
  item list | pagination

<div#panel-dialogs>
  stats (Dialogs / Transactional Dialogs / Nodes / Recognition Nodes)
  filter pills (All / Dialogs / Transactional Dialogs / Has responses)
  item list | pagination

<div#settingsModal>
  CM.com Context URL input
  Open CM.com links: radio (popup / browser)

<div#exportModal>
  read-only textarea | Copy to clipboard
```

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
| `cm-open-mode`         | `"popup"` or `"browser"`               |
| `cm-dismissed-version` | Last update version the user dismissed |

---

## GitHub repository

GitHub account: **WithoutWout** (not `wouttonio`)
Repository: `WithoutWout/cm-conversation-dashboard`
Release URL pattern: `https://github.com/WithoutWout/cm-conversation-dashboard/releases/latest`

- Always use `WithoutWout` as the GitHub username, never `wouttonio`.
- The `check-for-updates` IPC handler fetches `api.github.com/repos/WithoutWout/cm-conversation-dashboard/releases/latest`.

---

## Coding conventions

- No external libraries (no lodash, no jQuery, no UI framework).
- All HTML built via string concatenation — always use `esc()` for any dynamic value.
- CSS variables for theming: `--bg`, `--surface`, `--surface2`, `--border`, `--text`, `--muted`, `--accent`, `--green`, `--blue`, `--orange`, `--red`, `--teal`.
- Internal identifiers (`_kind`, `tDialogMap`, `b-tdialog`, CSS class `type-tdialog`) use the short `tdialog`/`tDialog` form — only the user-facing label says "Transactional Dialog".
- Use `querySelector` / `getElementById` for DOM access; event delegation where multiple dynamic elements share a handler.
- `buildSearchRegex` is the single source of truth for search logic — do not duplicate regex construction elsewhere.
- Inline `onclick="..."` attributes are used intentionally for dynamically rendered cards (no event listener cleanup needed in this app).
