# Insights

Reading a conversation search as distributions: the chooser, the two units, the charts and the copy formats.

_Split out of `CLAUDE.md`. Read this before changing anything it covers._

---

The Conversations view answers "which conversations match?", fifty rows at a
time. **Insights** answers "what is in all of them?" — the same result set read
as distributions. Every one of its reads takes the *same* `GetSessionsArgs` as
`get_sessions`, and the renderer passes `lastConvSearchArgs`, so the headline
count and the count above the session list are the same number by construction;
there is no second notion of "the current results" that can drift.

It **opens on a chooser and reads nothing** until asked — see `The chooser`.
What it then reads comes back in two reads, reuses a result set it has already
resolved, and can be stopped part-way — see `Why it is two reads, not one`,
`The result set is resolved once, not once per read`, and `Cancelling`.

It is called **Insights**, not Analytics — "Analytics API" already names the
import source, and reusing the word would make two unrelated things share a name.

## The two readings

One result set, two units, chosen by the header's segmented control:
**Conversations** (the default) and **Interactions**.

A conversation is several turns and a search usually matches one of them, so
counting conversations was really answering *"at what hour did conversations
containing a match start?"*. Counting the matching turns themselves is a
different — often sharper — question, and the two must never be mistaken for
each other. That is the whole design constraint here.

- **The unit is a reading of one result set, never a second filter.**
  `session_count` is identical in both, which
  `the_interactions_unit_counts_matching_turns_not_the_conversations_holding_them`
  asserts. The sidebar's count stays the headline count in conversations mode
  and stays visible as its own tile in interactions mode.
- **The matching turns come from the search's own relation, before it is
  collapsed.** `build_session_filter_query` already produced
  `(session_uuid, match_log_id)` per matching interaction and then threw all but
  one away with `MIN(match_log_id) GROUP BY session_uuid`. `SessionFilterQuery.
  match_rows` carries the pre-collapse relation; nothing on the hot search path
  changed, and the same positional parameters are simply referenced twice.
  A feedback or recognition pill with no query narrows turns too — their CTEs
  are already exactly "the rows that satisfied it".
- **With no query and no pill, every interaction is a match**, and
  `matches_are_narrowed` is false so the labelling stops saying "matching". The
  mode is still worth having there — it counts interactions instead of
  conversations — but claiming a turn was singled out would be a lie.
- **`insight_matches` is `log_id`-keyed with `INSERT OR IGNORE`.** Several OR
  groups of one search can match the same row; the primary key dedupes it
  instead of counting it twice. It is only built in interactions mode, and
  dropped with `insight_sessions` at the end of the call.
- **`IS_GENAI_ROW` / `IS_ZERO_RECOG_ROW` / `IS_SCORED_ROW` are written once**
  and mirror `session_summary_insert_sql`. Two spellings of "is this turn a
  zero-recognition turn?" is exactly how a tile and a chart on one screen come
  to disagree about the same rows.
- **`insWords(d)` is the single source of the nouns.** Every card note, tooltip,
  table header, tile label, hero label and both exports read `noun`/`subject`
  from it, so a chart drawn in one reading and labelled in the other is not
  expressible. `every card, tile and tooltip in one reading names that reading`
  pins it, and the copied TSV's column header names the unit too — a bare number
  in a spreadsheet cell has no other way to say which reading it came from.
- **Switching refetches; it does not re-derive.** Nearly every aggregate is a
  different query in the other reading. `insLoadSeq` discards a stale answer if
  the user toggles twice quickly, and the toggle goes inert while a read runs.
- **The selected Context/Metadata key survives the switch**, so the two readings
  of one key can be compared in two clicks.

**Two cards are conversation-only, and the body says so rather than hiding it.**
*Feedback* — a thumb rates the conversation, not the turn that happened to match,
and showing it per-turn invites exactly the misreading the mode exists to prevent
("43% of my matching turns got a thumbs down" when it is 43% of their
conversations). *Opening questions* — a property of the conversation with no
per-turn reading at all. The "Thumbs down" tile goes with them.

