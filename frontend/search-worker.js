// search-worker.js — Runs all filtering and sorting off the main thread.
// Receives "init" (full dataset) and "search" (query + options) messages.
// Returns "results" with filtered arrays + matchingEntityNames.

"use strict"

// ── Dataset (populated on "init") ────────────────────────────────────────────
let workerArticles = [] // items with _kind === "article"
let workerDialogs = [] // allDialogsCombined (dialogs + tDialogs w/ _kind)
let workerEntities = [] // allEntities
let allItems = [] // pre-built articles + dialogs combined

// ── Pre-computed search indexes (built on "init") ────────────────────────────
// Maps item → pre-stripped searchable text so strip() isn't called per search.
// Article: _searchId (string), _searchQuestionsUpper, _answerItems
// Dialog: _searchId (string), _searchName, _searchDesc, _searchNodes [{name, isRef, refId, _answerItems}]
// Entity: _searchName, _searchWords [lowercased texts]

// Pre-computed entity cross-reference sets (built on init)
let entityHasArticleXref = new Set() // entity names (upper) that have article xrefs
let entityHasDialogXref = new Set() // entity names (upper) that have dialog xrefs
let entityByNameUpper = new Map() // entity name (upper) → entity object, for per-term lookups
// Content phrase (upper) → entity name (upper), resolved on the main thread by
// `getEntityForChip` so the search and the card agree. See `entityNamesForPhrases`.
let phraseEntityUpper = new Map()

// Variable name maps (id → name), populated on "init" from dialogs export
let convVarMap = new Map() // ConversationVariable id → name
let ctxVarMap = new Map() // ContextVariable id → name

// ── Search options (updated on each "search" message) ────────────────────────
let searchCase = false
let searchWord = false
let searchRegex = false
let searchContent = true
let searchExcludeNonDefault = false
// A Dialog node can show an Article instead of a Response of its own
// (`output.kbaIdReference`). That Article is not the Dialog's content, so by
// default neither the node's name — a label for the reference — nor the
// Article's text makes the Dialog match. The Ref toggle turns both back on.
let searchIncludeRefs = false
// Article id → Article, for a reference node's text under the Ref toggle.
let articleById = new Map()
let contentContextFilters = [] // [{name, value}] — active content context filters
// The OutputMetaData tags an Answer carries (escalationGroup, entryType,
// nochat, transaction, attractionIdentifier…). Filtered exactly like context,
// against `_metaSets` instead of `_ctxSets`.
let contentMetadataFilters = []

// ── Utilities ────────────────────────────────────────────────────────────────
/**
 * The context conditions one output fires under, as `{name: [values]}`.
 *
 * Every output type carries them — Answer, DialogStart, TDialogStart,
 * HaloAgentStart — so this takes the variables rather than the output, and
 * callers must not pre-filter on type. `any` is not a condition (it matches
 * everyone) and is dropped, so an output set to `any` throughout is `{}`.
 * escalationGroup is left out: its filter reads the tag and the condition
 * together through the aggregate set (`outputEscGroups`). Mirrored by
 * `_outputCtxSet` in index.html.
 */
/// Adds values to a context set, keeping what is there. A Dialog stores a
/// condition on several values as one entry *per value* with the same id —
/// `{id: 2, value: "Tablet"}, {id: 2, value: "Mobile"}, …` — so assigning
/// instead kept only the last: on the 2026-08-15 export 66 of the 141 context
/// values Dialogs carry found few or none of them (DeviceType=Mobile: 0 of 48).
function ctxAdd(set, name, vals) {
  if (!vals.length) return
  const cur = set[name] || (set[name] = [])
  for (const v of vals) if (!cur.includes(v)) cur.push(v)
}

function outputCtxSet(cvs, isArticle) {
  const set = {}
  for (const cv of cvs || []) {
    const name = ctxVarMap.get(isArticle ? cv.Id : cv.id)
    if (!name || name === "escalationGroup") continue
    const raw = isArticle ? cv.Values || [] : cv.value ? [cv.value] : []
    const vals = []
    for (const valStr of raw) {
      for (const v of String(valStr).split(",")) {
        const t = v.trim()
        if (t && t !== "any") vals.push(t)
      }
    }
    ctxAdd(set, name, vals)
  }
  return set
}

/**
 * The escalation groups one output is tied to, by either route: the
 * `escalationGroup` metadata tag it carries (which group it belongs to), and
 * the values of an `escalationGroup` context-variable condition (the groups it
 * fires for). The Context tab's escalationGroup chips match both, deliberately:
 * `theater` exists in the export only as a condition, never as a tag, so a
 * tag-only filter could not find it at all. `any` is not a group. Mirrored by
 * `_outputEscGroups` in index.html.
 */
function outputEscGroups(meta, cvs, isArticle) {
  const out = []
  const push = (g) => {
    const t = String(g == null ? "" : g).trim()
    if (t && t !== "any" && !out.includes(t)) out.push(t)
  }
  if (meta && meta.escalationGroup != null) push(meta.escalationGroup)
  for (const cv of cvs || []) {
    if (ctxVarMap.get(isArticle ? cv.Id : cv.id) !== "escalationGroup") continue
    const raw = isArticle ? cv.Values || [] : cv.value ? [cv.value] : []
    for (const valStr of raw) for (const v of String(valStr).split(",")) push(v)
  }
  return out
}

// Parse a query into OR groups of AND terms.
// "hello world | goodbye" → [["hello","world"],["goodbye"]]
// When in regex mode, return a single group with the raw query as one term.
// Tokenize a query segment into terms, respecting "quoted phrases" as single tokens.
function tokenizeSegment(str) {
  const tokens = []
  const re = /"([^"]*)"|([^\s"]+)/g
  let m
  while ((m = re.exec(str)) !== null) {
    const token = m[1] !== undefined ? m[1] : m[2]
    if (token) tokens.push(token)
  }
  return tokens
}

// ── Accents ──────────────────────────────────────────────────────────────────
//
// A case-insensitive search ignores accents, as the Conversations search does:
// `oke` finds `oké` (376 of them in the Dialogs export, against 27 spelled
// without), `krumel` finds `krümel`. A plain search compares folded strings; a
// whole-word search, which is a regex, gets a character class per letter
// instead. Case-sensitive and `.*` searches are exact, as they ask to be.

/// Lower-cased, accents removed. Not length-preserving — nothing here maps an
/// index back into the original.
function fold(s) {
  const l = String(s).toLowerCase()
  return /[^\x00-\x7f]/.test(l) ? l.normalize("NFD").replace(/[\u0300-\u036f]/g, "") : l
}

/// Base letter → a character class of it and its accented forms.
const ACCENT_CLASS = (() => {
  const groups = {}
  for (let c = 0xc0; c <= 0x24f; c++) {
    const ch = String.fromCharCode(c).toLowerCase()
    const base = ch.normalize("NFD")[0]
    if (base < "a" || base > "z" || ch === base) continue
    if (!groups[base]) groups[base] = []
    if (!groups[base].includes(ch)) groups[base].push(ch)
  }
  const out = {}
  for (const b in groups) out[b] = "[" + b + groups[b].join("") + "]"
  return out
})()

/// An escaped (non-regex) pattern with every letter widened to its accented
/// forms. Escaping only ever precedes punctuation, so no letter here is part
/// of an escape sequence.
function accentPattern(escaped) {
  let out = ""
  for (const ch of escaped) out += ACCENT_CLASS[fold(ch)] || ch
  return out
}

/// `%{DialogOptions()}`, `%{Link(1)}`, `%{Image(2)}` — markup, not words. On
/// the real export "option" matched 489 Dialogs, 487 of them only through
/// `%{DialogOptions()}`. Variable references are expanded to their names
/// before this runs (`expandVarNames`) and carry no parentheses, so they stay.
function dropPlaceholders(t) {
  return (t || "").replace(/%\{[A-Za-z]+\([^}]*\)\}/g, " ")
}

// ── Phrases ──────────────────────────────────────────────────────────────────
//
// A term of several words matches however the words are joined. The export
// spells one event "oud en nieuw" (406×), "Oud & Nieuw" (199×), "oud-en-nieuw"
// (113×, mostly URLs) and "oud nieuw" — and an exact phrase found 0 Dialogs,
// the one *named* "Oud & Nieuw" included. Between two words any run of spaces,
// hyphens, underscores or slashes is one join, and a connective — en, and,
// und, & or + — may stand in it or not.

const PHRASE_SEP = "[\\s\\-\u2013\u2014_/]*"
const PHRASE_CONNECTIVES = ["en", "and", "und", "&", "+"]
const PHRASE_JOIN = PHRASE_SEP + "(?:(?:en|and|und|&amp;|&|\\+)" + PHRASE_SEP + ")?"

/// Is this term more than one word? Only then is it matched as a phrase.
function isPhraseTerm(term) {
  return /[\s\-\u2013\u2014_/&+]/.test(term.trim())
}

