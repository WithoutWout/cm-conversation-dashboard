# Analysis (formerly GAP)

The questions the bot recognised badly, as a work list: pick a date range, go
down the rows, open the conversation each came from, find the entity that should
have caught it, fix it in CM.com, tick it off, and export what is left.

_Read this before changing the GAP view (`#view-gap`), `get_gap_interactions`,
`get_gap_feedback`, `set_gap_fixed`, `save_export_xlsx` or `xlsx.rs`._

The view is called **Analysis** on screen (it was "GAP"; the code, the
`#view-gap` id, the `cm-view` value `gap` and this file keep the old name, so
nothing saved is lost). It has three work lists behind a **Recognition |
Feedback | GenAI** switch at the start of the toolbar, sharing the range picker
— `#view-gap[data-mode]` shows the panes and controls of the one that is on.
Everything below up to "Feedback" is the Recognition list. Exports are
`Analysis {recognition|feedback|GenAI} {today} {Month}.xlsx`.

---

## What a row is

One **interaction** — a question and the answer it got — not a conversation and
not a distinct question. The same question asked forty times is forty rows,
because each is a separate piece of evidence and each opens its own chat. The
chat header says how often it was asked in the range, and offers to mark every
open row with that exact question at once.

A row is in the list when it is either:

- **Low** — recognised, with a score above zero and **strictly under** the
  low-recognition threshold (Settings › Conversations). This is exactly the row
  predicate of the conversation list's **Low %** pill, so the two cannot
  disagree about what "low" means.
- **Zero** — scored 0 by a recognizer that ran (`recognition_type` set): the
  **Zero %** pill's predicate. The most important gaps are the ones nothing
  recognised at all, so they are in the list rather than left for another view;
  the *Low* / *Zero* pills separate them.

GenAI turns are out on both counts (their score explains something other than
the answer), and so is a turn with no question text — there is nothing to
improve about it. Newest first, at most 20,000 rows; a range with more says so
in the count and asks to be narrowed, rather than silently showing a sample.

The range is picked on the **shared day calendar** — the same two-month
`calMonthHtml` grid, `_calRangeCls` range classes and class-only hover preview
as Import and Stored data (`docs/import.md` → "The shared day calendar"), in a
popover under the range button, with the presets underneath as quick buttons.
Unlike those two calendars its days are in the **display timezone** (the legend
says which): they name moments, as the conversation date filter's do, not rows.
`insZoneDayBounds` turns them into UTC bounds — the same conversion that filter
uses. **Days that hold data are marked** as on Import and Stored data — green
when every hour is imported, orange when some are, a tooltip with the count —
from the same `get_db_hour_coverage`, loaded when the calendar opens and dropped
after an import or a delete (`gapCoverageInvalidate`). That coverage is per UTC
day, and a display-timezone day spans two of them, so `zoneDayCoverage` checks a
local day hour by hour against the UTC hours it covers (23 or 25 across a DST
change; `gap-fb.test.js`). The consequence is honest rather than odd: in
Amsterdam the first imported UTC day reads as *partly* imported locally, because
its first two local hours fall on the UTC day before. `gap_rows` validates both bounds as `YYYY-MM-DDTHH:MM:SS` and the
threshold as 1–99 before anything runs.

## Leaving and coming back

Switching to another view and back changes nothing: the list, the open
conversation, the entity finder and all three scroll positions are as they were.

- **The list is read again only when what it is *for* changed** —
  `gapLoadedKey` is the database, the range and the threshold it was read with.
  Re-reading on every visit was the old behaviour, and it threw away the scroll
  position, the selection and the finder's place for an identical answer. A
  failed read clears the key, so the next visit retries.
- **The scroll positions are saved on leaving** (`gapRememberScroll`, from
  `switchView`) because the view is `display: none` while away and the browser
  resets a hidden element's scroll to 0 — measured, not assumed.
- **They are restored synchronously**, then the windowed rows repainted: the
  view is already shown when `onEnterGapView` runs, and a restore in a frame
  left rows painted for the top of the list, because a frame does not arrive in
  a window that is not composited.