**One card keeps its own unit in both readings.** *Conversation length* bins
conversations either way; what changes is what is being binned — every turn, or
only the matching ones — so it is retitled *"Matching interactions per
conversation"* rather than dropped, and its note and tooltips say "conversations"
so its axis can never be read as the toggle's unit.

**Context and metadata are recorded per conversation, not per turn**
(`context_index`/`metadata_index` are keyed by `session_uuid` with no `log_id`),
so in interactions mode a turn is counted under the value *its conversation*
carried. There is no per-turn context to count instead; the card's note says so
in those words, which is the difference between a caveat and a wrong number.

## One timezone disclaimer, not ten

The database stores naive UTC by design and the conversation date filter reads
it that way, so every day and hour on this screen is a UTC one. That used to be
said on both time axes, in three tooltips, in two card notes, in the header, in
the footer and in the copied table header — a lot of words for something that is
true of the whole screen at once and never varies.

What is left is **one quiet `.ins-tz` badge in the header**, carrying the
explanation in its `title`.

- **The hour axis keeps `Hour (UTC)`, and that is a label rather than a
  disclaimer.** A copied chart image is an SVG on its own in someone's email
  with no header above it, and an axis reading `09` with nothing to say what
  that means is ambiguous in a way a date is not. The copied table's column
  header follows it for the same reason.
- **Both exports keep one line each** — the report's own header and footer —
  since neither travels with the badge.
- `the timezone is a header badge, not a caption on every chart` asserts it over
  every real card in both readings, and asserts the hour axis is still labelled
  so it cannot pass by the disclaimer having been deleted outright.

## The Context and Metadata key picker

Around forty keys of each on a real export. As a chip per key that was four
wrapped rows of buttons standing between the section heading and its only chart
— so the chart the section exists for started below the fold.

One button naming the current key, with the rest behind `#insKeyPop`: a
searchable list where each row carries the key's coverage **and** its distinct
value count, because a key set on four conversations and a key with eight
hundred values are both dead ends and neither is visible from a name.

- **Bounded by `#insBody`, not by the window**, and that is the rule rather than
  a refinement. At a fixed height it flipped above a button in the lower half of
  the modal and came down over the header — on top of Copy dashboard and the
  close button, which it must never cover. The body's rect is also a real rect
  in the moments when `window.innerHeight` reads 0.
- **It opens downward unless there is an actual shortage of room**, and its
  height follows the space available down to `INS_KEYPOP_MIN_H`.
- **It closes on scroll rather than following the button.** Unlike the
  message-metadata bubble this is a menu, not an annotation of the thing it
  points at, so there is nothing to keep it beside.
- **`.ins-keypop[hidden]` needs its own rule** — `display: flex` beats the
  `hidden` attribute's UA `display: none`, so without it the popover floats over
  the charts from the moment the modal opens.
- Escape closes the picker before the modal, so one press dismisses one thing.

## The screen it is given

Insights is deliberately the largest surface in the app —
`min(2100px, 97vw)` × `96vh`, where every other modal is sized to its content.
Sixteen charts at a fixed 480px each: every 480px of width is another column and
every 100px of height is another chart row read without scrolling, so capping it
at a comfortable dialog size spent the window on backdrop.

**The section headings are sticky, and making that work is one rule about the
scroll container, not a style choice.** `.ins-body` carries **no top padding**,
because WebKit resolves a sticky `top: 0` against the scrollport's *content*
box: a `padding-top` parks the stuck heading that far down the pane and leaves a
band above it that the cards scroll through in full view — a strip of chart
above the heading with nothing to explain it. Measured at exactly the 16px the
padding was, with 4 of 9 probe points across the pane hitting a card. The space
now lives on `.ins-tiles` instead.

The heading is **full-bleed** to match: negative side margins carry its opaque
background out over the body's side padding, so nothing can pass beside it
either. With both, every probe point from the top of the scrollport down to the
heading's bottom hits the heading, at every scroll position.

## Why the charts are hand-built SVG