/// The regex source for a term: exact for `.*`; otherwise escaped, widened
/// for accents when case-insensitive, and joined tolerantly when a phrase.
function termPattern(term) {
  if (searchRegex) return term
  const esc = (w) => {
    const e = w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
    return searchCase ? e : accentPattern(e)
  }
  if (!isPhraseTerm(term)) return esc(term)
  const words = term
    .replace(/[&+]/g, " & ")
    .split(/[\s\-\u2013\u2014_/]+/)
    .filter((w) => w && !PHRASE_CONNECTIVES.includes(w.toLowerCase()))
  return words.length ? words.map(esc).join(PHRASE_JOIN) : esc(term)
}

function parseOrGroups(q) {
  if (!q) return []
  if (searchRegex) return [[q]]
  return q
    .split("|")
    .map((g) => tokenizeSegment(g.trim()))
    .filter((g) => g.length > 0)
}

// Build a regex for a single escaped term (respects searchCase and searchWord).
// ── Short words start a word ────────────────────────────────────────────────
//
// A word of up to SHORT_TERM_MAX letters only matches at the start of a word;
// a longer one anywhere. Short words are where "anywhere" is mostly noise —
// "oud" matched 657 items against 210 at a word start, most of the rest
// h-oud-en, onderh-oud, abonnementh-oud-er; "eten" 1370 against 360, through
// weten, genieten, vergeten. Long words are where it finds real compounds, and
// Dutch is full of them: "korting" in verjaardagskorting, "tickets" in
// entreetickets. A phrase is judged by its first word. `\b` still makes a
// term exact at both ends, and `.*` is left alone.
const SHORT_TERM_MAX = 5
const WORD_EDGE_BEFORE = "(?<![\\w\\u00C0-\\u024F])"
const WORD_EDGE_AFTER = "(?![\\w\\u00C0-\\u024F])"

function isShortTerm(term) {
  if (searchRegex) return false
  const first = term.trim().split(/[\s\-\u2013\u2014_/&+]+/)[0] || ""
  return first.length > 0 && first.length <= SHORT_TERM_MAX
}

function buildTermRegex(term) {
  try {
    let pat = termPattern(term)
    if (searchWord) pat = WORD_EDGE_BEFORE + pat + WORD_EDGE_AFTER
    else if (isShortTerm(term)) pat = WORD_EDGE_BEFORE + pat
    return new RegExp(pat, searchCase ? "" : "i")
  } catch (e) {
    return null
  }
}

function compiledGroupsHaveInvalidRegex(groups) {
  return groups.some((andGroup) =>
    andGroup.some((compiled) => compiled.re === null && compiled.needle === null),
  )
}

// Pre-compiled OR groups: array of AND-groups, each being array of {re, needle} objects.
// Built once per search message and shared across all match calls.
let _orRegexGroups = [] // [{re, needle}[]][]

// Cache key for matchingEntityNames so it isn't rebuilt on filter/sort-only changes.
let _entityCacheKey = ""

function buildOrRegexGroups(orGroups) {
  return orGroups.map((andTerms) =>
    andTerms.map((term) => ({
      // A phrase, or a short word, is a regex: joins and word starts cannot
      // be an indexOf.
      re:
        !canUsePlainMatch() || (!searchRegex && (isPhraseTerm(term) || isShortTerm(term)))
          ? buildTermRegex(term)
          : null,
      needle: canUsePlainMatch() && !isPhraseTerm(term) && !isShortTerm(term)
        ? searchCase
          ? term
          : fold(term)
        : null,
    })),
  )
}

// Test a single string against one compiled term {re, needle}.
// strLower is an optional pre-lowercased version of str for case-insensitive plain matches.
function testTerm(compiled, str, strLower) {
  if (!str) return false
  if (compiled.re) {
    compiled.re.lastIndex = 0
    return compiled.re.test(str)
  }
  if (compiled.needle !== null) {
    if (searchCase) return str.indexOf(compiled.needle) !== -1
    return (strLower !== undefined ? strLower : fold(str)).indexOf(compiled.needle) !== -1
  }
  return false
}

// Test a single term against multiple field strings (any match = term is found).
// fieldsLower is an optional parallel array of pre-lowercased strings.
function termFoundInFields(compiled, fields, fieldsLower) {
  return fields.some((f, i) => f != null && testTerm(compiled, f, fieldsLower && fieldsLower[i]))
}

// Entity enrichment: does this specific compiled term match any word of any entity
// in the given list of entity name uppers?
function termMatchesEntityByNames(compiled, entityNameUppers) {
  for (const nameUpper of entityNameUppers) {
    const entity = entityByNameUpper.get(nameUpper)
    if (entity && termFoundInFields(compiled, entity._triggerWords)) return true
  }
  return false
}

/// Which entity a content phrase resolves to, the way the chip on the card does.
///
/// The main thread owns that resolution (`getEntityForChip`: exact name, then
/// exact word, then the longest token) and ships the answer as a map, because
/// the card and the search disagreeing about the same relationship is exactly
/// the bug this closes — an Article whose chip reads "Entity: PARKEREN" was
/// not found by a search for one of that entity's other words unless the
/// question phrase happened to *be* the entity's name.
function entityNamesForPhrases(phrasesUpper) {
  const out = []
  for (const t of phrasesUpper) {
    const name = phraseEntityUpper.get(t)
    if (name && matchingEntityNames.has(name) && !out.includes(name)) out.push(name)
  }
  return out
}

// Check if ALL terms in an AND-group are each found somewhere in the given fields.
function andGroupMatchesFields(andGroup, fields, fieldsLower) {
  return andGroup.every((compiled) => termFoundInFields(compiled, fields, fieldsLower))
}

// Check if ANY OR-group's AND terms all match the given fields.
function orGroupsMatchFields(groups, fields, fieldsLower) {
  return groups.some((andGroup) => andGroupMatchesFields(andGroup, fields, fieldsLower))
}

// Expand %{ConversationVariable(N)} and %{ContextVariable(N)} to their names
// so users can search by variable name instead of numeric ID.
function expandVarNames(text) {
  if (!text) return ""
  return text
    .replace(/%\{ConversationVariable\((\d+)\)\}/g, (_, id) => {
      const name = convVarMap.get(Number(id))
      return name ? "%{" + name + "}" : ""
    })
    .replace(/%\{ContextVariable\((\d+)\)\}/g, (_, id) => {
      const name = ctxVarMap.get(Number(id))
      return name ? "%{" + name + "}" : ""
    })
}

function strip(t) {
  return (t || "")
    .replace(/%\{[^}]*\}/g, " ")
    .replace(/\{[^}]*\}/g, (m) => {
      // Preserve URLs and button label text from CM.com CTA blocks so they remain searchable
      const parts = []
      const urls = m.match(/https?:\/\/[^\s}"]+/g)
      if (urls) parts.push(...urls)
      const btnText = m.match(/buttonText="([^"]*)"/)
      if (btnText && btnText[1]) parts.push(btnText[1])
      return parts.length ? " " + parts.join(" ") + " " : " "
    })
    .replace(/\s+/g, " ")
    .trim()
}

function sortBy(arr, sort, idFn, nameFn) {
  const s = arr.slice()
  if (sort === "id-asc") s.sort((a, b) => idFn(a) - idFn(b))
  else if (sort === "id-desc") s.sort((a, b) => idFn(b) - idFn(a))
  else if (sort === "name-asc")
    s.sort((a, b) => nameFn(a).localeCompare(nameFn(b)))
  else if (sort === "name-desc")
    s.sort((a, b) => nameFn(b).localeCompare(nameFn(a)))
  return s
}

/**
 * Restore a JSON document CM stored with its newlines replaced by `_`.
 *
 * `_` is CM's line-break marker (see the `parseCmOutput` rules), and a value
 * authored across several lines comes back as
 * `{__  "label": "…",__  "topicName": "…"__}` — the same object as the
 * single-line spelling, but not parseable and therefore a *second* chip for
 * the same thing.
 *
 * Only `_` outside a double-quoted span is touched, which is safe by
 * construction: `_` is not valid JSON syntax there, so it can only be the
 * marker. Inside a string it is left alone — `Stoppen_TD_Algemeen` is a real
 * value and must survive intact.
 */
function _unmarkJsonBreaks(text) {
  let out = ""
  let inStr = false
  let esc = false
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]
    if (inStr) {
      out += ch
      if (esc) esc = false
      else if (ch === "\\") esc = true
      else if (ch === '"') inStr = false
      continue
    }
    if (ch === '"') {
      inStr = true
      out += ch
      continue
    }
    if (ch === "_") {
      // Collapse a run to one space, matching the whitespace it replaced.
      while (text[i + 1] === "_") i++
      out += " "
      continue
    }
    out += ch
  }
  return out
}

/// How deep a nested metadata value is flattened, and how many leaves one
/// value may contribute. Bounds exist so a large embedded document can't turn
/// one answer into hundreds of filter chips.
const META_MAX_DEPTH = 3
const META_MAX_LEAVES = 24

/**
 * Expand one metadata entry into the `name → value` pairs it should filter by.
 *
 * A value that is a JSON object becomes one pair per leaf, named
 * `key.subkey` — `abortTransactionAction` holds
 * `{"label":"Aanvraag stoppen","topicName":"Stoppen_Faciliteitenkaart"}`, and
 * as a single chip that is an unreadable blob nobody would click. Split, it
 * becomes `abortTransactionAction.topicName = Stoppen_Faciliteitenkaart`,
 * which is a filter someone actually wants. It also collapses three spellings
 * of one value into one chip: the compact form, the `__`-broken form, and the
 * same object with its keys in the other order.
 *
 * Anything else — including a JSON *array*, which has no stable member names —
 * stays a single pair with its value stringified.
 */