- The entity finder is redrawn only when the content export behind it changed.
- **The sort survives a restart** (`cm-gap-sort`, in the settings backup too).
  It is read field by field: an unknown column or a direction other than ±1
  falls back to newest first. The pills, the text filter and the range are not
  remembered across a restart — the range starts at the last seven days.

## Fixed marks

`gap_fixed(log_id PRIMARY KEY, session_uuid, fixed_at, note)` in the
**conversations database**. The LogId is stable across re-imports, so a mark
survives re-importing a day. It goes when its interaction goes — the retention
purge and *Delete* in Stored data both delete the marks of the rows they remove,
so a mark can never point at nothing. A mark on a LogId that does not exist is
not written (`INSERT … SELECT … FROM interactions`).

Marking is optimistic: the list changes on click and puts itself back, with a
toast, if the write fails. `set_gap_fixed` returns the timestamp it stored so
the row shows the database's time, not the renderer's.

## The list

- **Windowed.** Rows are absolutely positioned at `index × 34px` inside a spacer
  the height of the whole list; only the rows in view (plus ten either side)
  exist. A month of low-recognition turns scrolls like ten.
- **One list for everything.** `gapView()` applies the pills, the text filter and
  the sort, and is what the rows, the count and the export all read. What is
  exported is exactly what is on screen, in the order it is on screen.
- The response column is one plain line: CM.com markup (`**`, `_` as a line
  break, `%{…}` tokens) is stripped by `gapPlain`, the full text is the tooltip,
  and the formatted answer is in the chat on the right.
- Keyboard: ↑ ↓ move and open, **F** or space toggles fixed, **C** copies the
  question.

## The side panel

- **The header starts with the language badge** the conversation list uses
  (`langBadgeHtml`, shared by both): the conversation's `translation_language`
  context when it set one — the language the bot actually answered in — and the
  culture otherwise, with both in the tooltip. `gap_rows` reads the translation
  language per row through `idx_ctx_name_session`.

- **The conversation** is `renderFlaggedThread` with `readOnly` and the row's
  turn marked (`gap-mark`) and centred. It is cached per session, so moving
  through several rows of one conversation does not refetch it. The bubble
  details work as in the chat — `toggleBubbleDetail` looks in the visible view
  first, because the same conversation can be open in two views at once.
- **The question's words** are chips. A click copies the word *and* looks it up
  in the entity finder below — the two things someone improving recognition does
  next. Shift-click collects a phrase.
- **"Fix this"** replaces the plain entity finder with what to change in
  CM.com, each line ending in *Edit ↗* (through the usual link handler, so the
  pop-up/browser setting holds) and a copy-link button. It is read from the
  turn's own `recognitionDetails` and the turns after it — the chat rows the
  panel already loaded, so the backend sends nothing extra. `gapFixModel` is the
  data, `gapFixHtml` the drawing; `gap-fix.test.js` pins the model.
  - **Unknown words** — `missingWords`: words no entity knows. *Add to entity…*
    copies the word and turns the search box into a picker: the closest
    entities first (`gapClosestEntities` — a shared stem, or one word inside
    the other, which is the usual reason: "schatkaarten" beside "schatkaart"),
    then anything typed. Each result's button copies the word again and opens
    that entity, ready to paste.
  - **Recognised as** — `entityMatches`, by the entity's own `entityId`, so the
    link is direct even for an entity the imported conversations never
    otherwise fired.
  - **Every other entity link needs the id map.** The EntitiesExport CSV has
    no id column; the only ids are in `entity_index`, loaded with the entity
    options (`ensureEntityIdMap`). `onEnterGapView` asks for it — the view used
    not to, so every *Add "word"* and finder link went to the Entities list
    page unless the Conversations entity picker had happened to be opened
    first. The links render at once and `_refreshEntityLinks` upgrades them in
    place when the ids land. An import resets `entityOptionsLoaded`, so an
    entity firing for the first time gets its link without a restart. One that
    has never fired in any imported conversation has no id anywhere and keeps
    the list-page link, with a tooltip saying why.
  - **Answered by** — the turn's `article_ids` as Article / Dialog-node links;
    for a zero it says the fallback answered, rather than linking the fallback.
  - **No Article for this** — when CM.com set `missingArticle` (or the turn is a
    zero): the Articles whose questions use the recognised entities, most shared
    first, the one that answered left out. Needs the content export.
  - **They asked next** — the next turn within eight that the recognizer scored
    at or above the threshold (GenAI skipped): usually a rephrase, and the best
    hint for which Article the person was after.
  - A turn with no recognition details says so rather than guessing.
