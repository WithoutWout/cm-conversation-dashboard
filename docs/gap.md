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

- **The conversation** is `renderFlaggedThread` with `readOnly` and the row's
  turn marked (`gap-mark`) and centred. It is cached per session, so moving
  through several rows of one conversation does not refetch it. The bubble
  details work as in the chat — `toggleBubbleDetail` looks in the visible view
  first, because the same conversation can be open in two views at once.
- **The question's words** are chips. A click copies the word *and* looks it up
  in the entity finder below — the two things someone improving recognition does
  next. Shift-click collects a phrase.
- **The entity finder** searches the content export's entities by name and by
  trigger word; an exact word match ranks first. Each entity opens in CM.com
  through `entityOpenButton` (by id when imported conversations have fired it,
  the list page otherwise — the export CSV has no ids), and each word copies. No
  match says so: a word no entity knows is a candidate to add.

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