function flattenMetaEntry(key, value, out, depth) {
  const name = String(key).trim()
  if (!name || out.length >= META_MAX_LEAVES) return out
  let v = value
  // A nested object may arrive already parsed (the content export) or as text
  // (everywhere else). Both take the same path from here.
  if (typeof v === "string") {
    const t = v.trim()
    if (t.startsWith("{") && t.endsWith("}")) {
      let parsed = null
      try {
        parsed = JSON.parse(t)
      } catch (_) {
        try {
          parsed = JSON.parse(_unmarkJsonBreaks(t))
        } catch (_) {}
      }
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed))
        v = parsed
    }
  }
  if (
    v &&
    typeof v === "object" &&
    !Array.isArray(v) &&
    depth < META_MAX_DEPTH
  ) {
    const keys = Object.keys(v)
    // An object with no keys carries nothing to filter by; keep the key itself
    // so "not set" still distinguishes it from absent.
    if (!keys.length) {
      out.push([name, ""])
      return out
    }
    for (const sub of keys) flattenMetaEntry(name + "." + sub, v[sub], out, depth + 1)
    return out
  }
  out.push([
    name,
    v === null || v === undefined
      ? ""
      : typeof v === "object"
        ? JSON.stringify(v)
        : String(v),
  ])
  return out
}

/**
 * One Answer output's metadata, as `{name: [value]}`.
 *
 * The content export spells this as a plain object (`OutputMetaData` on an
 * Article output, `metadata` on a dialog output item) while the interaction
 * log spells it as an array of `{key, value}` pairs — same information, two
 * encodings, so only this side needs to know about the object form.
 *
 * Values are stringified because the export mixes `"true"` with `true`, and a
 * chip the user clicks is a string either way. `null`/`undefined` become `""`,
 * which is the same "set but empty" state the conversations side records.
 *
 * **`buildContentMetadataOptions` in index.html must expand values the same
 * way**, or a chip would filter to a different set than its own count claims.
 * `frontend/tests/metadata-filter.test.js` asserts the two against each other
 * over the real export.
 */
function metaSetOf(meta) {
  const out = {}
  if (!meta || typeof meta !== "object") return out
  for (const key of Object.keys(meta)) {
    for (const [name, value] of flattenMetaEntry(key, meta[key], [], 0)) {
      // Repeated names can only come from an object with duplicate-ish leaves;
      // keep every distinct value so a filter on any of them matches.
      if (!out[name]) out[name] = []
      if (!out[name].includes(value)) out[name].push(value)
    }
  }
  return out
}

// ── Article helpers ───────────────────────────────────────────────────────────
function aKind(a) {
  return a._aKind // pre-computed on init
}

function aFaqQ(a) {
  return a._faqQ // pre-computed on init
}

// ── Pre-computation on init ───────────────────────────────────────────────────

function precomputeArticle(a) {
  // Cache kind
  const types = a.Outputs.map((o) => o.Type)
  if (types.includes("Answer")) a._aKind = "answer"
  else if (types.includes("TDialogStart")) a._aKind = "tdialog"
  else a._aKind = "dialog"

  // Cache FAQ question
  const f = a.Questions.find((q) => q.IsFaq)
  a._faqQ = f ? f.Text : a.Questions[0] ? a.Questions[0].Text : null

  // Cache response text
  const o =
    a.Outputs.find((o) => o.Type === "Answer" && o.IsDefault) ||
    a.Outputs.find((o) => o.Type === "Answer")
  a._response = o ? o.Text : null

  a._searchId = String(a.Id)
  a._searchQuestionsUpper = a.Questions.map((qs) => qs.Text.toUpperCase())
  // The part outside the Responses: the id and the questions, searched
  // together (off under ¬T). `ph` is what entity enrichment resolves.
  const _ncF = [a._searchId, ...a.Questions.map((qs) => qs.Text)]
  a._ncParts = [{ f: _ncF, l: _ncF.map(fold), ph: a._searchQuestionsUpper }]
  // One question at a time, only to count which ones matched (`explainMatch`);
  // the search itself still reads the questions as one part.
  a._qParts = a.Questions.map((qs, i) => ({
    f: [qs.Text],
    l: [fold(qs.Text)],
    ph: [a._searchQuestionsUpper[i]],
  }))

  // One context set per output of every type, in output order. A route into a
  // Dialog is conditioned on context exactly like an Answer is, and on the real
  // export most conditions sit on routes: available_livechat_theater is set on
  // 11 DialogStart outputs and not one Answer. An unconditioned output is an
  // empty set, which is what lets "not set" pass on it.
  a._ctxSets = a.Outputs.map((o) => outputCtxSet(o.ContextVariables, true))

  // One metadata map per Answer output, in output order. Values are arrays so
  // the filter reads identically to the context one, even though a metadata
  // key only ever holds a single value.
  a._metaSets = []
  for (const o of a.Outputs) {
    if (o.Type !== "Answer") continue
    a._metaSets.push(metaSetOf(o.OutputMetaData))
  }

  // Add aggregate escalation group ctxSet so the escalationGroup filter works.
  // Every group any output is tied to — by tag or by condition, Answer or
  // route — is merged into a single ctxSet so multi-select (AND logic) can
  // match articles that cover multiple groups.
  const _escGroups = []
  for (const o of a.Outputs) {
    for (const g of outputEscGroups(o.OutputMetaData, o.ContextVariables, true))
      if (!_escGroups.includes(g)) _escGroups.push(g)
  }
  if (_escGroups.length) a._ctxSets.push({ escalationGroup: _escGroups })

  // Build aligned per-answer data: {s, r, e, ctxSet} for every Answer output.
  // This links text and context conditions for the SAME answer so combined
  // text+context filtering can require both to be satisfied by one answer.
  // Per Answer output, its own context and metadata — what a ctx:/meta: chip
  // or the panel is counted against (`explainMatch`). The aggregate
  // escalationGroup set sits apart, as `matchesContentContext` reads it.
  a._answerTags = []
  for (const _ao of a.Outputs) {
    if (_ao.Type !== "Answer") continue
    const esc = outputEscGroups(_ao.OutputMetaData, _ao.ContextVariables, true)
    const ctxSets = [outputCtxSet(_ao.ContextVariables, true)]
    if (esc.length) ctxSets.push({ escalationGroup: esc })
    a._answerTags.push({ ctxSets, metaSets: [metaSetOf(_ao.OutputMetaData)] })
  }

  a._answerItems = []
  for (const _ao of a.Outputs) {
    if (_ao.Type !== "Answer") continue
    const _alm = new Map()
    ;(_ao.Links || []).forEach((l) => {
      if (l.TagId && l.Label) _alm.set(l.TagId, l.Label)
    })
    const _arT = _ao.Text || ""
    const _aExp = _arT.replace(
      /%\{Link\((\d+)\)\}/g,
      (_, n) => _alm.get(Number(n)) || "Link " + n,
    )
    const _aCvs = _ao.ContextVariables || []
    const _aCtx = {}
    if (_aCvs.some((cv) => cv.Values && !cv.Values.includes("any"))) {
      for (const cv of _aCvs) {
        const name = ctxVarMap.get(cv.Id)
        if (!name) continue
        const vals = []
        for (const valStr of cv.Values) {
          for (const v of valStr.split(",")) {
            const t = v.trim()
            if (t && t !== "any") vals.push(t)
          }
        }
        ctxAdd(_aCtx, name, vals)
      }
    }
    // Tag and condition together, as the item-level aggregate reads them.
    const _aEscGroups = outputEscGroups(_ao.OutputMetaData, _aCvs, true)
    if (_aEscGroups.length) _aCtx.escalationGroup = _aEscGroups
    else delete _aCtx.escalationGroup
    const _as = strip(_aExp)
    const _ar = dropPlaceholders(_arT)
    const _ae = dropPlaceholders(expandVarNames(_aExp))
    a._answerItems.push({
      s: _as,
      r: _ar,
      e: _ae,
      sl: fold(_as),
      rl: fold(_ar),
      el: fold(_ae),
      f: [_as, _ar, _ae],
      l: [fold(_as), fold(_ar), fold(_ae)],
      ctxSet: _aCtx,
      isNonDefault: _ao !== o && !_ao.IsDefault,
    })
  }
}