- **The class is `.gap-fixpanel`, not `.gap-fix`** — that name is the round ✓
  on each list row, and reusing it squeezed the panel to 20 px.
- Typing in the panel's search box still searches entities by name and word,
  as the finder did; clearing it brings the panel back.

## Feedback

The other half of what the bot gets wrong: not what it failed to recognise, but
what it answered and was told was wrong. The Feedback mode lists every Article
and Dialog node that answered a rated question in the range, with how often it
was rated, how often down, and the positive share — lowest first.

- **`get_gap_feedback` returns rated *answers*, not ratings.** Each rating is
  resolved to the answer it names (`originatingInteractionId`, or the row
  itself for an older in-place score) and an answer comes back once, scored by
  its **latest** rating — CM.com's rule ("one feedback item per interaction –
  the latest one"), and the Insights feedback card's, so none of them can
  disagree about the same answer. The range is the rating's time.
- **The alias is `origin_uuid`, never `oid`.** `oid` is SQLite's own name for
  the rowid; over one table `GROUP BY oid` grouped by the *rating's* rowid and an
  answer rated twice came back twice. The test that caught it is
  `the_gap_feedback_list_is_the_rated_answers_once_each`.
- **Grouping is the renderer's** (`gapFbItems`, pinned by `gap-fb.test.js`), so
  switching *Per answer* ↔ *Per Article · Dialog* is instant. An answer counts
  under **every** id in its `article_ids` — a Dialog node that answered with an
  Article's text is about both. Per Dialog, the nodes add up into `dn-<id>`. An
  answer with no id (the fallback, GenAI) is one line of its own, not dropped.
- **"At least N ratings"** (default 3) keeps a single thumbs-down from topping
  the list at 0%. The count line says how many items it left out.
- Selecting an item lists its rated questions, thumbs down first, and opens the
  first; each opens its conversation with the rated answer marked, through the
  same cache and `renderFlaggedThread` the Recognition list uses. *Thumbs down
  in Conversations* searches the item's id chip with the 👎 pill on — which,
  since the rated-answer rule in `docs/search.md`, finds exactly these.
- **The two modes keep separate panes and separate state**, so switching loses
  neither list's place. Scroll positions are saved per mode as its panes are
  hidden (`gapSavedScroll[mode]`), because a hidden pane reads a scroll of 0 —
  one shared snapshot lost the hidden mode's position on every switch.
- A new range re-reads the mode on screen only (`gapReload`); the other notices
  on its next visit because its loaded key no longer matches.
- Export writes the listed items as they are sorted:
  `GAP feedback {today} {Month}.xlsx`.

## GenAI

Everything GenAI answered in the range, newest first: the question, the answer,
and the conversation around it. It is not feedback on GenAI — just what was
asked and what it said.

- **`get_gap_genai` is the GenAI pill's predicate, row for row**
  (`main_interaction_type` or `all_interaction_types` naming `GenerativeAI`),
  so this list and the conversation list cannot disagree about which turns are
  GenAI. A turn with no question text is left out.
- **No split of this app's own invention.** About half the GenAI turns are
  logged with main type `QA` and `GenerativeAI` among `all_interaction_types`
  (391 of 715 on the reference database). There was a *GenAI only / With
  another answer* pill and a `+QA` badge for them; neither is a CM.com term and
  they read as something the portal never shows, so they are gone. The header
  and the export state the type exactly as the log records it
  (`gapAiType`: `GenerativeAI`, or `QA + GenerativeAI`).
- Selecting one shows the answer formatted (`parseCmOutput(…).body`), the
  source Articles from `faqs_found` with Edit links, the Halo Studio link for
  the conversation (`haloConversationLink`, when a Halo Studio URL is set in
  Settings), and the conversation with the turn marked; the list is windowed
  like the Recognition list.
