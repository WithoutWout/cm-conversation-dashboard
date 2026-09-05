# Search semantics

The authoritative rules for content search, conversations search, tag filtering and entity cross-references.

_Split out of `CLAUDE.md`. Read this before changing anything it covers._

---

## Content search semantics

- `search-worker.js` is the source of truth for result inclusion. Renderer helpers may mirror parts of search only for snippets, highlights, and modal display.
- Plain search supports space-separated AND terms, `|` OR groups, quoted exact phrases, case sensitivity (`Aa`), whole word (`\b`), and regex (`.*`).
- Invalid regex mode returns an explicit `invalid_regex` result from the worker; the renderer must show that as an error state, not as a valid zero-result search.
- When content context filters and a text query are both active, the same answer output must satisfy both the context filter and the text query.
- `¬T` means **Responses only**. When enabled, search excludes IDs, titles/names, descriptions, node names, and entity enrichment.
- `ND` means **Exclude non-default responses from search**. It only affects matching when a text query is active and must not hide items for an empty query.
- A response is user-facing unreachable only when it is not the default response and it has no context condition. Non-default responses with context are reachable for users in that context and should not be labeled "non-default" or "unreachable" in result cards.
- Contextual/non-default query hits should show a compact snippet or reason on result cards so users can see why an item matched without opening the modal.
- The info modals' **Matches only** toggle (`modalMatchFilter`, `toggleModalMatchFilter`) must use the same answer/node sections that caused worker result inclusion. It is hidden entirely when no query is active — it can do nothing then, and a permanently greyed-out control reads as broken rather than inapplicable.

## Conversations search semantics

`build_session_filter_query` in `lib.rs` is the single source of truth. The search bar's contents are one boolean expression (see "The search bar is one expression" below); every leaf of it — a text run, an entity, an Article, a Dialog, a node — resolves to a relation producing `(session_uuid, match_log_id)`, and the operators compose those relations. `conv_search` in `lib.rs` is the test module; every test there is a bug that was invisible in the SQL and only showed up in the rows that came back.

**A column filter always applies to a parenthesized expression.** In FTS5 `interaction_value : a OR b` binds the filter to `a` alone and lets `b` match any column — which is why a **U**-scoped (user) search kept returning conversations where only the bot had said the word. `fts_columns()` is the only place a colspec is built and it always emits `{cols} : (expr)`. "Both" is a column filter too: with none at all, a plain query also matched `article_ids`/`dialog_paths` and a number found conversations by their dialog path. Those two columns are an id leaf's business now and never a text leaf's, so `fts_columns` no longer takes a flag for them at all.

**Search terms are never emitted as FTS5 barewords.** `www.efteling.nl` or `qa-1234` unquoted is an FTS5 **syntax error**, so the whole search failed rather than the one term. Terms are tokenized (`fts_tokens`) and re-emitted as a quoted phrase, with the trailing `*` kept for single-word prefix matching.

- **A query FTS cannot express falls back to `LIKE`, never to nothing.** When a term produced no tokens at all (a lone `€`, `?`) the old code left the search CTE unbuilt, so the query silently returned *every* session — indistinguishable from a search that matched everything on purpose. A group with an inexpressible term now goes down the LIKE path as a whole.

**Punctuation gets an exact re-check on top of the index.** The tokenizer drops it, so `www.efteling.nl` and `www efteling nl` are the same FTS query. `term_needs_exact_check` flags terms carrying punctuation and the search adds a `LIKE '%term%'` on the stored text. Whitespace deliberately does not count: a quoted phrase already matches as adjacent tokens, and requiring byte equality there would throw away the diacritic-insensitive matching the tokenizer gives for free (`cafe` finding `café`).

- **The exact re-check is per OR group, which is why groups get their own SELECT.** Folded into one MATCH it would either be skipped for rows that matched a different group or wrongly applied to them. The single-MATCH fast path is still taken when no group needs a re-check, which is the common case.

**An id leaf is FTS-narrowed and then decided exactly.** The FTS lookup is a *filter*, not the answer: `IdTarget::matches` compares whole ids, so `qa-123` no longer also matches `qa-1234` and `dn-6391-4` no longer matches `dn-6391-42`. Measured on a real 110k-interaction database: **5.0 s → 3 ms** for an Article id and **1.4 s → 1 ms** for a Dialog id. `an_id_search_on_a_real_database_matches_an_exhaustive_scan` (needs `CAI_TEST_DB`) pins the narrow+decide pair against a boundary-exact full scan; `search_perf::search_cost` is the timing harness.