function precomputeDialog(item) {
  item._searchId = String(item.id)
  item._searchName = item.name || ""
  item._searchDesc = item.description || ""

  // Pre-compute per-node search data
  const nodes = item.nodes || []
  const byId = new Map(nodes.map((n) => [n.id, n]))
  const isRefNode = (n) =>
    !!n &&
    !!n.output &&
    n.output.kbaIdReference != null &&
    !(n.output.items || []).some((i) => i.type === "Answer")
  // Where a node leads once GoTo jumps are followed. A GoTo named after a
  // reference node ("GoTo - D-3A3BLC") is part of the reference: 31 Dialogs
  // matched their referenced Article's title through one. A *phrasing* that
  // routes to a reference is not — see `_phrases` below.
  const leadsToRef = (id) => {
    const seen = new Set()
    let n = byId.get(id)
    while (n && n.type === "GoTo" && !seen.has(n.id)) {
      seen.add(n.id)
      const l = (n.links || [])[0]
      n = l ? byId.get(l.childNodeId) : null
    }
    return isRefNode(n)
  }
  item._searchNodes = nodes.map((n) => {
    const nodeAnsItems = ((n.output && n.output.items) || []).filter(
      (i) => i.type === "Answer",
    )
    const ans = nodeAnsItems.find((i) => i.isDefault) || nodeAnsItems[0] || null
    // Build aligned per-answer items for this node: {s, r, e, ctxSet}
    // so combined text+context matching can require both on the same answer.
    const _nodeAnsItems = []
    for (const _nai of nodeAnsItems) {
      const _nlm = new Map()
      ;((_nai.data && _nai.data.hyperlinks) || []).forEach((h) => {
        if (h.id !== undefined && h.label) _nlm.set(h.id, h.label)
      })
      const _nrT = (_nai.data && _nai.data.text) || ""
      const _nExp = _nrT.replace(
        /%\{Link\((\d+)\)\}/g,
        (_, n) => _nlm.get(Number(n)) || "Link " + n,
      )
      const _nCvs = _nai.contextVariables || []
      const _nCtx = {}
      for (const cv of _nCvs) {
        const name = ctxVarMap.get(cv.id)
        if (!name || !cv.value) continue
        const vals = cv.value
          .split(",")
          .map((v) => v.trim())
          .filter(Boolean)
        ctxAdd(_nCtx, name, vals)
      }
      const _nEscGroups = outputEscGroups(_nai.metadata, _nCvs, false)
      if (_nEscGroups.length) _nCtx.escalationGroup = _nEscGroups
      else delete _nCtx.escalationGroup
      const _ns = strip(_nExp)
      const _nr = dropPlaceholders(_nrT)
      const _ne = dropPlaceholders(expandVarNames(_nExp))
      _nodeAnsItems.push({
        s: _ns,
        r: _nr,
        e: _ne,
        sl: fold(_ns),
        rl: fold(_nr),
        el: fold(_ne),
        f: [_ns, _nr, _ne],
        l: [fold(_ns), fold(_nr), fold(_ne)],
        ctxSet: _nCtx,
        isNonDefault: _nai !== ans && !_nai.isDefault,
      })
    }
    const refId = n.output ? n.output.kbaIdReference : null
    // Every output of the node — Answers and routes — for context, as
    // `_ctxSets` reads them; Answers only for metadata, as `_metaSets` does.
    const _nItems = (n.output && n.output.items) || []
    const _nEsc = []
    for (const oi of _nItems)
      for (const g of outputEscGroups(oi.metadata, oi.contextVariables, false))
        if (!_nEsc.includes(g)) _nEsc.push(g)
    const ctxSets = _nItems.map((oi) => outputCtxSet(oi.contextVariables, false))
    if (_nEsc.length) ctxSets.push({ escalationGroup: _nEsc })
    const metaSets = _nItems.filter((oi) => oi.type === "Answer").map((oi) => metaSetOf(oi.metadata))
    return {
      ctxSets,
      metaSets,
      name: n.name || "",
      // A node that shows an Article rather than a Response of its own, or a
      // GoTo that jumps to one.
      isRef: (refId != null && !nodeAnsItems.length) || (n.type === "GoTo" && leadsToRef(n.id)),
      refId: refId != null ? refId : null,
      _answerItems: _nodeAnsItems,
    }
  })

  // The fields outside the Responses, searched together (off under ¬T): the
  // id, name, description, node names — and what the user says to move
  // between nodes. Those phrasings are the Dialog's own recognition, as an
  // Article's questions are its own; they used to be read only to look up
  // entities, so 1500 of 2532 distinct phrasings on the real export did not
  // find the Dialog they belong to. A reference node's name is kept apart:
  // it counts only under the Ref toggle.
  const _phrases = []
  const _refNames = []
  for (const n of nodes) {
    for (const link of n.links || []) {
      const c = link.condition || {}
      if (c.type !== "Recognition" || !c.data || c.data.isFallback) continue
      // Counted even when the link lands on a reference node. The phrasing
      // is written in this Dialog, and a user who types it here takes this
      // route — "which Dialogs handle zwembad?" has this one as an answer
      // (9 → 16 Dialogs when it was left out). The card says the route ends
      // in an Article, which is what keeps it from reading as a match on
      // the Article's content.
      for (const qo of c.data.questions || [])
        if (qo.text && !_phrases.includes(qo.text)) _phrases.push(qo.text)
    }
  }
  // Parts, not one bucket: every word of a condition has to be found in the
  // same one — the Dialog's name and description, one node's name, one
  // phrasing — or "hotel annuleren" matched "hotel" in a phrasing and
  // "annuleren" in the description, which is not something the Dialog says.
  const part = (fields, ph) => ({ f: fields, l: fields.map(fold), ph })
  item._ncParts = [part([item._searchId, item._searchName, item._searchDesc])]
  item._refParts = []
  for (const sn of item._searchNodes) {
    sn.namePart = part([sn.name])
    ;(sn.isRef ? item._refParts : item._ncParts).push(sn.namePart)
  }
  const phrasePart = new Map()
  for (const t of _phrases) {
    const p = part([t], [t.toUpperCase()])
    phrasePart.set(t, p)
    item._ncParts.push(p)
  }
  for (const t of _refNames) item._refParts.push(part([t]))
  // The same phrasing parts, by the node whose links carry them — the node
  // `nodeMatchReasons` credits a phrasing to — so a match can be counted per
  // node (`explainMatch`). Shared objects, deduplicated per node.
  nodes.forEach((n, i) => {
    const own = []
    for (const link of n.links || []) {
      const c = link.condition || {}
      if (c.type !== "Recognition" || !c.data || c.data.isFallback) continue
      for (const qo of c.data.questions || []) {
        const p = qo.text && phrasePart.get(qo.text)
        if (p && !own.includes(p)) own.push(p)
      }
    }
    item._searchNodes[i].phraseParts = own
  })

  // Pre-compute entity question texts for entity-word enrichment
  item._entityQuestionTexts = []
  for (const n of nodes) {
    for (const link of n.links || []) {
      const condData = (link.condition && link.condition.data) || {}
      if (!condData.isFallback) {
        for (const qo of condData.questions || []) {
          if (qo.text) item._entityQuestionTexts.push(qo.text.toUpperCase())
        }
      }
    }
  }

  // Pre-compute whether any node has an Answer output (for recognition filter)
  item._hasAnswerOutput = nodes.some((n) =>
    ((n.output && n.output.items) || []).some((i) => i.type === "Answer"),
  )

  // One context set per output item across every node, of every type — see
  // precomputeArticle.
  item._ctxSets = []
  for (const n of nodes) {
    for (const oi of (n.output && n.output.items) || [])
      item._ctxSets.push(outputCtxSet(oi.contextVariables, false))
  }

  // One metadata map per Answer item across every node, matching the Article
  // side. A dialog's answers live one level deeper, but nothing else differs.
  item._metaSets = []
  for (const n of nodes) {
    for (const oi of (n.output && n.output.items) || []) {
      if (oi.type !== "Answer") continue
      item._metaSets.push(metaSetOf(oi.metadata))
    }
  }

  // Add aggregate escalation group ctxSet so the escalationGroup filter works —
  // tag or condition, Answer or route, as for an Article.
  const _dEscGroups = []
  for (const n of nodes) {
    for (const oi of (n.output && n.output.items) || []) {
      for (const g of outputEscGroups(oi.metadata, oi.contextVariables, false))
        if (!_dEscGroups.includes(g)) _dEscGroups.push(g)
    }
  }
  if (_dEscGroups.length) item._ctxSets.push({ escalationGroup: _dEscGroups })
}

function precomputeEntity(entity) {
  entity._searchName = entity.name
  // Every text an entity actually carries, not just `text`. `wordInBetween`
  // and `expression` are what an entity matches on at runtime, so an entity
  // findable in CM.com by one of them was not findable here.
  const words = []
  for (const w of entity.words) {
    if (w.text) words.push(w.text)
    if (w.wordInBetween) words.push(w.wordInBetween)
    if (w.expression) words.push(w.expression)
  }
  entity._searchWords = words
  // The trigger phrases alone, for the Article/Dialog enrichment: an entity
  // should not be dragged into a content result by the text of its own regex.
  entity._triggerWords = entity.words.map((w) => w.text)
  entity._nameUpper = entity.name.toUpperCase()
}

// Which entities are used in Articles / Dialogs, for the Entities tab pills.
//
// Resolved on the main thread by `getEntityForChip` and handed over as two name
// lists, because that function is the single source of truth for "which entity
// is this phrase?" — and it matches by word and by token, not only by the
// entity's literal name. The worker used to decide this itself with a
// name-equality check, so the pills disagreed with the cards they filtered.
function buildEntityXrefSets(articleNames, dialogNames) {
  entityHasArticleXref = new Set(articleNames || [])
  entityHasDialogXref = new Set(dialogNames || [])
}

// ── Match functions (receive pre-compiled regex) ──────────────────────────────
let matchingEntityNames = new Set()