- The backend also returns each answer's worst rating (`score`), unused on
  screen for now.

## The export

`Analysis recognition {today} {Month}.xlsx` — `GAP 2026-09-23 September.xlsx`, or
`September-October` when the range spans two. Columns: date/time (display
timezone), question, response (plain), recognition % (a number, so it sorts),
kind, Articles/Dialogs, culture, fixed, fixed at, conversation id. Fixed rows are
filled green.

The workbook is written by `xlsx.rs`, not a crate: the format needed is six small
XML parts over the `zip` dependency the updater already brings in. What had to
be right is in its tests — every cell escaped, XML-forbidden control characters
dropped (Excel refuses a whole file over one), cells cut at Excel's 32,767
characters, sheet names cleaned to Excel's rules, and a number kept a number.
`save_export_xlsx` takes rows rather than a query, so the next table that wants
an Excel export can use it as is.

## Halo Studio and Open in Conversations

- **Every mode links the conversation shown to Halo Studio** (`haloConversationLink`, hidden while no Halo Studio URL is set). Recognition and GenAI put it in the row's header. Feedback's header is about an Article or Dialog that spans many conversations, so its link (`#gapFbHalo`, `gapFbHaloFill`) follows the rated answer whose conversation is open.
- **Open in Conversations lands on a full header.** `selectSession` used to draw the chat header only for a conversation in the current search results (`convSessions`) — one opened from Analysis usually is not, and arrived with no Halo Studio link, date, counts or contexts. It now falls back to `sessionFromRows`, the same summary built from the conversation's own rows (answers counted as `session_summary` counts them, contexts from the latest row that has any), kept as `convOpenedSession` so the contexts button finds it too.

## Fixed marks in Feedback

Feedback takes the same fixed marks as Recognition — the `gap_fixed` row keyed by the **answer's** log id, which `get_gap_feedback` returns as `fixedAt`. One mark per answer, so an answer on both lists is fixed on both (`gapToggleFixed` and `gapFbToggleFixed` update each other's rows).

- **An Article or Dialog is fixed once every thumbs down on it is** (`gapFbItems` → `fixed`, `negFixed`, `fixedAt` = the latest fix). A thumbs down that arrives after the fix is not marked, so the item reopens by itself — which is the point: the fix did not hold.
- **Nothing rated down is nothing to fix**: an item with only thumbs up has no ✓ and is never "fixed".
- The list's ✓ column (and **F**) marks every thumbs down on the item, or reopens them all when it is fixed; a partly fixed item shows an outlined ✓. In the side panel, the header has **Mark fixed** / **Mark the rest fixed** / **✓ Fixed**, and each thumbs-down answer its own ✓.
- **Any / Open / Fixed** filters Feedback as it filters Recognition. Under Open an item that has just been fixed leaves the list but stays in the side panel (`gapFbItemByKey` falls back to the unfiltered items).
- **The export** adds *Fixed* (`Yes`, or `1 of 3` for a partly fixed item) and *Fixed at*, and fills fixed items green — as the Recognition export does its fixed rows. GenAI has no fixed marks: it is a reading list, not a work list.

## Selecting, copying and flagging turns

Every Analysis conversation — Recognition, Feedback and GenAI — takes the same turn selection as the Conversations view, through the selection bubble: **Select turns** at the bottom of the thread (or a triple-click on a turn), then Copy and Flag. Flag sends the conversation to the Flagged view with those turns marked. See `docs/chat-rendering.md` → "Selecting turns: the selection bubble".

## Export names

An export is named for its analysis and its date range — `Recognition analysis 14–20 Sep 2026.xlsx` — and the sheet likewise when Excel's 31 characters allow (`Recognition 14–20 Sep 2026`), otherwise just the analysis. `gapRangeLabel` writes only as much of the month and year as the two ends do not share: `19 Sep 2026`, `28 Aug – 3 Sep 2026`, `28 Dec 2025 – 3 Jan 2026`. It used to be today's date plus the month names, which said when the file was made rather than what it covers.

