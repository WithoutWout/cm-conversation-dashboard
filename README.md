# Conversational AI Cloud Dashboard

A desktop application for inspecting, navigating, and searching [CM.com Conversational AI Cloud](https://www.cm.com/en-gb/conversational-ai/) content exports and conversation logs — without needing to be logged into the platform.

Built with **Tauri v2** (Rust backend + vanilla JS frontend). Works fully offline.

---

## Contents

- [Features](#features)
- [Installation](#installation)
- [Getting Started](#getting-started)
- [Content View](#content-view)
  - [Search](#search)
  - [Tabs & Filters](#tabs--filters)
  - [Article Cards](#article-cards)
  - [Dialog Cards](#dialog-cards)
  - [Entity Cards](#entity-cards)
  - [Detail Modals](#detail-modals)
  - [Flow Graph](#flow-graph)
  - [Context Filter](#context-filter)
  - [Export IDs](#export-ids)
- [Conversations View](#conversations-view)
  - [Importing Interaction Logs](#importing-interaction-logs)
  - [Session List](#session-list)
  - [Chat Thread](#chat-thread)
  - [Chat Search](#chat-search)
  - [Managing the Database](#managing-the-database)
- [Settings](#settings)
- [Keyboard Shortcuts](#keyboard-shortcuts)
- [Updates](#updates)

---

## Features

| Area                | Highlights                                                                                          |
| ------------------- | --------------------------------------------------------------------------------------------------- |
| **Content**         | Browse and search all Articles, Dialogs, Transactional Dialogs, and Entities from your export files |
| **Search**          | AND / OR / exact phrase syntax; case, word-boundary, and regex modes; responses-only toggle         |
| **Conversations**   | Import CSV interaction logs into a local SQLite database; browse and search sessions                |
| **Chat**            | Read individual conversation threads with rich bot output rendering (cards, CTAs, dialogs)          |
| **Flow Graph**      | Interactive visual map of dialog node connections (powered by vis-network)                          |
| **Context Filters** | Filter by contextual answer context variables in both Content and Conversations views               |
| **Export IDs**      | Copy article/dialog IDs in Jira-ready rich hyperlink format                                         |
| **Deep Links**      | Open any article, dialog, or dialog node directly in CM.com (when a context URL is configured)      |
| **Auto-updates**    | Checks GitHub releases on startup and notifies when a new version is available                      |

---

## Installation

Download the latest release for your platform from the [Releases page](https://github.com/WithoutWout/cm-conversation-dashboard/releases/latest).

| Platform              | File                    |
| --------------------- | ----------------------- |
| macOS (Apple Silicon) | `.dmg`                  |
| Windows               | `.exe` installer (NSIS) |

> **macOS note:** The app is not yet notarised. On first launch, right-click the app → **Open** to bypass the Gatekeeper warning.

---

## Getting Started

### 1 — Export your data from CM.com

You need up to three export files. Place them all in the same folder on your machine.

**Articles** (required):

1. In CM.com → _Articles_
2. Click ⋯ → **Export**
3. Choose **Export JSON**
4. Save the file — it will be named something like `*ArticlesExport*.json`

**Dialogs** (required):

1. In CM.com → _Dialogs_
2. Click ⋯ → **Export**
3. Choose **Export JSON**
4. Save the file — named `*DialogsExport*.json`

**Entities** (optional, enriches search):

1. In CM.com → _Entities_
2. Click **Export**
3. Save the `.csv` file

> You can also open the built-in instructions by clicking the **?** link in Settings.

### 2 — Select the data folder

On first launch the app shows an empty state. Click **Select Data Folder…** (or open **⚙ Settings** → Content tab) and pick the folder containing your export files.

The app will immediately load and display your content.

---

## Content View

The Content view is the default view. Switch between Content and Conversations using the two icons in the top-right of the header.

### Search

The global search bar sits below the header and applies to whichever tab is currently active.

#### Search syntax

| Syntax           | Meaning                             | Example                                |
| ---------------- | ----------------------------------- | -------------------------------------- |
| `word1 word2`    | AND — both must appear              | `payment error`                        |
| `word1 \| word2` | OR — either must appear             | `payment \| betaling`                  |
| `"exact phrase"` | Phrase — words must appear adjacent | `"opening hours"`                      |
| combinations     | Compose freely                      | `"opening hours" ticket \| reserveren` |

> Hover over the search bar to see a quick syntax reminder.

#### Search modifiers (buttons to the right of the input)

| Button | Default | Effect                                                    |
| ------ | ------- | --------------------------------------------------------- |
| `Aa`   | Off     | Case-sensitive matching                                   |
| `\b`   | Off     | Whole-word matching only                                  |
| `.*`   | Off     | Full regex mode (input border turns red on invalid regex) |
| `¬T`   | **On**  | Responses only — ignores IDs, titles, and entity names    |

> **Entity enrichment:** when your query matches a word inside an entity, articles and dialogs that use that entity also appear in results — even if the query text isn't literally in the response.

#### Context filter

The **funnel** button (right of the search options) opens the **Content Context Filter** panel. Filter by one or more contextual answer context-variable values. The badge on the button shows how many filters are active. Matched contextual answers glow green inside cards.

---

### Tabs & Filters

| Tab             | Shows                                               | Filter pills                                          |
| --------------- | --------------------------------------------------- | ----------------------------------------------------- |
| **All Results** | Articles + Dialogs + Transactional Dialogs combined | All / Articles / Dialogs / Transactional Dialogs      |
| **Articles**    | Articles only                                       | All / Has response / Dialog link                      |
| **Dialogs**     | Dialogs + Transactional Dialogs                     | All / Dialogs / Transactional Dialogs / Has responses |
| **Entities**    | Entities only                                       | All / Used in articles / Used in dialogs              |

Each tab shows a result count and sub-stats (e.g. `12 art · 5 dlg · 2 t.dlg`).

Results are paginated at **50 items per page**. Sort by ID ascending/descending or by name A→Z / Z→A — the sort is saved per-tab.

---

### Article Cards

Each article card shows:

- **ID badge** (blue) + article title (primary FAQ question, or first entity if no FAQ)
- **Badges**: `Article`, `Response` / `Dialog Link` / `Transactional Dialog`, `FAQ`, entity count
- Hover the card to preview the first 250 characters of the response

**Click a card to expand it:**

- **Entities** — all question/entity chips; FAQ chips highlighted in green; click a chip to open the Entity Info Modal
- **Response** — full response text with variable tokens styled as `[VarName]` and search terms highlighted
- **Linked dialog** — if the article routes to a dialog, a jump link appears
- **Contextual Responses** — collapsible section; each contextual variant shows its context-variable conditions as pills and its response text; items matching the active context filter glow green; items with no context conditions are flagged `Unreachable` in red

**Action buttons** (top-right of expanded card):

- **↗** — open in CM.com (requires a context URL in Settings)
- **💬 Conversations** — switch to Conversations view with this article's ID pre-searched

---

### Dialog Cards

**Dialog cards** (purple):

- ID badge + name + `Dialog` badge + `N Recognition` badge + node count
- Hover to preview the description

**Transactional Dialog cards** (teal):

- ID badge + name + `Transactional Dialog` badge

**Expand a dialog card** to see all nodes with their type (Recognition / Output), response text, entity options and routing.

**Action buttons:**

- **↗** — open in CM.com
- **▶ Show dialog** (Dialogs only) — opens the visual Flow Graph
- **💬 Conversations** — pre-search conversations by this dialog's ID

---

### Entity Cards

Orange left border. Shows the entity name, type, word count, and how many articles and dialogs reference it.

**Click to expand:** see all entity words as chips and the articles + dialogs that use this entity as clickable links.

---

### Detail Modals

Clicking the info button on a card opens a **full-screen detail modal**. All modals support **← Back** navigation and stack:

| Modal            | Contents                                                                          |
| ---------------- | --------------------------------------------------------------------------------- |
| **Article Info** | All entities, full response, contextual responses                                 |
| **Dialog Info**  | Description, all nodes with outputs, options, and routing; CM.com node deep-links |
| **Entity Info**  | All words, all referencing articles and dialogs                                   |

> Press **ESC** to close the top-most modal.

---

### Flow Graph

Click **▶ Show dialog** to open an interactive visual map of the dialog's node connections.

- **Nodes**: Recognition (purple), Output (blue), GoTo (dashed gray)
- **Edges**: labelled with entity question text; fallback edges in orange; GoTo edges dashed
- Scroll to zoom, drag to pan
- **Click a node** to highlight it and show a detail panel at the bottom:
  - Type, name, response text, entity options, child connections, linked dialogs

**Direction toggle:** switch between left-to-right (→) and top-to-bottom (↓) layout (persisted per session).

---

### Context Filter

The Content Context Filter panel lets you narrow results to articles and dialogs that have answers for a specific context (e.g. a specific language, user tier, or session variable value).

- Click the **🏷** funnel button next to the search options
- Select one or more context variable values (grouped by variable name)
- Matching contextual answers glow inside expanded cards
- Multiple selections: all filters must match (AND logic)

---

### Export IDs

Click **Export IDs** in the header to copy article and dialog IDs from the current tab and active search.

- Each row shows the ID token (`qa-{id}` for articles, `dn-{id}` for dialogs) and the item title
- **Copy row** copies a single Jira-ready `<a>` hyperlink for that item
- **Copy to clipboard** copies all items as a rich HTML list (plain-text fallback for non-rich editors)

---

## Conversations View

Switch to the Conversations view using the chat icon in the header. This view requires a local SQLite database to be set up (see Settings).

---

### Importing Interaction Logs

Interaction logs are exported one day at a time from CM.com Analytics:

1. Analytics → Interactions Export
2. Select a date
3. Choose **CSV** format → Export
4. In the app: click **Import CSV…** → select the file(s)

You can also **drag and drop** one or more CSV files directly onto the Conversations view.

After import a success toast shows how many interactions were added, how many duplicates were skipped, and how many old entries were purged.

> Only one day at a time can be exported from CM.com.

---

### Session List

The left sidebar shows a paginated list of conversation sessions (50 per page).

#### Searching sessions

The search bar at the top of the sidebar searches across all sessions. Press **Enter** or click the magnifier button to run a search.

**Search syntax** — same as content search:

| Syntax                | Example               |
| --------------------- | --------------------- |
| `word1 word2` (AND)   | `payment failed`      |
| `word1 \| word2` (OR) | `payment \| betaling` |
| `"exact phrase"`      | `"where is my order"` |
| Regex (`.*` toggle)   | `\\bpayment\\b`       |

**Search modifiers:**

| Toggle    | Effect                                                              |
| --------- | ------------------------------------------------------------------- |
| `#ID`     | Search by Article or Dialog ID (numeric only); bypasses text search |
| `.*`      | Regex mode                                                          |
| `U` / `B` | Restrict to User messages / Bot messages / both                     |

#### Date range picker

Click the date button to open a calendar. Click once to set the start date, again to set the end date. Days outside the stored date range are grayed out. On first connect, the most recent day is selected automatically.

#### Session filter pills

| Pill          | Shows                                                                   |
| ------------- | ----------------------------------------------------------------------- |
| `GenAI`       | Sessions handled by the GenAI engine                                    |
| `👎 feedback` | Sessions with negative feedback (double-click → 👍 positive)            |
| `Low %`       | Sessions with a low recognition score (below the threshold in Settings) |
| `Zero %`      | Sessions with zero recognition score                                    |

#### Session cards

Each card shows:

- Truncated session UUID
- Badges: `AI` (GenAI), 👎 / 👍 (feedback), culture code
- First user-message preview (search terms highlighted)
- Timestamp + interaction count
- **ℹ︎** button — opens a modal with all session context variables

---

### Chat Thread

Click a session card to open its conversation in the right panel.

Interactions are grouped into logical Q&A turns and rendered as a chat timeline:

- **Bot messages** (purple avatar): parsed response text with support for:
  - **Ask cards** (image + button + prompt text)
  - **CTA buttons** (open URL in an in-app iframe preview or the default browser)
  - **Dialog option chips**
  - Markdown-style links and HTML links
  - Variable and template tokens
- **GenAI messages** (teal avatar + `AI` badge): styled with a teal tint
- **User messages** (blue tint)
- **System events**: shown as centered pills
- **Link-click events**: `🔗 User clicked [URL]`
- **Dialog-started events**: `↳ Dialog started [name]`

**Turn detail panel** — click any bot message bubble to expand it:

- Referenced **Articles** and **Dialogs** — click to jump to their detail modal
- **Entity matches** — entity name + matched word chips
- **Recognition quality** — percentage with a color-coded bar (green / orange / red)
- **Recognition type**

> Bubbles with zero or low recognition are highlighted in red/orange and scrolled into view automatically when those filters are active.

---

### Chat Search

The search bar inside the chat panel filters turns within the currently open conversation.

- **200 ms debounce**
- Supports the same AND / OR / `"exact phrase"` / regex syntax
- Non-matching turns are hidden; a banner shows `N matched turns of M · Show full conversation`
- Click the banner to show all turns with matches still highlighted

The `.*` regex toggle turns red when the pattern is invalid.

---

### Managing the Database

Click **Manage DB** to open the database management modal:

- Overview: total interactions + number of days stored
- Select days to delete (checkbox per day + session count)
- **Delete Selected Days** — requires confirmation; shows exact interaction count before deleting
- **This action cannot be undone.**

---

## Settings

Open **⚙ Settings** from the header.

### Content tab

| Setting                | Description                                                                                                                                            |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **CM.com Context URL** | Base URL for your AI Cloud environment (e.g. `https://www.cm.com/en-gb/app/aicloud/{tenantId}/{projectName}/nl/`). Required to show deep-link buttons. |
| **Open CM.com links**  | Open links in a popup window within the app, or in the default system browser                                                                          |
| **Data Folder**        | Select or refresh the folder containing your export files                                                                                              |
| **App Updates**        | Manually check for a new GitHub release                                                                                                                |

### Conversations tab

| Setting                           | Description                                                                    |
| --------------------------------- | ------------------------------------------------------------------------------ |
| **Halo Studio URL**               | Enables "Open in Halo Studio" buttons on sessions that have GenAI interactions |
| **Low recognition threshold (%)** | Sessions below this value are flagged as `Low %` (default: 60)                 |
| **Conversations Database**        | Create a new SQLite database or open an existing one                           |

---

## Keyboard Shortcuts

| Key     | Action                                                                              |
| ------- | ----------------------------------------------------------------------------------- |
| `ESC`   | Close the top-most open modal, or clear the global search input if no modal is open |
| `Enter` | Run conversation search (in the session search bar)                                 |

---

## Updates

On startup the app silently checks [GitHub releases](https://github.com/WithoutWout/cm-conversation-dashboard/releases/latest) for a newer version.

When an update is available:

- A green banner appears at the top of the window
- An **Update Available** modal shows the version change
- Click **↓ Download update** to open the releases page
- Click **Later** to dismiss (the same version won't be shown again)

You can also manually trigger a check via **⚙ Settings** → Content tab → **Check for updates**.

---

## License

This project is not open source and is intended for internal use.
