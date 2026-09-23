# GAP analysis

The questions the bot recognised badly, as a work list: pick a date range, go
down the rows, open the conversation each came from, find the entity that should
have caught it, fix it in CM.com, tick it off, and export what is left.

_Read this before changing the GAP view (`#view-gap`), `get_gap_interactions`,
`set_gap_fixed`, `save_export_xlsx` or `xlsx.rs`._

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
uses. `gap_rows` validates both bounds as `YYYY-MM-DDTHH:MM:SS` and the
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

## The export

`GAP {today} {Month}.xlsx` — `GAP 2026-09-23 September.xlsx`, or
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