// Determine if we can use fast plain-text matching (no regex, no word boundary)
function canUsePlainMatch() {
  return !searchRegex && !searchWord
}

/**
 * Does one tag set satisfy one filter?
 *
 * The single place the three kinds of filter value are told apart, because the
 * distinction was written out six times and adding `__any__` to five of them
 * would have been a filter that worked on Context but not Metadata, or on cards
 * but not in the modal.
 *
 * `__not_set__` passes when the key is absent and `__any__` when it is present
 * with any value at all — exact complements, and the only two that ask about the
 * key rather than about one of its values.
 */
function tagFilterMatches(set, f) {
  const vals = set[f.name]
  if (f.value === "__not_set__") return !vals
  if (f.value === "__any__") return !!vals
  return !!vals && vals.includes(f.value)
}

/** The tag set of an answer that carries nothing. */
const EMPTY_TAG_SET = {}

// Returns true if item passes the active content context filter set.
// An item passes if one of its outputs — an Answer or a route — has a ctxSet
// satisfying ALL active filters.
// NOTE: escalationGroup is stored in a dedicated aggregate ctxSet separate from
// per-answer contextVariable ctxSets, so it is checked independently and the
// results are AND'd with the regular context variable check.
function matchesContentContext(item) {
  if (!contentContextFilters.length) return true
  const ctxSets = item._ctxSets || []

  // Split filters: escalationGroup uses an item-level aggregate ctxSet;
  // regular context vars use per-output ctxSets. Must check independently
  // or combining the two always yields 0 results.
  const escFilters = contentContextFilters.filter(
    (f) => f.name === "escalationGroup",
  )
  const varFilters = contentContextFilters.filter(
    (f) => f.name !== "escalationGroup",
  )

  // ── Escalation group check ────────────────────────────────────────────────
  if (escFilters.length) {
    const aggCtx = ctxSets.find((cs) => cs.escalationGroup !== undefined)
    if (
      !aggCtx ||
      !escFilters.every((f) => aggCtx.escalationGroup.includes(f.value))
    )
      return false
  }

  // ── Regular context variable check ───────────────────────────────────────
  if (!varFilters.length) return true
  // Exclude the escalation aggregate ctxSet — it holds no regular vars and
  // would falsely satisfy any "__not_set__" check. Unconditioned outputs are
  // already in the list as empty sets, so "not set" needs no special case.
  return ctxSets.some(
    (ctxSet) =>
      ctxSet.escalationGroup === undefined &&
      varFilters.every((f) => tagFilterMatches(ctxSet, f)),
  )
}

/**
 * Whether an item has an Answer whose metadata satisfies every active filter.
 *
 * Deliberately simpler than `matchesContentContext`: metadata has no
 * escalationGroup special case (that tag is reachable through the Context tab,
 * where it has always lived). Like context, an answer with no metadata is a
 * real entry in `_metaSets` — an empty map — and so already satisfies
 * `__not_set__` on its own. Unlike context it reads Answers only: metadata is a
 * tag on a response, where context is a condition on any output.
 *
 * Filters on different keys must be satisfied by *one* answer, matching how
 * context works: an item whose answer A is `nochat=true` and whose answer B is
 * `entryType=choice_prompt` does not match a filter asking for both.
 */
function matchesContentMetadata(item) {
  if (!contentMetadataFilters.length) return true
  const sets = item._metaSets || []
  if (!sets.length) {
    // No answer carries metadata, so the only filter that can pass is one
    // asking for absence.
    return contentMetadataFilters.every((f) =>
      tagFilterMatches(EMPTY_TAG_SET, f),
    )
  }
  return sets.some((set) =>
    contentMetadataFilters.every((f) => tagFilterMatches(set, f)),
  )
}

// Check if a single answer's ctxSet satisfies all active content context filters.
function ctxSetMatchesFilters(ctxSet) {
  if (!contentContextFilters.length) return true
  return contentContextFilters.every((f) => tagFilterMatches(ctxSet, f))
}

// Check if a single answer item matches ALL terms in a single AND-group.
// The AND terms can be spread across the s/r/e fields of the same answer item.
function answerMatchesAndGroup(ai, andGroup) {
  const fields = [ai.s, ai.r, ai.e]
  const fieldsLower = [ai.sl, ai.rl, ai.el]
  return andGroupMatchesFields(andGroup, fields, fieldsLower)
}

// Check if any answer item satisfies both context filter AND ALL terms of ANY OR-group.
function answerItemsMatchOrGroups(answerItems, groups) {
  // For each OR-group, check if any single answer item satisfies all AND terms
  // AND the context filter.
  return groups.some((andGroup) =>
    (answerItems || []).some(
      (ai) =>
        (!searchExcludeNonDefault || !ai.isNonDefault) &&
        ctxSetMatchesFilters(ai.ctxSet) && answerMatchesAndGroup(ai, andGroup),
    ),
  )
}

// Combined match (context + text): the SAME answer must satisfy both.
function matchArticleCombined(a) {
  return answerItemsMatchOrGroups(a._answerItems, _orRegexGroups)
}

/// The Response items a node answers with, for matching. A reference node has
/// none of its own; under the Ref toggle it lends the Article's.
function nodeAnswerItems(sn) {
  if (!sn.isRef) return sn._answerItems
  if (!searchIncludeRefs) return []
  const art = articleById.get(sn.refId)
  return art ? art._answerItems : []
}

function matchDialogCombined(item) {
  return (item._searchNodes || []).some((sn) =>
    answerItemsMatchOrGroups(nodeAnswerItems(sn), _orRegexGroups),
  )
}

// ── Parts ───────────────────────────────────────────────────────────────────
//
// Text is matched within one *part* of an item at a time, never across two:
// an Article's questions, or one of its Responses; a Dialog's name and
// description, one node's name, one phrasing, or one Response. Every word of a
// chip must sit in the same part, and so must text chips joined by AND
// (`evalContentExpr`) — "kinderen AND korting" matched 10 Dialogs where only 2
// said both in one place. Ids, entities and tags stay conditions on the item.

/// The parts of an item a text condition can be found in, under the current
/// toggles: ¬T drops everything but Responses, ND the non-default ones, and a
/// reference node counts only under Ref.
function itemTextParts(item) {
  const out = []
  if (!searchContent) {
    out.push(...item._ncParts)
    if (searchIncludeRefs && item._refParts) out.push(...item._refParts)
  }
  const answers =
    item._kind === "article"
      ? item._answerItems || []
      : (item._searchNodes || []).flatMap((sn) => nodeAnswerItems(sn) || [])
  for (const ai of answers) if (!(searchExcludeNonDefault && ai.isNonDefault)) out.push(ai)
  return out
}

/// One term in one part — its text, or an entity its phrasings resolve to.
function partHasTerm(p, compiled) {
  if (termFoundInFields(compiled, p.f, p.l)) return true
  if (!p.ph || !matchingEntityNames.size) return false
  const names = entityNamesForPhrases(p.ph)
  return names.length > 0 && termMatchesEntityByNames(compiled, names)
}

function partMatchesGroup(p, andGroup) {
  return andGroup.every((compiled) => partHasTerm(p, compiled))
}

/// Does any OR group of `groups` have all its terms in one part?
function matchText(item, groups) {
  const parts = itemTextParts(item)
  return groups.some((g) => parts.some((p) => partMatchesGroup(p, g)))
}

function matchArticle(a) {
  return matchText(a, _orRegexGroups)
}

function matchDialog(item) {
  return matchText(item, _orRegexGroups)
}

function matchEntity(entity) {
  // Type and description included: "which entities are Regex ones?" and
  // "which one handles refunds?" were both unanswerable from this tab, and
  // the description had been parsed and discarded since the extractor was
  // written.
  const fields = [
    entity._searchName,
    entity.type || "",
    entity.description || "",
    ...entity._searchWords,
  ]
  return orGroupsMatchFields(_orRegexGroups, fields)
}

// ── The search bar as an expression ──────────────────────────────────────────
//
// The Content bar takes the same chips as the Conversations bar: text, ids,
// entities, context and metadata tags, joined by AND / OR / AND NOT and grouped
// with brackets. The main thread serialises them to the same string the
// Conversations bar sends (`exprToQuery`), and this is the worker's reading of
// it — a port of `scan_search_tokens` / `parse_search_expr` in lib.rs, with the
// same rules: keywords upper-case only, quoting escapes anything, strictly left
// to right with brackets overriding, runs of one operator folded into one
// n-ary node, malformed input read generously rather than rejected.
//
// Operators combine at the **item** level — the content equivalent of the
// conversation-level AND on the other side. Inside one text leaf the old rule
// still holds: all its terms must be found in the same answer.

function unquoteToken(s) {
  const t = String(s || "").trim()
  return t.length >= 2 && t[0] === '"' && t[t.length - 1] === '"' ? t.slice(1, -1) : t
}