Because the deliverable is a picture on someone's clipboard, not a picture on
this screen. A charting library draws to a canvas or to DOM wired to a
stylesheet; either way, getting a clean 2× PNG out means re-rendering it
somewhere else and hoping it matches. An SVG string with every colour, size and
font written into the attributes **is** the export: rasterising it is
`new Image()` plus `drawImage`, and the same builder can be asked for the
*light* variant an email actually wants without touching the one on screen. It
also satisfies the vendored/offline rule trivially.

- **`insRenderChart(spec, theme)` is the only entry point**, so a card can be
  asked for its screen form and its export form by changing one argument.
  `insColumnChart` / `insBarChart` / `insStackChart` / `insHeatmap` /
  `insAreaChart` are pure string builders with no DOM access, which is what lets
  `frontend/tests/insights.test.js` run them under Node.
- **No mark carries a class, and no chart references anything external.** That
  is the property the whole copy feature rests on: a chart that needs a
  stylesheet renders as a blank box inside an `<img>`, and it does so only after
  it has been pasted somewhere. `an exported chart depends on nothing outside
  itself` asserts it — no `class=`, no `<style>`, no `url(`/`href=`, a colour on
  every element and a font on every `<text>`.
- **`INS_FONT` is deliberately not the app's `-apple-system` stack.** An exported
  SVG is rasterised inside an `<img>`, where the font resolves against whatever
  the renderer can see, and the PNG then lands on machines that are not this one.
  Helvetica and Arial resolve everywhere.
- **Text is measured before it is drawn** (`insTextWidth` / `insFitText`, an
  average-glyph-width estimate). A clipped label is worse than a shorter one —
  `Stoppen_Faciliteitenkaart` and `Stoppen_Faciliteitenpas` differ in exactly the
  part that gets cut. Long labels get an ellipsis, and the full text stays in the
  tooltip and the copied table. The bar chart's tip-label gutter is measured from
  the widest label it will actually draw: `3,140 · 50%` is half again as wide as
  `3,140`, and a fixed reservation put the longest one past the right edge, where
  nothing crops it — it is simply gone.
- **`assertInsideCanvas` runs over every real card**, marks and labels alike.

## The two palettes

Two themes, not one flipped. `INS_THEME_EXPORT` is the one that matters most: a
dark-surfaced chart pasted into Gmail is a black box in the middle of a white
message. Both were validated as categorical/sequential sets against the surface
they are actually drawn on (`#21253a` on screen, `#ffffff` in an email) — every
mark clears 3:1, and the four status steps are the reserved good→critical scale,
never used for series identity.

- **Almost every chart here is one series**, so it takes the single accent hue.
  Colouring nominal bars by their value would spend the identity channel
  re-encoding what bar length already shows.
- **Recognition bands wear status colours** (`Zero` → critical, `Under 40%` →
  serious, `40–69%` → warning, `70–100%` → good) because the band *means*
  good-to-bad, and each one carries its label.
- **The heatmap is sequential — one hue, light→dark — and `ramp[0]` is a bare
  tint off the surface, not the surface itself.** An hour with nothing in it has
  to read as empty *and* still be a cell, or the grid dissolves wherever the data
  is quiet and "no conversations" becomes indistinguishable from "no square
  here". It is deliberately un-blue so it can never be read as the bottom of the
  value scale, and the scale legend keys it along with every other step.

## Numbers that had to be got right

- **The feedback split is four disjoint groups**, which is why
  `mixed_feedback_sessions` exists. `has_pos_feedback` and `has_neg_feedback` are
  independent flags — one conversation can carry a thumbs up *and* a thumbs down
  — so "no feedback" is not `total − up − down`. Without the overlap the stack
  over-counts the rated conversations and under-states how many nobody rated.
- **A tag value's share is of the conversations that set the key** (`withKey`),
  not of the whole result. Against the whole result every value of a rarely-set
  key reads as negligible, which is a statement about the key rather than about
  the value. The note states the remainder that never set it.