- **Dialog and Node are one leaf, and the `-` is what tells them apart.** `IdTarget::parse` reads `dn-6391` as a Dialog and `dn-6391-4` as one of its nodes; an Article has no nodes, so a `-` after `qa-` is a typo rather than a node search.
- **The prefix is mandatory.** There used to be a bare spelling whose meaning came from a "Search by:" pill row, which is how `1418` could mean an Article. In one field holding text and ids at once a bare number can only be text — the suggestion list offers `qa-1418` and `dn-1418` for it instead, which is a question rather than a guess.
- A Dialog matches an interaction that answered from one of its nodes (`article_ids`, `dn-<dialog>-<node>`) **or** merely walked through it (`dialog_paths`, `<dialog>:<node>/<node>/…`). `path_has_node` checks the dialog and the node together, so a second path in the same cell cannot contribute the dialog while another contributes the node.

**Entities are a search field, not a subset of the text.** The recognizer stores the entity it matched, not the wording that triggered it, so "mag ik een fles rood meenemen" is found by searching the entity `WIJN`. The **E** toggle sits with **U**/**B** and is independent of them; turning both message toggles off sends `queryScope: "none"`, which means entity-only. Nothing at all selected falls back to searching the text — an empty result set would read as "no matches" rather than "you switched it off".

- **The default is U + E** — what someone asked for, and what the bot understood it as. Both the markup (`class="conv-scope-btn active"`) and the initial state say so, and they must agree: `_syncConvScopeFromButtons` reads the buttons, so a disagreement at startup silently wins for whichever the first click resolves to. `setConvSearchScope(user, bot, entity)` is the only programmatic way in, and it goes through the same read-back.
- **The Entities tab has a 💬 Conversations button** (`entityConvButton` → `searchConvsForEntity`) on every entity card and in the entity modal, which is the only way to answer "which conversations actually fired this entity?". It switches to entity-only deliberately: the entity's *name* is a label the recognizer assigned, so searching the message text for it returns a different — usually much smaller — set.
- **The opened chat filters to the turns that matched.** `chatMatchEntities` mirrors the E toggle when a session is opened (the way `chatSearchRegex` already mirrors `.*`), and `turnMatches` then also tests `rowEntityFields(row)` — display name, internal name, matched text, entity id, cached on the row because `recognition_details` is a sizeable blob and a long chat re-renders on every filter change. Without it an entity-only search opens every result on "no messages match".
- **The entity chip that caused the match is marked** (`.is-hit`, accent-coloured like `<mark>`). And a GenAI row — which normally hides its recognition data because that data explains something other than the answer, see `docs/chat-rendering.md` — shows the entity anyway *when it is the hit*. Suppressing it there would leave the turn in the results with no visible reason at all.

- `entity_index` (`log_id`, `session_uuid`, `entity_id`, `name`, `matched`) lifts `recognition_details.entityMatches` out of the JSON at import time; searching it is a scan of a small narrow table instead of a JSON parse of every interaction. `name` is the `displayName` (falling back to the internal `name`), lowercased on the way in so the search side never has to. A bare number also matches `entity_id`.
- **It carries no secondary index on purpose.** The search is a substring `LIKE`, which no index can serve, and every extra b-tree is a per-row tax on import; `WITHOUT ROWID` keeps it to one write per entity match. On a real database it holds ~103k rows for 110k interactions and an entity-only search costs ~8 ms.
- **Deletion must remove entity rows too** — `purge_old` and `delete_interactions_by_dates` both do, alongside the FTS cleanup.
- The one-time backfill in `open_db` is gated on `META_ENTITY_INDEX_BUILT`, not on "is the table empty?": a database whose interactions genuinely triggered no entities would otherwise re-run the whole-table `json_each` scan on every launch. `the_entity_backfill_matches_what_an_import_would_have_indexed` asserts the SQL backfill and `entity_index_rows` agree, so an older database searches the same as a freshly imported one.

**A feedback filter narrows the search to the answer the thumb was about**, so "thumbs-down on answers mentioning X" is one query rather than two. `feedback_origins` resolves each feedback row to the interaction it rated — via `originatingInteractionId`, falling back to the previous bot output — and the search then only looks at those rows. That restriction is cheap to express and was, twice over, ruinous to execute.

- **`feedback_origins` must stay `AS MATERIALIZED`.** It is a constant relation, but SQLite is free to inline a CTE, and with a search term present it did: the plan drove from `SCAN interactions_fts` and recomputed the whole relation — JSON-parsing scalar function and correlated subquery included — **once per matching row**. With no search term the planner already materialized it, which is exactly why the pill alone was fast and the pill *plus* a query never came back at all.
- **The restriction is an `IN`, not a `JOIN`, and that is the second half.** Joining `feedback_origins fo ON fo.match_log_id = i.log_id` left the plan doing a full `SCAN fo` per matching row — still n × m, just with a smaller m. As an `IN` SQLite builds one ephemeral index and probes it, and the join order stops mattering. It also collapses duplicates, which `GROUP BY session_uuid` was discarding anyway.
- Measured on a 120k-row / 13k-session database with a term matching every row: **never finished → 3.6 s materialized → 31 ms as an `IN`**. `perf::conv_filter_cost` is that harness — it times every pill alone and combined with a search, and prints `EXPLAIN QUERY PLAN` for the combined cases.
- `a_feedback_search_never_re_derives_its_origins_per_row` pins the plan rather than a duration — a timing threshold in a test is a flake waiting to happen. `a_feedback_search_only_matches_the_answer_the_feedback_was_about` pins the semantics the fix had to preserve, since an `IN` that admitted too much would silently widen the search instead of hanging.
- `sessions_page_sql` is split out of `get_sessions` so the harness times the query the app really runs. Timing `SELECT COUNT(*) FROM filtered_sessions` instead is misleading by a wide margin — the count never sorts, and it touches `filtered_sessions` once where the real query touches it twice.

**The other pills never had this, and the reason is structural.** GenAI and both recognition filters express themselves as an inline `AND` on the row the search already fetched (`search_row_filter`), so there is nothing for the planner to re-derive per row — their plans are flat, every step a `SEARCH` on an index. Feedback is the only filter that routes the search *through* a relation, because the answer a thumb was about has to be resolved before it can be searched. Verified rather than assumed: `perf::conv_filter_cost` covers all five pills alone, with text, and with entities, and nothing exceeds ~70 ms on the 120k-row database.

- The recognition pills do have a CTE of their own (`recognition_matches`), but only on the *no query* path, where it is referenced once and materialized once — `total` and `page_rows` both probe it through an automatic index. Combined with a search the CTE is not built at all.

**Chat search matches what the session list matched.** The FTS index is built with `remove_diacritics`, so searching `cafe` returns conversations containing `café`; the in-chat search was plain JS `includes` and found nothing in them, which reads as the chat being broken rather than as two searches with different rules. `foldDiacritics` folds both sides.

- **It folds one character at a time, and that is the whole trick.** The folded string is exactly as long as the input, so an index into it is an index into the original — which is what lets `_chatMarkSegment` find matches in the folded copy and write `<mark>` into the real text. A whole-string `normalize("NFD").replace(/\p{M}/gu, "")` would be shorter than its input and misplace every mark after the first accent. Anything that is not a base-plus-marks decomposition (a lone surrogate half included) passes through untouched, which is what preserves the length.
- Ranges are merged before insertion, longest-first at a given start: two terms can cover the same span, and `<mark>` nested in `<mark>` renders wrong. `opening | openingstijden` produces one mark, not `[opening][stijden]`.
- Regex mode is deliberately **not** folded — the user wrote a pattern, and the backend's regex path does not fold either.

## The search bar is one expression

The conversations bar holds free text, entities, Articles, Dialogs and Dialog
nodes at once, joined by operator chips you can click and grouped with brackets.
"Conversations that walked through the parking Dialog and where someone was
angry" is one search rather than two you intersect in your head.

It used to be two mutually exclusive halves. `#ID` on searched ids and nothing
else; `#ID` off searched text and entities and nothing else. Underneath, the
reason was structural: `base_conditions.join(" AND ")` was the only AND and
`match_selects.join(" UNION ALL ")` was the only OR, and they sat on opposite
sides of the join between them — so a text predicate and an id predicate could
never meet.

### The grammar

```
expr   := term ( op term )*                 -- left to right, no precedence
op     := "AND" | "OR" | "AND NOT" | "NOT"  -- a bare NOT between terms is AND NOT
term   := "(" expr ")" | "NOT" term | leaf
leaf   := qa-<n> | dn-<n> | dn-<d>-<n>      -- an IdTarget, unquoted
        | entity:<word> | entity:"<phrase>"
        | <text>                            -- the term grammar above, verbatim
```

**Evaluation is strictly left to right; brackets override.** `qa-1 OR qa-2 AND
boos` is `((qa-1 OR qa-2) AND boos)` — what someone reading a row of chips left
to right means. There is no precedence to learn, and it does not collide with the
older `a b | c d` rule, because that rule lives *inside* one text leaf, a layer
below these operators. `parse_search_expr` folds a run of one operator into a
single n-ary node, so three Articles are one `UNION ALL` and not two nested ones.

- **The keywords are upper-case only.** `and` is a word people search for in both
  languages this app sees, and a case-insensitive keyword would quietly turn half
  of "brood and boter" into an operator. Quoting escapes anything in either
  direction: `"AND"`, `"dn-6391"` and `"(gratis)"` are all text.
- **`|` is not an expression operator.** It stays the OR separator *within* one
  text leaf, because for pure text the two are the same relation — so nothing is
  lost and one character keeps one meaning.
- **The renderer serialises with explicit brackets** even though its model is
  flat, so the string in the Insights chip and in an AI export says what it means
  without the reader knowing the left-to-right rule.
- **Malformed input is read as generously as possible**, never rejected: a
  dangling operator is dropped, an unclosed `(` closes at the end, a stray `)` is
  ignored. With five chips up, one typo must not silently turn the other four
  off. The renderer's field cannot produce any of these, but a pasted string can.
- **`GetSessionsArgs` gained nothing.** It is the Insights scope-cache
  fingerprint, and `lastConvSearchArgs`, the AI export and the chat query mirror
  all read it, so a tree-shaped field would have had to be threaded through four
  places to say what the query string could already carry. What it *lost* is
  `query_ids`, `query_ids_only`, `query_id_type` and `entity_filters` — the
  expression says all four. The AI export header is `schema_version` 6 and its
  legend states the grammar; a file that does not describe its own filter
  language is misleading to the model reading it.

### AND is conversation-level

`dn-6391 AND boos` means the conversation walked through 6391 *and* some turn
says "boos" — not necessarily the same turn. The operators combine **searches**,
and a search returns conversations; requiring both in one interaction would
answer a much rarer question and return almost nothing.

Several words *inside one text leaf* keep their old meaning — one message holding
all of them, FTS implicit AND — because that is a term, not two searches.
`a_dialog_and_a_word_need_not_share_a_turn` pins the distinction.

Exclusion is the mirror of it: `parkeer AND NOT duur` removes the whole
conversation, not the turn. "Conversations about parking that never mention the
price" is meaningless read turn by turn.

### One CTE per node

Each node of the tree is emitted as a named CTE producing
`(session_uuid, match_log_id)`, which is what makes the operators composable at
all: **OR is `UNION ALL`**, **AND is that union filtered by an `IN` per
conjunct**, and **NOT is the complement within `base_sessions`**.

- **Negation is carried, not materialised.** `A AND NOT B` becomes one `NOT IN`
  on A's rows rather than a scan of every conversation minus B. It only becomes a
  relation of its own where it has to be — inside an OR, or as the whole search,
  where `base_sessions` is the universe and no other case needs a special rule.
- **A node read twice is written once, and marked `AS MATERIALIZED`.** An AND
  reads each conjunct once for its rows and once to intersect on; inlined,
  SQLite would re-run that leaf's FTS match per row of the other side — the same
  n × m shape that made `feedback_origins` unfinishable.
  `a_conjunct_is_never_recomputed_per_row` asserts the plan rather than a
  duration, because a timing threshold in a test is a flake waiting to happen.
- **A single un-negated leaf is spliced inline instead**, which reproduces the
  SQL this file emitted before expressions existed — statement-cache key and
  query plan included. Every pre-existing search takes that path, so "nothing
  changed for a plain search" is a property of the code rather than of thirty
  tests that happen to agree. `one_leaf_compiles_to_the_sql_it_always_did` pins
  it from both sides.
- **`match_rows` is now a reference to the root**, not a second textual copy of
  the whole search sharing its `?N` numbering by hand. Each node's parameters are
  allocated exactly once, which retires that trick entirely.
  - It is `None` when the tree holds no positive leaf. Only a positive leaf can
    point at a particular interaction — no turn is the reason a conversation
    *lacks* a word — and Insights would otherwise read "zero matching
    interactions" where the truth is "nothing singled any of them out".
- **Every leaf's WHERE is parenthesised before the row filter is appended.**
  `WHERE {conds}{row_filter}` only works while `{conds}` is an AND-chain, and it
  already was not in the `like` branch: `WHERE (a) OR (b) AND <row filter>` bound
  the `AND` to the last disjunct alone. That was a live precedence bug with a
  GenAI or recognition pill plus an unindexable term.
- **The per-leaf scalar functions are indexed.** `create_scalar_function` keys on
  **(name, arity)**, so one closure per leaf silently overwrites its predecessor
  and every leaf ends up searching for the last one. `cai_id_hit` learned this
  when one query string first carried several ids; `regexp` had exactly the same
  shape with its pattern baked in, and is now `cai_regexp(idx, text)` over a
  tree-global vector. `two_regex_leaves_do_not_collide` is what catches it.

### An entity is a leaf, not a filter

`query_entities` (the **E** toggle) is a boolean saying "also match this text
against entity name, matched text and id" — a substring search over *typed
words*. `entity:camper` is the exact label the recognizer assigned. Picking an
entity by name is not the same question as searching for a word that might
appear in one, and `an_entity_leaf_is_exact_where_the_e_toggle_is_a_substring`
pins the difference.

**E is not redundant now that `entity:` exists, and it is not always on in
effect.** On a real 150k-interaction database it widens a text search by 8–35%
— `parkeren` goes 335 → 452 user-scoped, `openingstijden` 149 → 191 — and
turning it off is the only way to ask what people actually *typed*. It is also
what `searchConvsForEntity` uses, with `query_scope: "none"`, to answer "which
conversations fired this entity?" from an entity card.
`the_entity_toggle_still_changes_what_a_text_leaf_matches` pins both halves.

- **The suggestions are deliberately *not* gated on it.** An `entity:` condition
  works whatever E says, so gating the type-ahead on E left the field unable to
  offer a chip that would have worked perfectly well once placed — E off meant
  no entity could be *found*, while one already there kept filtering. The
  toggle governs what a typed word matches, and nothing else.
- **U / B / E are a property of the search, not of one condition**, so every
  text leaf reads them. `the_scope_toggles_still_govern_every_text_leaf` asks
  that of the compiled tree, which is the path that could have dropped it; the
  single-leaf path is spliced inline and covered by
  `a_user_scoped_search_never_matches_the_bot_side`.
- **They still do not re-run the search on their own**, unlike `.*` and the
  operator chips. That is deliberate and predates the expression: setting a
  scope is usually two clicks (U off, B on), and firing a search on each would
  run an intermediate query nobody asked for.

Entities used to be `entity_filters`, an `IN` in `base_where` written from a
funnel tab. As a leaf they can be ORed, ANDed, grouped and excluded like anything
else — and the funnel's Entities tab is gone, because a list you scroll cannot
do any of that and the bar's type-ahead already outranks it.

- **The leaf is a single uncorrelated pass over `entity_index`**, which is the
  whole reason it is fast. That table deliberately carries no secondary index —
  every extra b-tree is a per-row tax on import — so a correlated
  `ei.session_uuid = s.session_uuid` had nothing to seek on and re-scanned all
  81k rows *per candidate session*: **23.4 s** over 8395 sessions, against
  **8 ms** for the same rows in one pass. `an_entity_leaf_never_rescans_the_index_per_session`
  counts the scans rather than looking for one, because "scanned once" is the
  property and "scanned" is not.
- **It also says which turn fired the entity**, which the `IN`-shaped filter
  never could: the leaf carries a `match_log_id`, so the preview and the opened
  chat can point at the turn instead of falling back to the first message.
- Names are stored lower-cased (`entity_index_rows` lowercases the display name
  and the matched text) and the suggestions come from that same column, so they
  arrive in the right case already; the leaf lower-cases again rather than
  trusting what crossed the bridge.
- **`get_entity_options` is still one full scan of `entity_index`**, run once
  when the field is first focused and cached in the renderer, exactly as the
  context and metadata options already are. The same scan carries `entityId`,
  which is the only place in the app an entity id exists — the EntitiesExport CSV
  has no id column at all. See `docs/collections.md` and `cmLink("entity", …)`.

### The field

The renderer's model is a flat, ordered list of exactly what is on screen —
chips, the operator between each pair, and bracket tokens — and the wire string
is its serialisation. It never builds a tree; `parse_search_expr` does that on
the far side. `normalizeConvExpr` is what keeps the list readable (one operator
between adjacent conditions, none against a bracket it cannot join across, no
empty groups, no closer without an opener), which is why the search runs with no
validation step in front of it.

- **Committing puts the typed text in a chip.** One rule with no exceptions: the
  field always shows the committed search, and the box is always "what's next".
  Typing a sentence and pressing Enter is the same two keystrokes it always was
  and returns the same rows; the words simply land in a chip you can click to
  edit in place, which is what makes fixing a typo in the first of five
  conditions an edit rather than a retype.
- **Enter no longer takes the first suggestion.** The list used to open with row
  0 highlighted, so typing a word that happens to name an entity and pressing
  Enter added a chip instead of searching — the plain-sentence case this field
  has to keep cheap. Nothing is highlighted until you move: `Enter` searches,
  `↓`/`Tab` then Enter picks, and the hint line says which of the two Enter is
  about to do.
- **The operator chip appears by itself between adjacent conditions** and cycles
  `and → or → and not`. Nobody has to insert one; everybody can change one. It is
  drawn lower-case and quiet because it is connective tissue — set in caps it
  shouted over the chips it joins.
  - **The default is guessed from the kinds either side**: two Articles, or two
    entities, almost always mean OR; a Dialog and a word almost always mean AND.
    Guessing is worth doing because it is one click to undo.
  - **A leading exclusion is a separate, dashed, nearly transparent toggle.**
    Drawn like the operators beside it, it read as *already applied*, which is
    the one thing it must not do. It is only needed when the whole search is a
    negation — an exclusion anywhere else can be written after the thing it
    narrows, and AND commutes.
- **A matched bracket pair is drawn as one nested band**, rebuilt from the flat
  list on every render: which conditions are *inside* the group is the whole
  question a bracket answers. The brackets themselves stay visible because that
  is what someone asked for when they asked for parentheses.
- **The `( )` button opens a group, or closes the one that is open**, and typing
  `(` or `)` at a term boundary does the same. Groups are built as you go rather
  than wrapped around chips after the fact — with left-to-right evaluation the
  only shape that needs one is `a AND (b OR c)`, and that is exactly the order
  you type it in.
- **Under `.*` the brackets belong to the pattern.** `a(b|c)` is a group the
  *user* wrote, and reading it as ours would silently search for two different
  things. The operators still apply, so an id or entity leaf can sit beside a
  pattern; only grouping is unavailable, and the `( )` button says so by being
  disabled. A group already built is dropped when `.*` goes on, rather than left
  to serialise brackets the pattern would then swallow.

### The suggestions

Both feeds are already in memory, so a keystroke costs no IPC. The content index
is built once per `loadData` from `allCombinedItems` (~4100 rows, 4.2 ms) and
invalidated with it; entities come from the cached `entityOptions` plus their
export trigger words, warmed on first focus by the existing idempotent
`ensureEntityIdMap`. A query is 0.06–0.7 ms and about 1 ms for a single letter
matching nearly everything, so it runs synchronously on `input` with no debounce.

- **Both pools are searched at once and ranked together.** There is no mode to be
  in any more, and the badge on each row is what says which kind it is.
- **An id spelled in full is always offered, with or without a content export**,
  and a bare number offers `qa-<n>` and `dn-<n>`. Only the *label* needs the
  export — without this, removing `#ID` would have taken away the only way to
  search by id on a machine that has a database but no export folder. With an
  export loaded those guesses rank below every row that has a real title behind
  it: a nameless "Dialog 1418" outranking the Article actually called *Waar kan
  ik parkeren?* is exactly backwards.
- **The warm re-offers when the scan lands.** `get_entity_options` is a full
  scan, and on a real database the first keystroke of a cold open beat it — which
  showed as *nothing*, indistinguishable from "this database has no entities". It
  now re-runs the suggestion once the options arrive, if the field still holds
  what was typed while waiting.
- **The field's hover tooltip is suppressed while the list is open**
  (`.conv-search-wrap.is-suggesting`). It hangs below the field, which is exactly
  where the list opens, and it landed on the first row.
- **Every typed token must hit, but the ranking uses the whole typed string.**
  "park hotel" narrows; "park" still puts *parkeren* above *zonnepark*, because
  what someone means by typing is "starts with this" and a token split throws
  that away.
- **An entity is offered by the words that fire it, not only by its name.**
  `caravan` is not the CAMPER entity's name — it is one of the twenty words the
  recognizer fires CAMPER on, and it is what someone types when looking for those
  conversations. The words come from the content export (`entityMap`, keyed by
  upper-cased name, which is also what bridges the two spellings: `entity_index`
  stores the lower-cased *display* name and the export carries the internal one).
  With no export loaded this degrades to name-only.
  - **Every name match ranks above every word match**, so typing `camper` cannot
    bury the CAMPER entity beneath whatever else happens to list it as a word.
  - **The row says which word it matched on** (`camper · via caravan`). Offering
    *camper* for `caravan` with nothing else on it reads as a bug rather than as
    a synonym.
- **A Dialog written with a `-` switches the list to that dialog's nodes**, by
  number or by name, with the whole Dialog still offered first. The node half of
  an id search was previously reachable only by already knowing the number.
- **A Dialog chip opens its own node list when you click it.** A Dialog is a
  question you are half-way through asking — "these conversations touched 6391"
  is usually a step towards "…and reached *this* node of it", and re-typing the
  whole thing was the only way to say the second one. The list is anchored under
  the chip, leads with **whole dialog** so the narrowing is undoable from the same
  menu that did it, and opens on the row the chip already carries so the arrow
  keys move from where you are.
  - It is the *same* popover, opened from a second place (`_convSuggestShow`), so
    the keyboard handling, the dismissal rules and the row markup cannot drift
    between the two. `_convSuggestEditIdx` is what says a pick replaces a chip
    instead of adding one, and `closeConvSuggest` clears it.
  - **Only when the nodes are actually held.** An Article has none, a
    Transactional Dialog carries none in the export (`tDialogs` is `{id, name}`
    and nothing else), and without a content export there is nothing to offer —
    all three stay plain, non-clickable chips, because one that looks clickable
    and does nothing is worse than one that does not.
  - Narrowing two chips onto the same node would leave them ORing with
    themselves, so the list is deduplicated after a replace; first one wins.
- **`_convMarkSuggest` is not `hl()`.** `hl` is built from the *content* search
  bar's `Aa` / `\b` / `.*` toggles, which have nothing to do with this list and
  would make the same keystroke highlight differently depending on a control in
  another view. It also refuses to mark a string that changes length when
  lower-cased — an index into the folded copy is only an index into the original
  while the two are the same length, the same trap `foldDiacritics` documents.
- **`_convEscapeText` and `_convEntityToken` avoid a `"` inside a regex
  literal.** `frontend/tests/extract.js` is a brace matcher that does not model
  regex literals, and one containing a quote runs its scan off the end of the
  function — which would silently take that function out of the tests.

### What the opened chat is told

`_convChatQuery` hands the chat the **union of every positive condition**, not
their intersection: the chat's job is "why is this conversation here", and a turn
that satisfied any one of them answers it, where an AND would open a chat with
nothing marked at all. An excluded condition is no reason a conversation is here,
so it is left out.

- `chatMatchEntities` turns on for the **E** toggle *or* an `entity:` chip.
  Without it, naming an entity opened a forty-turn chat with nothing marked,
  which reads as the chat being broken.
- Id chips lose their `qa-`/`dn-` prefix on the way in, because the chat tests
  `articleIds` and `dialogPaths` directly and a Dialog's evidence there is a bare
  `6391:2/15` — exactly what the raw number in the box produced before any of
  this existed. Those two fields are in scope for the chat exactly when the
  search named an id.
- **The session-list preview highlight is given the text conditions only**
  (`convTextTerms`), never the expression: `hl()` would otherwise mark `AND` and
  `entity:` in every preview.
- **`searchConvsForEntity` is deliberately untouched.** The 💬 Conversations
  button on an entity card still runs an entity-only *text* search, whose
  substring matching over entity names is a genuinely different — usually much
  wider — question from the exact label. It sets a single text chip.
- **`searchConvsForId` appends rather than replaces**, and the same-kind default
  makes a second jump from a Content card OR with the first, which is what
  appending has always meant here.

## The date filter is an instant, not a day

`date_from` / `date_to` are compared as plain strings against
`session_summary.last_ts` / `first_ts`, which is what keeps the predicate an
`idx_timestamp` range scan. They are *not* days: a day picked on the calendar is
a day in the display timezone, and `insZoneDayBounds` resolves it into the two
naive-UTC instants that bound it before it reaches Rust — `2026-07-01` in
Amsterdam is `2026-06-30T22:00:00` → `2026-07-01T21:59:59`.

Shifting the bounds rather than wrapping the column is the whole point: an IANA
zone cannot be expressed in this SQLite at all, and `datetime(s.first_ts, …)`
would kill the index. See `docs/insights.md` → "Reading this in your own
timezone". A change of timezone therefore changes `date_from`/`date_to`, which
*is* a different result set, and the session list is re-run.

## Metadata filtering (Context · Metadata)

Both filter popovers — the Content one (`#contentCtxModalOverlay`) and the Conversations one (`#ctxModalOverlay`) — carry **two tabs**: Context and Metadata. Context says what the user's session was in; metadata (`OutputMetaData` / the `OutputMetadata` column) says what the bot's answer was *marked with* — `entryType`, `nochat`, `transaction`, `attractionIdentifier`, `restaurantName`. On the real export that is **40 distinct keys and 196 chips**.

One button and one popover, not two funnels in an already-crowded toolbar: the two tag sets are indexed the same way and filter with identical semantics, so the only thing that varies is which list is shown and which array a click lands in. `CTX_KINDS` / `CONTENT_CTX_KINDS` hold everything that differs, and `_buildTagChipsHtml` renders both (its `unit` parameter is the only real difference — sessions vs items).

- **The button's badge counts both tabs**, and each tab carries its own count, because a filter left on in the tab you are not looking at still narrows the results. **Clear all** clears both, or the badge would stay lit.
- **Every key offers `any` and `not set`**, the two chips that ask about the key rather than about one of its values, and they are exact complements. `any` exists because a key with more values than anyone will click through cannot otherwise be asked about at all — and because it is the only way to express the opposite of `not set` once a key is capped. `tagFilterMatches` (worker) / `_tagFilterMatches` (renderer) / the `EXISTS` without a value test (Rust) are the three places the distinction lives; the predicate was written out six times in the worker before, which is how `any` would have ended up working on Context but not Metadata.
  - Picking `any` alongside individual values on the same key drops the values: `any` already subsumes them, and ORing them in would only cost parameters.
- **A key with thousands of distinct values must not crowd out the others.** `tag_options` used one global `LIMIT 500` over `ORDER BY name, value`, so `conversation_id` consumed the whole budget and every key sorting after it — `entryLimit`, `testCase`, `transaction`, `wheelchair` — came back with no values and rendered as nothing but "not set". The cap is now **per name** (`TAG_VALUES_PER_NAME`), taken by descending session count so a capped key keeps the values worth filtering on. `one_noisy_key_cannot_crowd_out_the_others` reproduces the original shape.
- **`conversation_id` and `CURRENT_DATETIME` are never indexed** (`META_EXCLUDED_KEYS`). One value per session and one per *message* respectively, so as filters they are thousands of chips each matching a single thing — and they are what made the cap bite. Excluded at index time, so they do not cost a row per message either. **This is about filtering only**: both are still on the message and still shown by the per-message metadata popover, because there they are facts worth reading rather than ways to select.
- **`METADATA_INDEX_RULES_VERSION` is what lets any of this reach an existing database.** The backfill flag was a bare "built?", so the pairs a database happened to be indexed with were the pairs it kept forever: flattening nested values and dropping `conversation_id` both landed *after* some databases were already indexed, and those went on showing raw JSON blobs as chips. Bump the version whenever `metadata_index_rows` starts producing different pairs for the same input — the rebuild clears the table first, because the backfill inserts with `OR IGNORE` and would otherwise leave the old rows beside the new. `meta_flag_version` reads the original `'1'` as version 1, so no migration of the flag itself is needed. Pinned by `older_rules_are_rebuilt_not_left_in_place`.
- **`escalationGroup` is never offered as a metadata chip.** It already has chips on the Context tab, where it belongs — it is a declared context variable with its own `Id`. Listing it twice would let a user set two filters that look independent and describe the same thing. (This is the filtering side only; `docs/collections.md` documents separately why the *tag* is not a reachability condition.)
- **An empty value renders as `(empty)`, not as a blank chip.** "Set with no value" is a real state, distinct from `not set`, and a chip nobody can see is a chip nobody can click.
- **A nested value is split into `key.subkey`, never rendered whole.** `abortTransactionAction` holds `{"label":"Aanvraag stoppen","topicName":"Stoppen_Faciliteitenkaart"}`; as one chip that is an unreadable blob wrapping over four lines, and it is not a filter anyone would click. Flattened it becomes `abortTransactionAction.topicName = Stoppen_Faciliteitenkaart`, which is a question someone actually asks. **It also collapses three spellings of one value into one chip** — the compact form, the form CM stored with its newlines replaced by `__` (its line-break marker, see `docs/chat-rendering.md`), and the same object with its keys in the other order. On the real export that turns `abortTransactionAction`'s blobs into two clean keys, and `Stoppen` correctly aggregates across four items instead of appearing four times.
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

## The card and the search agree about entities

An Article card resolves each question phrase to an entity by name, then by
exact word, then by the longest token — `getEntityForChip` — and draws a chip
saying so. The *search* behind it required the phrase to be that entity's name
**verbatim**. So a card could read "Entity: PARKEREN" while a search for one of
PARKEREN's other words did not return the Article the chip was on, and nothing
looked wrong in either half.

The resolution stays on the main thread, where `getEntityForChip` lives, and the
answer is shipped to the worker as a phrase → entity-name map. One resolution,
two consumers — which is the same arrangement `entityArticleNames` /
`entityDialogNames` already used for the xref pills.

- **Flat pairs, not an object.** Phrases are arbitrary user text and can collide
  with `Object.prototype` keys.
- **The enrichment reads trigger words only** (`_triggerWords`), never
  `expression`. An entity whose *regex source* happens to contain the search
  term has not been "found in" an Article — that would be a match on a pattern
  nobody wrote as content. The Entities tab still searches the expression,
  which is its job.
- **An entity is searchable by everything it carries**: its name, its type, its
  description, and all three word texts (`text`, `wordInBetween`, `expression`).
  `wordInBetween` and `expression` are what an entity matches on at runtime, so
  one findable in CM.com by them was not findable here.
- **The description had been parsed and discarded** by `extract_entities` since
  it was written — `cols[2]`, read into a variable and dropped. It is the one
  field saying what an entity is *for*, and it was the one field you could not
  search. It is now emitted, indexed and shown on the card.
- `frontend/tests/entity-search.test.js` drives the real worker and asserts
  both directions: the Article *is* found through the resolved entity, and is
  *not* found without the map — so the test is about the fix rather than about
  the Article's own text.

## Entity → Article / Dialog cross-references

`getEntityForChip(phrase)` is the single source of truth for "which entity is this phrase?", and it resolves by entity name, then by an entity *word*, then by the longest token in the phrase. `entityRefIndex()` is the **exact inverse** of it, built in one pass over the export and cached until `loadData`.

- The two directions used to disagree, and that is what "the entity view doesn't show all its Articles" was: a chip resolved a phrase by word or token and labelled itself `Entity: WIJN`, while `entityArticleXrefs` listed only Articles whose phrase was *verbatim* the entity name. On the real export that is **2013 entities with Article links instead of 1167**. `every chip's entity lists the Article it labels` in `frontend/tests/conv-search.test.js` pins the inverse property.
- **The worker's "Used in Articles" / "Used in Dialogs" pills are fed from the same index.** `buildEntityXrefSets` used to recompute the relationship itself with a name-equality check, so the pills filtered on a different rule than the cards they were filtering. It now just receives two name lists over the init message; the resolution stays on the main thread, where `getEntityForChip` lives.
- One entity per phrase (the first that resolves), matching what the chip displays, and an item is listed once however many of its phrases resolve to it.
- `_chipEntityCache` memoizes phrase → entity because the same phrases recur constantly and the index build asks for every phrase in the export; `null` is a real answer and is cached too. Building the index costs ~48 ms on the real export (3295 Articles, 808 Dialogs, 2233 entities), once per data load.