function parseTagToken(word) {
  const low = word.toLowerCase()
  const kind = low.startsWith("ctx:") ? "context" : low.startsWith("meta:") ? "metadata" : null
  if (!kind) return null
  const rest = word.slice(kind === "context" ? 4 : 5)
  let name
  let after
  if (rest[0] === '"') {
    const close = rest.indexOf('"', 1)
    if (close < 0) return null
    name = rest.slice(1, close)
    after = rest.slice(close + 1)
  } else {
    const eq = rest.indexOf("=")
    name = eq < 0 ? rest : rest.slice(0, eq)
    after = eq < 0 ? "" : rest.slice(eq)
  }
  name = name.trim()
  if (!name) return null
  return { k: "tag", kind, name, values: after[0] === "=" ? parseTagValues(after.slice(1)) : [] }
}

/// `"a","b"` / `a,b` — the value list of a tag, any of which matches. `*`
/// anywhere means any value, which is an empty list. The mirror of the loop in
/// `TagLeaf::parse`.
function parseTagValues(src) {
  const out = []
  let any = false
  let j = 0
  while (j < src.length) {
    let v
    if (src[j] === '"') {
      const close = src.indexOf('"', j + 1)
      const end = close < 0 ? src.length : close
      v = src.slice(j + 1, end)
      j = end + 1
    } else {
      const comma = src.indexOf(",", j)
      const end = comma < 0 ? src.length : comma
      v = src.slice(j, end)
      j = end
    }
    v = v.trim()
    if (v === "*") any = true
    else if (v && !out.some((x) => x.toLowerCase() === v.toLowerCase())) out.push(v)
    while (j < src.length && src[j] !== ",") j++
    j++
  }
  return any ? [] : out
}

function parseIdToken(word) {
  let m = /^qa-(\d+)$/i.exec(word)
  if (m) return { k: "id", kind: "article", id: Number(m[1]) }
  m = /^dn-(\d+)(?:-(\d+))?$/i.exec(word)
  if (!m) return null
  return m[2] == null
    ? { k: "id", kind: "dialog", id: Number(m[1]) }
    : { k: "id", kind: "node", id: Number(m[1]), node: Number(m[2]) }
}

function scanExprTokens(input, regexMode) {
  const s = String(input || "")
  const out = []
  let i = 0
  while (i < s.length) {
    const c = s[i]
    if (/\s/.test(c)) {
      i++
      continue
    }
    if (!regexMode && (c === "(" || c === ")")) {
      out.push({ k: c === "(" ? "open" : "close" })
      i++
      continue
    }
    const start = i
    let quoted = false
    while (i < s.length) {
      const ch = s[i]
      if (ch === '"') {
        quoted = true
        i++
        while (i < s.length && s[i] !== '"') i++
        if (i < s.length) i++
        continue
      }
      if (/\s/.test(ch) || (!regexMode && (ch === "(" || ch === ")"))) break
      i++
    }
    const word = s.slice(start, i)
    if (!word) {
      i++
      continue
    }
    if (/^entity:/i.test(word)) {
      const name = unquoteToken(word.slice(7)).trim()
      if (name) {
        out.push({ k: "entity", name })
        continue
      }
    }
    const tag = parseTagToken(word)
    if (tag) {
      out.push(tag)
      continue
    }
    if (!quoted) {
      if (word === "AND" || word === "OR" || word === "NOT") {
        out.push({ k: word.toLowerCase() })
        continue
      }
      const id = parseIdToken(word)
      if (id) {
        out.push(id)
        continue
      }
    }
    out.push({ k: "text", raw: word })
  }
  return out
}

function parseExprTokens(toks, pos, depth) {
  const terms = []
  const ors = []
  for (;;) {
    while (toks[pos.i] && (toks[pos.i].k === "and" || toks[pos.i].k === "or")) pos.i++
    const t = parseExprTerm(toks, pos, depth)
    if (!t) {
      if (ors.length) ors.pop()
      break
    }
    terms.push(t)
    const nx = toks[pos.i]
    if (!nx) break
    if (nx.k === "and") {
      pos.i++
      ors.push(false)
    } else if (nx.k === "or") {
      pos.i++
      ors.push(true)
    } else if (nx.k === "not" || nx.k === "open" || nx.k === "id" || nx.k === "entity" || nx.k === "tag" || nx.k === "text") {
      ors.push(false)
    } else break
  }
  if (!terms.length) return null
  let result = terms.shift()
  let idx = 0
  while (idx < ors.length && idx < terms.length) {
    const isOr = ors[idx]
    const run = [terms[idx]]
    while (idx + 1 < ors.length && idx + 1 < terms.length && ors[idx + 1] === isOr) {
      idx++
      run.push(terms[idx])
    }
    result = { k: isOr ? "or" : "and", kids: [result].concat(run) }
    idx++
  }
  return result
}

function parseExprTerm(toks, pos, depth) {
  const t = toks[pos.i]
  if (!t) return null
  if (t.k === "not") {
    pos.i++
    if (depth > 32) return null
    const inner = parseExprTerm(toks, pos, depth + 1)
    if (!inner) return null
    return inner.k === "not" ? inner.kid : { k: "not", kid: inner }
  }
  if (t.k === "open") {
    pos.i++
    if (depth > 32) return null
    const inner = parseExprTokens(toks, pos, depth + 1)
    if (toks[pos.i] && toks[pos.i].k === "close") pos.i++
    return inner
  }
  if (t.k === "close") {
    if (depth === 0) {
      pos.i++
      return parseExprTerm(toks, pos, depth)
    }
    return null
  }
  if (t.k === "and" || t.k === "or") return null
  if (t.k === "text") {
    const parts = []
    while (toks[pos.i] && toks[pos.i].k === "text") parts.push(toks[pos.i++].raw)
    return { k: "text", raw: parts.join(" ") }
  }
  pos.i++
  return t
}

/// The expression tree for a Content query string, or `null` for nothing.
function parseContentExpr(input, regexMode) {
  return parseExprTokens(scanExprTokens(input, regexMode), { i: 0 }, 0)
}

/// Every text leaf of a tree, in order.
function exprTextLeaves(n, out) {
  const acc = out || []
  if (!n) return acc
  if (n.k === "text") acc.push(n)
  else if (n.k === "not") exprTextLeaves(n.kid, acc)
  else if (n.kids) for (const k of n.kids) exprTextLeaves(k, acc)
  return acc
}

function tagSetHas(set, name, values) {
  const vals = set[name]
  if (!vals) return false
  if (!values.length) return true
  // The chip's values are trimmed when it is read, so the stored ones are
  // too: "Voer hier je voor- en achternaam in " is in the export with the
  // space, and could not be found by its own chip.
  const want = values.map((v) => v.toLowerCase())
  return vals.some((v) => want.includes(String(v).trim().toLowerCase()))
}

function exprLeafMatches(n, item) {
  if (n.k === "text") return matchText(item, n.groups)
  if (n.k === "id") {
    if (n.kind === "article") return item._kind === "article" && item.Id === n.id
    if (item._kind === "article" || item.id !== n.id) return false
    return n.kind === "dialog" || (item.nodes || []).some((nd) => nd.id === n.node)
  }
  if (n.k === "entity") {
    const upper = n.name.toUpperCase()
    const phrases =
      item._kind === "article" ? item._searchQuestionsUpper : item._entityQuestionTexts
    return (phrases || []).some((p) => phraseEntityUpper.get(p) === upper)
  }
  if (n.k === "tag") {
    const sets = (n.kind === "metadata" ? item._metaSets : item._ctxSets) || []
    return sets.some((set) => tagSetHas(set, n.name, n.values))
  }
  return false
}

/// A subtree of text leaves joined by AND / OR — the kind that has to hold
/// within one part. NOT is not: an exclusion removes the whole item.
function isTextTree(n) {
  return n.k === "text" || ((n.k === "and" || n.k === "or") && n.kids.every(isTextTree))
}

function evalOnPart(n, p) {
  if (n.k === "text") return n.groups.some((g) => partMatchesGroup(p, g))
  if (n.k === "and") return n.kids.every((k) => evalOnPart(k, p))
  return n.kids.some((k) => evalOnPart(k, p))
}

function evalContentExpr(n, item) {
  if (n.k === "and") {
    // Text conditions ANDed together must meet in one part; everything else
    // (ids, entities, tags, exclusions) is a condition on the item.
    const text = n.kids.filter(isTextTree)
    for (const k of n.kids) if (!isTextTree(k) && !evalContentExpr(k, item)) return false
    if (text.length < 2) return text.every((k) => evalContentExpr(k, item))
    const parts = itemTextParts(item)
    return parts.some((p) => text.every((k) => evalOnPart(k, p)))
  }
  if (n.k === "or") return n.kids.some((k) => evalContentExpr(k, item))
  if (n.k === "not") return !evalContentExpr(n.kid, item)
  return exprLeafMatches(n, item)
}

// ── Where an item matched ───────────────────────────────────────────────────
//
// A result card says how many of its nodes, Responses and entities matched,
// and the info modal's "Matches only" shows exactly those. Both read this, so
// they agree with each other and with inclusion: a part counts when it would
// on its own have satisfied the words of the search — every word of a chip,
// and every AND-ed text chip, in that one part. Counting any part that holds
// any one word is what made "kinderen korting" read as 8 matching nodes on a
// Dialog where one node says both.