- **There is deliberately no "Other" bar on a tag chart.** It looks like the
  obvious completion of the chart and it cannot be computed: `context_index` is
  keyed by `(name, value, session_uuid)`, so a conversation whose context changed
  mid-way contributes to two values while counting once toward `withKey` —
  `withKey − shown` then goes *negative*. Found against the real Interaction Log,
  not imagined. How many values were folded away goes in the note, and where the
  bars do add up to more than their denominator the note says so.
- **Missing days are filled with zeros.** The query returns only days that have
  conversations, so a quiet week would close up and read as uninterrupted
  activity at a steady rate. `insFillDays` is string arithmetic over UTC dates —
  no `Date` with a local offset can creep in.
- **The heatmap week starts on Monday**, which `strftime('%w')` (0 = Sunday) does
  not; the row index is `(wd + 6) % 7`.
- **Hours are read as a substring of the stored timestamp**, not through
  `strftime`. The database stores naive UTC by design, so the substring *is* the
  UTC hour and no timezone can be applied to it by accident.
- **A recognition band is the conversation's worst scored turn**, the same
  measure the Low % and Zero % pills filter on, so a chart and a pill on one
  screen cannot disagree about the same conversation. In the interactions
  reading there is no worst-of to take — the score on the row *is* the answer,
  which is the sharper reading of the two.
- **A card with no data is not rendered.** An axis with no marks reads as a
  rendering fault, not as an empty result.
- **Every tooltip ends in a counted noun** via `insPlural` — `1 conversations`
  reads as a bug in the number rather than in the sentence.

## Copying

Three forms, all of them the point of the feature rather than a convenience on
the end of it.

- **`ClipboardItem` is handed a *promise*, never a resolved blob.** WebKit ties
  clipboard writes to the user gesture that started them, and rasterising an
  image outlives a gesture. Constructing the item synchronously in the click
  handler and letting it resolve afterwards is the only form that works in both
  engines.
- **Copy image** re-renders the card in `INS_THEME_EXPORT` at 2× and puts a PNG
  on the clipboard. An image cannot degrade to text, but the numbers can, so a
  refused write falls back to the chart's table.
- **Copy data** is the chart's table twin (`insChartTsv`) — tab-separated, so it
  pastes into Excel as cells. Every value on this dashboard is reachable without
  looking at a colour, and `every chart has a table twin carrying all of its
  values` asserts exactly that, per card.
- **Copy dashboard** writes `text/html` *and* `text/plain`: the HTML carries the
  tiles as a table and every chart as an embedded PNG data URI (measured at
  ~620 KB over 16 charts, built in ~100 ms), the plain text carries every number
  for Slack and for anything that refuses HTML. Mail clients strip `<style>`
  blocks and classes, so every element is styled inline and the layout is tables.
- **The progress toast only appears once the work has genuinely been slow**
  (`LOADING_SHOW_DELAY_MS`, the same threshold as `gateLoading`). Sixteen charts
  rasterise in about a tenth of a second, and a toast replaced by the success
  toast inside that window is the flicker every other loading affordance in this
  file exists to avoid.
- **The search description is not decoration.** "43% thumbs down" is alarming or
  unremarkable depending entirely on what was searched for, so `insSearchSummary`
  is in the header, in the HTML report and in the plain-text report — and it
  never returns empty (an unfiltered view says so in words).

## The chooser

Opening Insights used to fire every aggregate at once. On a wide search that is
several seconds in which the modal shows a spinner and nothing else — for
sixteen charts, most of which scroll past unread. It opens on a chooser instead:
what to count, and which of the five sections to build.

Measured on the seeded 120k-interaction / 13.3k-conversation database
(`perf::insights_cost`), the cost of asking for one section rather than all
three read sections:

| | all sections | one section | the same again |
| --- | --- | --- | --- |
| conversations, no filter | 537 ms | **23 ms** | 12 ms |
| conversations, a search term | 599 ms | **88 ms** | 12 ms |
| interactions, no filter | 667 ms | **212 ms** | 139 ms |
| interactions, a search term | 785 ms | **333 ms** | 139 ms |

(The third column is the same read against a result set already resolved — see
`The result set is resolved once, not once per read`. On the search-term row
it is the whole difference between 88 ms and 12 ms.)

- **`INS_SECTIONS` is the single menu**: each section's name, what it answers,
  what it costs, and — for the three the dashboard read covers — which payload
  fields carry it. The renderer filters cards by it, the loading pane names the
  chosen sections from it, and `insAddSection` merges by its `fields`.
  `the chooser and the cards agree on what a section is` pins the two halves
  together: a section renamed on one side and not the other makes every card in
  it silently disappear, with no error and no empty state.
- **Cards are filtered by the choice, not by emptiness.** A section that was not
  read comes back as empty arrays and most of its cards drop out on their own —
  but **Feedback** is built from the headline counters, which are always read.
  Filtering by emptiness would leave that one chart stranded under a heading for
  a section nobody asked for. `a section that was not chosen contributes no
  card, not even a derived one` is built around exactly that card.
- **The two tag sections are off by default** and marked *slower*: they are the
  expensive half, and they say nothing until a key is chosen. Volume, Quality
  and Content are on, because between them they answer what people open this
  for.
- **What was left out is offered at the bottom of the dashboard**, not only back
  on the chooser. A section nobody selected is invisible on this screen, and
  "the chart I wanted isn't here" needs an answer on the screen it is missing
  from.
- **Adding a section is not a reload.** `insAddSection` fetches only that
  section and merges its fields into the payload already being drawn — the
  result set is still resolved, so the fetch costs the third column above. A
  reload would re-run every aggregate already on the screen.
- **The chooser means "nothing has been read"**, so `insShowSetup` clears
  `insData`. Keeping it would leave Copy dashboard live over a dashboard that is
  no longer on screen.
- **The header's unit toggle works in both stages**, and in the chooser it is a
  choice rather than a refetch: nothing has been read, so it costs nothing and
  simply redraws.
- The selection is remembered in `cm-insights-sections`, read key by key so a
  file written by an older build cannot introduce one and a hand-edited one
  cannot introduce any. All-false falls back to the default — it is not a state
  the chooser can act on.

## The result set is resolved once, not once per read

`build_session_filter_query` — an FTS match, an entity scan, a materialized
feedback relation — is most of what an Insights read costs, and every read after
the first asks about the **same search**: another section, another tag key, the
other unit. Re-resolving it for each of those is re-running the search the user
already ran.

So the temp tables stay on the connection and `InsightScopeCache` records what
is in them. On the search-term case above that is **76 ms of an 88 ms read**.

- **Two fingerprints, not one.** The session set and the matching turns
  invalidate on different things: switching the unit rebuilds `insight_matches`
  and `insight_weights` and leaves the expensive `insight_sessions` exactly
  where it is (`keep_sessions`).
- **`total_changes()` is the safety catch**, and it is why no write path in this
  file has to know the cache exists. SQLite's own counter moves on any insert,
  update or delete this connection makes — an import, a deletion, a purge — so
  the fingerprint stops matching on its own. Recorded *after* the tables are
  built, since building them is itself a write. A failed read of the counter can
  never look like a hit: a hit requires `Some` on both sides.
- **Reuse that is too eager is the dangerous half.** A stale hit would chart
  rows that are no longer the result of that search, and the numbers would
  simply be wrong with nothing on screen to say so.
  `a_second_read_of_one_search_reuses_the_resolved_result_set` asserts both
  directions, including that an import ends the reuse.
- **A failed or interrupted read drops the tables *and* clears the cache**, in
  each of the three commands. An interrupt lands mid-build, so what is on the
  connection describes nothing — and a cache still pointing at it would reuse
  half a result set.
- **`release_insight_scope` is memory, not correctness.** The fingerprint would
  have caught a stale set anyway; this hands the pages back when the modal
  closes. `set_db_path` clears the cache too, because the connection the tables
  lived on has gone.

## Why it is two reads, not one

Building fires **`get_conversation_insights`**, and the renderer draws the
answer; **`get_insight_tags`** follows, unawaited, and fills the Context and
Metadata sections in — when they were chosen. Those two sections were more than
half of what a read cost and they draw two charts below the fold, so charging
the whole dashboard for them meant nothing at all appeared until they were
done.