/// The search's positive conditions — its words and its ctx:/meta: tags —
/// with the exclusions and the other item-level leaves (ids, entities) left
/// out, or null when none remain. `keep` picks the leaves: "all", "text"
/// or "tag". `parkeren AND NOT kosten` → `parkeren`.
function positiveTree(n, keep) {
  if (!n) return null
  if (n.k === "text") return keep !== "tag" ? n : null
  if (n.k === "tag") return keep !== "text" ? n : null
  if (n.k !== "and" && n.k !== "or") return null
  const kids = n.kids.map((k) => positiveTree(k, keep)).filter(Boolean)
  if (!kids.length) return null
  return kids.length === 1 ? kids[0] : { k: n.k, kids }
}

/// Does a unit — a Dialog node, or one of an Article's Responses or
/// questions — hold the tree on its own? Text is read in its parts, under the
/// same rule as inclusion (AND-ed words meet in one part); a tag in its own
/// outputs' context and metadata. The mirror of `evalContentExpr`, one level
/// down.
function evalOnUnit(n, u) {
  if (n.k === "tag") {
    const sets = n.kind === "metadata" ? u.metaSets : u.ctxSets
    return sets.some((set) => tagSetHas(set, n.name, n.values))
  }
  if (isTextTree(n)) return u.parts.some((p) => evalOnPart(n, p))
  if (n.k === "or") return n.kids.some((k) => evalOnUnit(k, u))
  const text = n.kids.filter(isTextTree)
  for (const k of n.kids) if (!isTextTree(k) && !evalOnUnit(k, u)) return false
  if (text.length < 2) return text.every((k) => evalOnUnit(k, u))
  return u.parts.some((p) => text.every((k) => evalOnPart(k, p)))
}

/// How this search reads a unit, or null when nothing in it can point at a
/// part of an item (no words, no tags, no panel filter). `modes` are tried in
/// order per item until one counts something: words and tags together, then
/// the words alone, then the tags alone — so an item found because its words
/// sit in one node and its tag in another still says where.
function matchExplainer(tree, hasCtxFilter, hasMetaFilter) {
  const panel = hasCtxFilter || hasMetaFilter
  const panelOk = (u) =>
    (!hasCtxFilter || matchesContentContext({ _ctxSets: u.ctxSets })) &&
    (!hasMetaFilter || matchesContentMetadata({ _metaSets: u.metaSets }))
  let all = null
  let text = null
  let tag = null
  if (tree) {
    all = positiveTree(tree, "all")
    text = positiveTree(tree, "text")
    tag = positiveTree(tree, "tag")
  } else if (_orRegexGroups.length && _orRegexGroups.every((g) => g.length)) {
    text = all = { k: "text", groups: _orRegexGroups }
  }
  if (!all && !panel) return null
  const terms = text ? exprTextLeaves(text).flatMap((l) => l.groups.flat()) : []
  const mode = (t, withPanel) =>
    t || withPanel ? (u) => (!t || evalOnUnit(t, u)) && (!withPanel || panelOk(u)) : null
  return {
    modes: [mode(all, panel), text && (tag || panel) ? mode(text, false) : null, tag && text ? mode(tag, panel) : null].filter(Boolean),
    // The words alone, for an Article's questions and a Dialog's name: they
    // carry no tags of their own, so a tag never counts them.
    text: text ? (p) => evalOnPart(text, p) : null,
    // The loose reading (any one word), only to say which questions an
    // Article matched on when its words were spread across several of them —
    // the questions are one part to the search.
    any: (p) => terms.some((c) => partHasTerm(p, c)),
    // A plain query under a context filter matches Responses only
    // (`matchArticleCombined`), so questions and names are no reason then.
    onlyAnswers: !tree && hasCtxFilter && !!text,
  }
}

/// Which parts of a matched item the search found, by index:
/// an Article → `{q, r}` (its questions; its Answer outputs, in order),
/// a Dialog → `{n, h}` (its nodes; whether its name/description did).
function explainMatch(item, ex) {
  // A Response counts only under the Context filter, as the modal shows it.
  const answerOk = (ai) =>
    !(searchExcludeNonDefault && ai.isNonDefault) && ctxSetMatchesFilters(ai.ctxSet)
  const textOk = !searchContent && !ex.onlyAnswers
  const first = (count) => {
    for (const m of ex.modes) {
      const hits = count(m)
      if (hits.length) return hits
    }
    return []
  }
  if (item._kind === "article") {
    const r = first((m) => {
      const out = []
      item._answerItems.forEach((ai, i) => {
        if (answerOk(ai) && m({ parts: [ai], ...item._answerTags[i] })) out.push(i)
      })
      return out
    })
    const q = []
    if (textOk && ex.text) {
      item._qParts.forEach((p, i) => ex.text(p) && q.push(i))
      if (!q.length && ex.text(item._ncParts[0]))
        item._qParts.forEach((p, i) => ex.any(p) && q.push(i))
    }
    return { q, r }
  }
  if (item._kind !== "dialog") return null
  const n = first((m) => {
    const out = []
    item._searchNodes.forEach((sn, i) => {
      const parts = nodeAnswerItems(sn).filter(answerOk)
      if (textOk) {
        if (!sn.isRef || searchIncludeRefs) parts.push(sn.namePart)
        parts.push(...sn.phraseParts)
      }
      if (m({ parts, ctxSets: sn.ctxSets, metaSets: sn.metaSets })) out.push(i)
    })
    return out
  })
  return { n, h: textOk && !!ex.text && ex.text(item._ncParts[0]) }
}