Measured on a seeded 120k-interaction / 13.3k-conversation database
(`perf::insights_cost`), against the single-read version:

| | was | first paint | tags fill in |
| --- | --- | --- | --- |
| conversations, no filter | 1254 ms | **542 ms** | +590 ms |
| interactions, no filter | 4836 ms | **672 ms** | +649 ms |
| interactions, with a search term | 4951 ms | **792 ms** | +770 ms |

- **The two tag sections always render, chart or not** — their key bar is the
  control that gets a chart back, so a section that vanished while its key was
  loading would take the way out with it. `insTagPendingHtml` distinguishes
  *still reading* from *read, and there is nothing*, which both arrive as an
  empty list; `tagsLoaded` is the only thing that tells them apart.
- **Both tables come back in one call**, so the result set is resolved once —
  and, since the dashboard read resolved the same search moments earlier, that
  resolve is now normally a cache hit rather than the 193 ms it used to be with
  a search term.
- **Each table is read only if its section was chosen** (`contextOn` /
  `metadataOn`). A key of `None` means "pick the most-covering one", which is a
  different thing from "do not read this table" — hence the separate flags.
- **`insLoadTags` is guarded three ways** — a newer read (`insLoadSeq`), a
  cancelled read (`null`), and a payload since replaced (`insData !== d`).

## Cancelling

A read holds the conversations connection's lock for its whole duration, so an
Insights read nobody wants any more blocks the next search as well as itself.

- **`cancel_db_query` is the existing session-search interrupt, renamed.** One
  connection behind one mutex means at most one statement is ever in flight, so
  there is nothing to address — a search and an Insights read cannot both be
  running. SQLite makes the call a no-op when nothing is, which is what makes it
  safe to fire on a modal close without racing the query that just finished.
- **An interrupt is an outcome, not a failure.** `insight_err` maps
  `OperationInterrupted` to the `INSIGHTS_CANCELLED` sentinel and the command
  turns that into `Ok(None)` — `null` in the renderer. Reported as an `Err` it
  would have to be recognised by its message to avoid painting a red failure
  over a modal the user is already closing.
- **`insStopRead` does two things and needs both**: the interrupt frees the
  database, and bumping `insLoadSeq` stops the answer painting over whatever
  replaced it whenever it lands. Cancel, closing the modal, and starting a
  second read all go through it.
- **The unit toggle stays live during a read.** The whole point of being able to
  stop one is changing your mind about it; a toggle that goes inert for the
  duration makes you wait out the answer you no longer want.
- **The temp tables are dropped on any non-success**, in the command rather than
  in `conversation_insights`, and the scope cache is cleared with them — an
  interrupt lands mid-build, and they live on a connection that outlives the
  call. A *successful* read deliberately leaves them; see
  `The result set is resolved once, not once per read`.

## The backend

`conversation_insights(conn, &args, unit)` is split out of the command — as
`sessions_page_sql` and `write_ai_export` are — so the queries can be run against
a real `Connection` in a test without a Tauri `State`. `conversation_insights_timed`
is the body, handing back what each step cost; a slow run logs the breakdown
under `target: "insights"` and `perf::insights_cost` prints it.

- **The filtered set is resolved once into a temp table** (`insight_sessions`),
  and every aggregate joins to that. The filter query is genuinely expensive — an
  FTS match, an entity scan, a materialized feedback relation — and there are
  ~15 aggregates; running them against the CTE would re-derive the result set
  once per chart. `resolve_insight_scope` is shared by all three commands. The
  tables now *survive* a successful call on purpose — see
  `The result set is resolved once, not once per read` — and
  `a_second_run_does_not_see_the_first_runs_result_set` pins the rebuild path
  instead: a read that does *not* reuse them must replace their rows, never
  append to them.
- **`insight_weights` is what one conversation is worth**: 1, or the number of
  its turns that matched. Context and metadata are keyed by conversation, so the
  interactions reading joined `insight_matches` — one row per matching turn — to
  a table keyed by session, multiplying the join by each conversation's length
  before the `DISTINCT` collapsed it back. **2.1 s per tag table against 0.34 s
  for the identical answer in conversations mode.** Summing a weight over the
  session set gives the same numbers over a join an order of magnitude smaller.
  - **A table of its own, not a column on `insight_sessions`, and the narrowness
    is the point.** The tag joins read the weight once per joined row — half a
    million times — and a column on `insight_sessions` means decoding a
    ten-column record whose widest field is the conversation's opening message:
    421 ms per table against 357 ms, for the same answer.
- **`article_ids` and `dialog_paths` are each read in one pass, not two.**
  `article_ids` holds Articles (`qa-`) *and* the Dialog nodes an answer came
  from (`dn-`); `dialog_paths` holds the Dialog walked through *and* the outcome
  key beside it. Four charts, two columns — and parsing the same JSON twice was
  the third-largest cost in the run. `insight_split_buckets` ranks within each
  kind and splits the rows apart again.
  - **`CROSS JOIN` against a two-row relation is load-bearing** in the
    `dialog_paths` query: it fixes those rows as the inner loop so `json_each`
    is walked once and each entry yields both readings. As a plain comma join
    SQLite is free to put the two rows outside and parse the JSON twice.
  - **So are the `bucket_` alias names.** `json_each` exposes columns called
    `key`, `value` and `type`, and a `GROUP BY value` binds to the *real column*
    in preference to an output alias of the same name — so the obvious spelling
    grouped by the raw path rather than by the Dialog cut out of it, and
    `6391:2` and `6391:2/15/4` counted as two Dialogs.
    `articles_dialog_nodes_and_walked_dialogs_are_counted_apart` caught it.
- **Only the charted key's values are read.** One chart is drawn at a time, and
  reading every key's cost a second full scan of the join to return ~40 keys ×
  12 values per table — 150 ms per table against 23 ms for one key, which
  `idx_ctx_name_session` turns into a range scan. It also stops the cost growing
  with a customer's key count. The renderer asks for another key through
  `get_insight_tag_values` and caches what comes back on the group, so going
  back to a key is free.
- **Per-key coverage dedupes with a `GROUP BY (name, session)`, not a
  `SELECT DISTINCT`** — `idx_ctx_name_session` is `(name, session_uuid)`, so
  grouping in that order is a covering-index scan with nothing to sort, where
  the DISTINCT form built a temp b-tree over the whole join (213 ms against
  377 ms). A conversation whose context changed mid-way legitimately carries two
  values of one key, which is why the dedupe is needed at all.
- **The distinct-value count is a nested `GROUP BY (name, value)` and a count of
  the groups**, not the `COUNT(DISTINCT t.value)` it reads as: the tag table's
  primary key is `(name, value, session_uuid)`, so that order is another ordered
  index walk — 101 ms against 316 ms.
- **Tag values are capped per name** (`INSIGHT_TAG_VALUES_PER_NAME`), for the
  same reason `tag_option_rows` caps them: one key with thousands of values would
  otherwise consume a global budget and leave every other key with nothing.
- **`perf::insights_cost` is the harness**, and every number above came out of
  it rather than off a query plan. Three of the changes tried here were *slower*
  than what they replaced and were reverted; the CLI's planner disagreed with
  the bundled SQLite's twice. `CAI_BENCH_ROWS` sizes the seeded database,
  `CAI_BENCH_KEEP` leaves it behind to shape a candidate query against, and
  `CAI_TEST_DB` points it at a real one.
- `the_aggregates_partition_a_real_interaction_log` runs the real portal CSV
  through the real import and asserts the invariants a wrong join breaks — every
  per-conversation breakdown must add back up to the result set, and the result
  set must equal what `sessions_page_sql` would report as its total. It found
  both of the real bugs above. It runs **both readings**, which is where the
  "what answered" joins change shape from `session_uuid` to `log_id` and a
  mis-keyed one still returns a plausible number. It skips when the sample export
  is not checked out beside the app. Measured: 287 conversations, 2,177
  interactions, 42 context keys, 39 metadata keys — 47 ms.