// ── Message handler ───────────────────────────────────────────────────────────
self.onmessage = function (e) {
  const msg = e.data

  if (msg.type === "init") {
    const parsed = JSON.parse(msg.json)
    workerArticles = parsed.articles || []
    workerDialogs = parsed.dialogs || []
    workerEntities = parsed.entities || []

    // Build variable name maps so searches by name resolve to numeric ID refs
    convVarMap = new Map()
    ctxVarMap = new Map()
    ;(parsed.convVars || []).forEach((v) => convVarMap.set(v.id, v.name))
    ;(parsed.ctxVars || []).forEach((v) => ctxVarMap.set(v.id, v.name))

    // Assign within-array indices so results can be returned as cheap int arrays
    // instead of full Structured-Clone copies of every object.
    for (let i = 0; i < workerArticles.length; i++) workerArticles[i]._widx = i
    for (let i = 0; i < workerDialogs.length; i++) workerDialogs[i]._widx = i
    for (let i = 0; i < workerEntities.length; i++) workerEntities[i]._widx = i

    // Pre-compute searchable fields once on data load
    for (const a of workerArticles) precomputeArticle(a)
    articleById = new Map()
    for (const a of workerArticles) articleById.set(a.Id, a)
    for (const d of workerDialogs) precomputeDialog(d)
    for (const ent of workerEntities) precomputeEntity(ent)

    // Pre-build combined array (avoids concat on every search)
    allItems = workerArticles.concat(workerDialogs)

    // Assign global indices that mirror allCombinedItems order on the main thread
    for (let i = 0; i < allItems.length; i++) allItems[i]._gidx = i

    // Pre-build entity cross-reference sets
    buildEntityXrefSets(parsed.entityArticleNames, parsed.entityDialogNames)
    entityByNameUpper = new Map()
    for (const ent of workerEntities) entityByNameUpper.set(ent._nameUpper, ent)
    // Shipped as flat pairs rather than an object: phrases are arbitrary user
    // text and can collide with `Object.prototype` keys.
    phraseEntityUpper = new Map()
    const pairs = parsed.phraseEntity || []
    for (let i = 0; i + 1 < pairs.length; i += 2) {
      phraseEntityUpper.set(pairs[i], pairs[i + 1])
    }
    return
  }

  if (msg.type === "search") {
    const {
      id,
      query,
      allFilterPill,
      aFilter,
      dFilter,
      eFilter,
      allSort,
      aSort,
      dSort,
      eSort,
    } = msg

    // Update per-search options
    searchCase = msg.searchCase
    searchWord = msg.searchWord
    searchRegex = msg.searchRegex
    searchContent = msg.searchContent
    searchExcludeNonDefault = msg.searchExcludeNonDefault
    searchIncludeRefs = !!msg.searchIncludeRefs
    contentContextFilters = msg.contentContextFilters || []
    contentMetadataFilters = msg.contentMetadataFilters || []

    // An expression — anything more than one run of text — is read into a
    // tree; a lone text leaf is exactly the old query and takes the old path.
    let tree = msg.expr ? parseContentExpr(msg.expr, searchRegex) : null
    if (tree && tree.k === "text") tree = null
    const textLeaves = tree ? exprTextLeaves(tree) : []
    // What the entity cache, the Entities tab and the highlight read: the
    // words that were searched for, never the expression.
    const q = tree ? textLeaves.map((l) => l.raw).join(" | ") : query

    // ── Build OR-groups of AND-term regexes ONCE for this search ──────
    const orGroups = q ? parseOrGroups(q) : []
    _orRegexGroups = q ? buildOrRegexGroups(orGroups) : []
    let leavesInvalid = false
    for (const leaf of textLeaves) {
      leaf.groups = buildOrRegexGroups(parseOrGroups(leaf.raw))
      if (searchRegex && compiledGroupsHaveInvalidRegex(leaf.groups)) leavesInvalid = true
    }
    const invalidRegex =
      leavesInvalid || (q && searchRegex && compiledGroupsHaveInvalidRegex(_orRegexGroups))
    if (invalidRegex) {
      const filteredAllIdx = new Int32Array(0)
      const filteredArticlesIdx = new Int32Array(0)
      const filteredDialogsIdx = new Int32Array(0)
      const filteredEntitiesIdx = new Int32Array(0)
      self.postMessage(
        {
          type: "results",
          id,
          error: "invalid_regex",
          filteredAllIdx,
          filteredArticlesIdx,
          filteredDialogsIdx,
          filteredEntitiesIdx,
          matchingEntityNames: [],
        },
        [
          filteredAllIdx.buffer,
          filteredArticlesIdx.buffer,
          filteredDialogsIdx.buffer,
          filteredEntitiesIdx.buffer,
        ],
      )
      return
    }
    const hasValidQuery =
      _orRegexGroups.length > 0 && _orRegexGroups.every((g) => g.length > 0)

    const isPlain = q ? canUsePlainMatch() : false

    // Pre-compute entity names matched by the current query.
    // Cache by query+mode key so filter/sort-only changes skip this step.
    const entityCacheKey = q
      ? `${q}|${searchCase}|${searchWord}|${searchRegex}|${searchContent}`
      : ""
    if (entityCacheKey !== _entityCacheKey) {
      _entityCacheKey = entityCacheKey
      matchingEntityNames = new Set()
      if (q && !searchContent && workerEntities.length) {
        // Match entities against each individual term in the union of all OR groups
        const allTerms = orGroups.flat()
        const allTermRegexes = isPlain
          ? []
          : allTerms.map((term) => buildTermRegex(term))
        for (const entity of workerEntities) {
          // Trigger words only. An entity whose *regex source* happens to
          // contain the search term has not been "found in" an Article.
          const wordMatches = entity._triggerWords.some((w) => {
            if (isPlain) {
              return allTerms.some((term) => {
                const n = searchCase ? term : fold(term)
                return searchCase
                  ? w.indexOf(n) !== -1
                  : fold(w).indexOf(n) !== -1
              })
            }
            return allTermRegexes.some((termRe) => {
              if (!termRe) return false
              termRe.lastIndex = 0
              return termRe.test(w)
            })
          })
          if (wordMatches) matchingEntityNames.add(entity._nameUpper)
        }
      }
    }

    // Short-circuit: no query and no filter → return everything
    const noQuery = !tree && (!q || !hasValidQuery)
    const hasCtxFilter = contentContextFilters.length > 0
    const hasMetaFilter = contentMetadataFilters.length > 0

    // ── Match pass: compute each item's match result once ─────────────────
    // Stored as _mc (match+context) so the per-tab filter passes can reuse it
    // without re-running expensive match functions on the same data.
    //
    // Metadata is ANDed at the item level rather than being folded into the
    // combined text+context path. That path exists so a text query and a
    // context condition must be satisfied by the *same* answer; metadata is a
    // tag on the answer, not a condition on reaching it, so requiring the hit
    // and the tag to coincide would exclude items the user is looking for.
    const needsMatch = !noQuery || hasCtxFilter || hasMetaFilter
    if (needsMatch) {
      for (const item of allItems) {
        if (tree) {
          // The Context · Metadata panel still narrows, ANDed at the item
          // level; the same-answer pairing with text is a one-leaf rule.
          item._mc = matchesContentContext(item) && evalContentExpr(tree, item)
        } else if (noQuery) {
          item._mc = matchesContentContext(item)
        } else if (hasCtxFilter) {
          item._mc = item._kind === "article"
            ? matchArticleCombined(item)
            : matchDialogCombined(item)
        } else {
          item._mc = matchesContentContext(item) &&
            (item._kind === "article" ? matchArticle(item) : matchDialog(item))
        }
        if (item._mc && hasMetaFilter) item._mc = matchesContentMetadata(item)
      }
      // The Entities tab and the highlight read the union of the text leaves.
      if (tree) _orRegexGroups = q ? buildOrRegexGroups(orGroups) : []
    }

    // Where each matched item matched, for its card and "Matches only".
    // Keyed by `_gidx`; only items the words of the search found are in it.
    const matchInfo = new Map()
    const ex = needsMatch ? matchExplainer(tree, hasCtxFilter, hasMetaFilter) : null
    if (ex) {
      for (const item of allItems) {
        if (!item._mc) continue
        const info = explainMatch(item, ex)
        if (info) matchInfo.set(item._gidx, info)
      }
    }

    // ── Filter: All (articles + dialogs combined) ─────────────────────────
    let filteredAll
    if (!needsMatch && allFilterPill === "all") {
      filteredAll = allItems
    } else {
      filteredAll = allItems.filter((item) => {
        if (allFilterPill === "articles" && item._kind !== "article") return false
        if (allFilterPill === "dialogs" && item._kind !== "dialog") return false
        if (allFilterPill === "tdialogs" && item._kind !== "tdialog") return false
        return needsMatch ? item._mc : true
      })
    }
    filteredAll = sortBy(
      filteredAll,
      allSort,
      (i) => (i._kind === "article" ? i.Id : i.id),
      (i) => (i._kind === "article" ? aFaqQ(i) || "" : i.name || ""),
    )

    // ── Filter: Articles ──────────────────────────────────────────────────
    let filteredArticles
    if (!needsMatch && aFilter === "all") {
      filteredArticles = workerArticles
    } else {
      filteredArticles = workerArticles.filter((a) => {
        if (aFilter === "answer" && aKind(a) !== "answer") return false
        if (aFilter === "dialog" && aKind(a) === "answer") return false
        return needsMatch ? a._mc : true
      })
    }
    filteredArticles = sortBy(
      filteredArticles,
      aSort,
      (a) => a.Id,
      (a) => aFaqQ(a) || "",
    )

    // ── Filter: Dialogs ───────────────────────────────────────────────────
    let filteredDialogs
    if (!needsMatch && dFilter === "all") {
      filteredDialogs = workerDialogs
    } else {
      filteredDialogs = workerDialogs.filter((item) => {
        if (dFilter === "dialogs" && item._kind !== "dialog") return false
        if (dFilter === "tdialogs" && item._kind !== "tdialog") return false
        if (dFilter === "recognition" && item._kind === "tdialog") return false
        if (dFilter === "recognition" && !item._hasAnswerOutput) return false
        return needsMatch ? item._mc : true
      })
    }
    filteredDialogs = sortBy(
      filteredDialogs,
      dSort,
      (i) => i.id,
      (i) => i.name || "",
    )

    // ── Filter: Entities ──────────────────────────────────────────────────
    let filteredEntities
    if (noQuery && eFilter === "all") {
      filteredEntities = workerEntities
    } else {
      filteredEntities = workerEntities.filter((entity) => {
        if (
          eFilter === "articles" &&
          !entityHasArticleXref.has(entity._nameUpper)
        )
          return false
        if (
          eFilter === "dialogs" &&
          !entityHasDialogXref.has(entity._nameUpper)
        )
          return false
        if (noQuery) return true
        // An expression with no words in it names Articles and Dialogs, not
        // entities; the Entities tab is not narrowed by it.
        if (tree && !textLeaves.length) return true
        return matchEntity(entity)
      })
    }
    if (eSort === "name-asc")
      filteredEntities = filteredEntities
        .slice()
        .sort((a, b) => a.name.localeCompare(b.name))
    else if (eSort === "name-desc")
      filteredEntities = filteredEntities
        .slice()
        .sort((a, b) => b.name.localeCompare(a.name))
    else if (eSort === "words-desc")
      filteredEntities = filteredEntities
        .slice()
        .sort((a, b) => b.words.length - a.words.length)
    else if (eSort === "words-asc")
      filteredEntities = filteredEntities
        .slice()
        .sort((a, b) => a.words.length - b.words.length)

    // Send index arrays as Int32Array with buffer transfer — zero-copy, no Structured Clone.
    // The main thread reconstructs filtered arrays from its own allCombinedItems etc.
    // Fill typed arrays directly to avoid allocating intermediate JS arrays via .map().
    const filteredAllIdx = new Int32Array(filteredAll.length)
    for (let i = 0; i < filteredAll.length; i++) filteredAllIdx[i] = filteredAll[i]._gidx
    const filteredArticlesIdx = new Int32Array(filteredArticles.length)
    for (let i = 0; i < filteredArticles.length; i++) filteredArticlesIdx[i] = filteredArticles[i]._widx
    const filteredDialogsIdx = new Int32Array(filteredDialogs.length)
    for (let i = 0; i < filteredDialogs.length; i++) filteredDialogsIdx[i] = filteredDialogs[i]._widx
    const filteredEntitiesIdx = new Int32Array(filteredEntities.length)
    for (let i = 0; i < filteredEntities.length; i++) filteredEntitiesIdx[i] = filteredEntities[i]._widx
    self.postMessage(
      {
        type: "results",
        id,
        filteredAllIdx,
        filteredArticlesIdx,
        filteredDialogsIdx,
        filteredEntitiesIdx,
        matchingEntityNames: Array.from(matchingEntityNames),
        matchInfo,
      },
      [
        filteredAllIdx.buffer,
        filteredArticlesIdx.buffer,
        filteredDialogsIdx.buffer,
        filteredEntitiesIdx.buffer,
      ],
    )
  }
}
