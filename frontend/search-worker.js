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
// Dialog: _searchId (string), _searchName, _searchDesc, _searchNodes [{name, _answerItems}]
// Entity: _searchName, _searchWords [lowercased texts]

// Pre-computed entity cross-reference sets (built on init)
let entityHasArticleXref = new Set() // entity names (upper) that have article xrefs
let entityHasDialogXref = new Set() // entity names (upper) that have dialog xrefs
let entityByNameUpper = new Map() // entity name (upper) → entity object, for per-term lookups

// Variable name maps (id → name), populated on "init" from dialogs export
let convVarMap = new Map() // ConversationVariable id → name
let ctxVarMap = new Map() // ContextVariable id → name

// ── Search options (updated on each "search" message) ────────────────────────
let searchCase = false
let searchWord = false
let searchRegex = false
let searchContent = true
let searchExcludeNonDefault = false
let contentContextFilters = [] // [{name, value}] — active content context filters

// ── Utilities ────────────────────────────────────────────────────────────────
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

function parseOrGroups(q) {
  if (!q) return []
  if (searchRegex) return [[q]]
  return q
    .split("|")
    .map((g) => tokenizeSegment(g.trim()))
    .filter((g) => g.length > 0)
}

// Build a regex for a single escaped term (respects searchCase and searchWord).
function buildTermRegex(term) {
  try {
    let pat = searchRegex
      ? term
      : term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
    if (searchWord)
      pat = "(?<![\\w\\u00C0-\\u024F])" + pat + "(?![\\w\\u00C0-\\u024F])"
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

function buildOrRegexGroups(orGroups) {
  return orGroups.map((andTerms) =>
    andTerms.map((term) => ({
      re: !canUsePlainMatch() ? buildTermRegex(term) : null,
      needle: canUsePlainMatch()
        ? searchCase
          ? term
          : term.toLowerCase()
        : null,
    })),
  )
}

// Test a single string against one compiled term {re, needle}.
function testTerm(compiled, str) {
  if (!str) return false
  if (compiled.re) {
    compiled.re.lastIndex = 0
    return compiled.re.test(str)
  }
  if (compiled.needle !== null) {
    return searchCase
      ? str.indexOf(compiled.needle) !== -1
      : str.toLowerCase().indexOf(compiled.needle) !== -1
  }
  return false
}

// Test a single term against multiple field strings (any match = term is found).
function termFoundInFields(compiled, fields) {
  return fields.some((f) => f != null && testTerm(compiled, f))
}

// Entity enrichment: does this specific compiled term match any word of any entity
// in the given list of entity name uppers?
function termMatchesEntityByNames(compiled, entityNameUppers) {
  for (const nameUpper of entityNameUppers) {
    const entity = entityByNameUpper.get(nameUpper)
    if (entity && termFoundInFields(compiled, entity._searchWords)) return true
  }
  return false
}

// Check if ALL terms in an AND-group are each found somewhere in the given fields.
function andGroupMatchesFields(andGroup, fields) {
  return andGroup.every((compiled) => termFoundInFields(compiled, fields))
}

// Check if ANY OR-group's AND terms all match the given fields.
function orGroupsMatchFields(groups, fields) {
  return groups.some((andGroup) => andGroupMatchesFields(andGroup, fields))
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

  // Build context sets for contextual Answer outputs
  a._ctxSets = []
  a._hasDefaultAnswer = false
  for (const o of a.Outputs) {
    if (o.Type !== "Answer") continue
    const cvs = o.ContextVariables || []
    if (!cvs.some((cv) => cv.Values && !cv.Values.includes("any"))) {
      a._hasDefaultAnswer = true
      continue
    }
    const ctxSet = {}
    for (const cv of cvs) {
      const name = ctxVarMap.get(cv.Id)
      // escalationGroup is handled by the OutputMetaData aggregate pass below
      if (!name || name === "escalationGroup") continue
      const vals = []
      for (const valStr of cv.Values) {
        for (const v of valStr.split(",")) {
          const t = v.trim()
          if (t && t !== "any") vals.push(t)
        }
      }
      if (vals.length) ctxSet[name] = vals
    }
    if (Object.keys(ctxSet).length) a._ctxSets.push(ctxSet)
  }

  // Add aggregate escalation group ctxSet so the escalationGroup filter works.
  // All groups found across Answer outputs are merged into a single ctxSet so
  // multi-select (AND logic) can match articles that cover multiple groups.
  const _escGroups = []
  for (const o of a.Outputs) {
    if (o.Type !== "Answer") continue
    const g = o.OutputMetaData && o.OutputMetaData.escalationGroup
    if (g && !_escGroups.includes(g)) _escGroups.push(g)
  }
  if (_escGroups.length) a._ctxSets.push({ escalationGroup: _escGroups })

  // Build aligned per-answer data: {s, r, e, ctxSet} for every Answer output.
  // This links text and context conditions for the SAME answer so combined
  // text+context filtering can require both to be satisfied by one answer.
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
        if (vals.length) _aCtx[name] = vals
      }
    }
    const _aEscGroup = _ao.OutputMetaData && _ao.OutputMetaData.escalationGroup
    if (_aEscGroup) _aCtx.escalationGroup = [_aEscGroup]
    a._answerItems.push({
      s: strip(_aExp),
      r: _arT,
      e: expandVarNames(_aExp),
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
        if (vals.length) _nCtx[name] = vals
      }
      const _nEscGroup = _nai.metadata && _nai.metadata.escalationGroup
      if (_nEscGroup) _nCtx.escalationGroup = [_nEscGroup]
      _nodeAnsItems.push({
        s: strip(_nExp),
        r: _nrT,
        e: expandVarNames(_nExp),
        ctxSet: _nCtx,
        isNonDefault: _nai !== ans && !_nai.isDefault,
      })
    }
    return {
      name: n.name || "",
      _answerItems: _nodeAnsItems,
    }
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

  // Build context sets for contextual Answer items in nodes
  item._ctxSets = []
  item._hasDefaultAnswer = false
  for (const n of nodes) {
    for (const oi of (n.output && n.output.items) || []) {
      if (oi.type !== "Answer") continue
      const cvs = oi.contextVariables || []
      if (!cvs.length) {
        item._hasDefaultAnswer = true
        continue
      }
      const ctxSet = {}
      for (const cv of cvs) {
        const name = ctxVarMap.get(cv.id)
        // escalationGroup is handled by the metadata aggregate pass below
        if (!name || !cv.value || name === "escalationGroup") continue
        const vals = cv.value
          .split(",")
          .map((v) => v.trim())
          .filter(Boolean)
        if (vals.length) ctxSet[name] = vals
      }
      if (Object.keys(ctxSet).length) item._ctxSets.push(ctxSet)
      else item._hasDefaultAnswer = true
    }
  }

  // Add aggregate escalation group ctxSet so the escalationGroup filter works.
  const _dEscGroups = []
  for (const n of nodes) {
    for (const oi of (n.output && n.output.items) || []) {
      if (oi.type !== "Answer") continue
      const g = oi.metadata && oi.metadata.escalationGroup
      if (g && !_dEscGroups.includes(g)) _dEscGroups.push(g)
    }
  }
  if (_dEscGroups.length) item._ctxSets.push({ escalationGroup: _dEscGroups })
}

function precomputeEntity(entity) {
  entity._searchName = entity.name
  entity._searchWords = entity.words.map((w) => w.text)
  entity._nameUpper = entity.name.toUpperCase()
}

function buildEntityXrefSets() {
  entityHasArticleXref = new Set()
  entityHasDialogXref = new Set()

  // Build a set of all question texts from articles (uppercased)
  const articleQuestionTexts = new Set()
  for (const a of workerArticles) {
    for (const qs of a.Questions) {
      articleQuestionTexts.add(qs.Text.toUpperCase())
    }
  }

  // Build a set of all entity question texts from dialogs (uppercased)
  const dialogEntityTexts = new Set()
  for (const item of workerDialogs) {
    for (const t of item._entityQuestionTexts || []) {
      dialogEntityTexts.add(t)
    }
  }

  for (const entity of workerEntities) {
    const nameUpper = entity._nameUpper
    if (articleQuestionTexts.has(nameUpper)) {
      entityHasArticleXref.add(nameUpper)
    }
    if (dialogEntityTexts.has(nameUpper)) {
      entityHasDialogXref.add(nameUpper)
    }
  }
}

// ── Match functions (receive pre-compiled regex) ──────────────────────────────
let matchingEntityNames = new Set()

// Determine if we can use fast plain-text matching (no regex, no word boundary)
function canUsePlainMatch() {
  return !searchRegex && !searchWord
}

// Returns true if item passes the active content context filter set.
// An item passes if it has at least one ctxSet satisfying ALL active filters.
// For "not set" filters the implicit default answer ({}) is also considered.
// NOTE: escalationGroup is stored in a dedicated aggregate ctxSet separate from
// per-answer contextVariable ctxSets, so it is checked independently and the
// results are AND'd with the regular context variable check.
function matchesContentContext(item) {
  if (!contentContextFilters.length) return true
  const ctxSets = item._ctxSets || []

  // Split filters: escalationGroup uses an item-level aggregate ctxSet;
  // regular context vars use per-answer ctxSets. Must check independently
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
  // would falsely satisfy any "__not_set__" check.
  const varCtxSets = ctxSets.filter((cs) => cs.escalationGroup === undefined)
  const hasNotSetFilter = varFilters.some((f) => f.value === "__not_set__")
  if (hasNotSetFilter && item._hasDefaultAnswer) {
    return (
      varCtxSets.some((ctxSet) =>
        varFilters.every((f) => {
          if (f.value === "__not_set__") return !ctxSet[f.name]
          const vals = ctxSet[f.name]
          return vals && vals.includes(f.value)
        }),
      ) ||
      varFilters.every((f) => {
        if (f.value === "__not_set__") return true // default answer has nothing set
        return false // regular filter can't be satisfied by empty ctxSet
      })
    )
  }
  return varCtxSets.some((ctxSet) =>
    varFilters.every((f) => {
      if (f.value === "__not_set__") return !ctxSet[f.name]
      const vals = ctxSet[f.name]
      return vals && vals.includes(f.value)
    }),
  )
}

// Check if a single answer's ctxSet satisfies all active content context filters.
// Supports "__not_set__" sentinel: passes when the variable is absent from ctxSet.
function ctxSetMatchesFilters(ctxSet) {
  if (!contentContextFilters.length) return true
  return contentContextFilters.every((f) => {
    if (f.value === "__not_set__") return !ctxSet[f.name]
    const vals = ctxSet[f.name]
    return vals && vals.includes(f.value)
  })
}

// Check if a single answer item matches ALL terms in a single AND-group.
// The AND terms can be spread across the s/r/e fields of the same answer item.
function answerMatchesAndGroup(ai, andGroup) {
  const fields = [ai.s, ai.r, ai.e]
  return andGroupMatchesFields(andGroup, fields)
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

function matchDialogCombined(item) {
  return (item._searchNodes || []).some((sn) =>
    answerItemsMatchOrGroups(sn._answerItems, _orRegexGroups),
  )
}

function matchArticle(a) {
  const articleEntityNames =
    !searchContent && matchingEntityNames.size > 0
      ? a._searchQuestionsUpper.filter((t) => matchingEntityNames.has(t))
      : []

  // Non-content fields (id, question texts) form a single per-item bucket.
  // Evaluated only when !searchContent; all AND terms must be in this bucket alone.
  if (!searchContent) {
    const nonContentFields = [a._searchId]
    for (const qs of a.Questions) nonContentFields.push(qs.Text)
    if (
      _orRegexGroups.some((andGroup) =>
        andGroup.every(
          (compiled) =>
            termFoundInFields(compiled, nonContentFields) ||
            (articleEntityNames.length > 0 &&
              termMatchesEntityByNames(compiled, articleEntityNames)),
        ),
      )
    )
      return true
  }

  // Content matching: ALL AND terms of an OR-group must be found within the
  // SAME answer item (default or contextual). Terms may not straddle different answers.
  return _orRegexGroups.some((andGroup) =>
    (a._answerItems || []).some((ai) => {
      if (searchExcludeNonDefault && ai.isNonDefault) return false
      const fields = [ai.s, ai.r, ai.e]
      return andGroup.every(
        (compiled) => termFoundInFields(compiled, fields),
      )
    }),
  )
}

function matchDialog(item) {
  const dialogEntityNames =
    !searchContent && matchingEntityNames.size > 0
      ? item._entityQuestionTexts.filter((t) => matchingEntityNames.has(t))
      : []

  // Non-content fields (id, name, description, node names) form a single per-item bucket.
  // Evaluated only when !searchContent; all AND terms must be in this bucket alone.
  if (!searchContent) {
    const nonContentFields = [
      item._searchId,
      item._searchName,
      item._searchDesc,
    ]
    for (const sn of item._searchNodes || []) nonContentFields.push(sn.name)
    if (
      _orRegexGroups.some((andGroup) =>
        andGroup.every(
          (compiled) =>
            termFoundInFields(compiled, nonContentFields) ||
            (dialogEntityNames.length > 0 &&
              termMatchesEntityByNames(compiled, dialogEntityNames)),
        ),
      )
    )
      return true
  }

  // Content matching: ALL AND terms of an OR-group must be found within the
  // SAME answer item in the SAME node. Terms may not straddle different nodes or answers.
  return _orRegexGroups.some((andGroup) =>
    (item._searchNodes || []).some((sn) =>
      (sn._answerItems || []).some((ai) => {
        if (searchExcludeNonDefault && ai.isNonDefault) return false
        const fields = [ai.s, ai.r, ai.e]
        return andGroup.every(
          (compiled) => termFoundInFields(compiled, fields),
        )
      }),
    ),
  )
}

function matchEntity(entity) {
  const fields = [entity._searchName, ...entity._searchWords]
  return orGroupsMatchFields(_orRegexGroups, fields)
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
    for (const d of workerDialogs) precomputeDialog(d)
    for (const ent of workerEntities) precomputeEntity(ent)

    // Pre-build combined array (avoids concat on every search)
    allItems = workerArticles.concat(workerDialogs)

    // Assign global indices that mirror allCombinedItems order on the main thread
    for (let i = 0; i < allItems.length; i++) allItems[i]._gidx = i

    // Pre-build entity cross-reference sets
    buildEntityXrefSets()
    entityByNameUpper = new Map()
    for (const ent of workerEntities) entityByNameUpper.set(ent._nameUpper, ent)
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
    contentContextFilters = msg.contentContextFilters || []

    const q = query

    // ── Build OR-groups of AND-term regexes ONCE for this search ──────
    const orGroups = q ? parseOrGroups(q) : []
    _orRegexGroups = q ? buildOrRegexGroups(orGroups) : []
    const invalidRegex = q && searchRegex && compiledGroupsHaveInvalidRegex(_orRegexGroups)
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

    // Pre-compute entity names matched by the current query
    matchingEntityNames = new Set()
    if (q && !searchContent && workerEntities.length) {
      // Match entities against each individual term in the union of all OR groups
      const allTerms = orGroups.flat()
      const allTermRegexes = isPlain
        ? []
        : allTerms.map((term) => buildTermRegex(term))
      for (const entity of workerEntities) {
        const wordMatches = entity._searchWords.some((w) => {
          if (isPlain) {
            return allTerms.some((term) => {
              const n = searchCase ? term : term.toLowerCase()
              return searchCase
                ? w.indexOf(n) !== -1
                : w.toLowerCase().indexOf(n) !== -1
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

    // Short-circuit: no query and no filter → return everything
    const noQuery = !q || !hasValidQuery
    const hasCtxFilter = contentContextFilters.length > 0
    // ── Filter: All (articles + dialogs combined) ─────────────────────────
    let filteredAll
    if (noQuery && !hasCtxFilter && allFilterPill === "all") {
      filteredAll = allItems
    } else {
      filteredAll = allItems.filter((item) => {
        if (allFilterPill === "articles" && item._kind !== "article")
          return false
        if (allFilterPill === "dialogs" && item._kind !== "dialog") return false
        if (allFilterPill === "tdialogs" && item._kind !== "tdialog")
          return false
        if (noQuery) return matchesContentContext(item)
        if (hasCtxFilter)
          return item._kind === "article"
            ? matchArticleCombined(item)
            : matchDialogCombined(item)
        if (!matchesContentContext(item)) return false
        return item._kind === "article" ? matchArticle(item) : matchDialog(item)
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
    if (noQuery && !hasCtxFilter && aFilter === "all") {
      filteredArticles = workerArticles
    } else {
      filteredArticles = workerArticles.filter((a) => {
        if (aFilter === "answer" && aKind(a) !== "answer") return false
        if (aFilter === "dialog" && aKind(a) === "answer") return false
        if (noQuery) return matchesContentContext(a)
        if (hasCtxFilter) return matchArticleCombined(a)
        if (!matchesContentContext(a)) return false
        return matchArticle(a)
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
    if (noQuery && !hasCtxFilter && dFilter === "all") {
      filteredDialogs = workerDialogs
    } else {
      filteredDialogs = workerDialogs.filter((item) => {
        if (dFilter === "dialogs" && item._kind !== "dialog") return false
        if (dFilter === "tdialogs" && item._kind !== "tdialog") return false
        if (dFilter === "recognition" && item._kind === "tdialog") return false
        if (dFilter === "recognition" && !item._hasAnswerOutput) return false
        if (noQuery) return matchesContentContext(item)
        if (hasCtxFilter) return matchDialogCombined(item)
        if (!matchesContentContext(item)) return false
        return matchDialog(item)
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
    const filteredAllIdx = new Int32Array(filteredAll.map((x) => x._gidx))
    const filteredArticlesIdx = new Int32Array(
      filteredArticles.map((a) => a._widx),
    )
    const filteredDialogsIdx = new Int32Array(
      filteredDialogs.map((d) => d._widx),
    )
    const filteredEntitiesIdx = new Int32Array(
      filteredEntities.map((e) => e._widx),
    )
    self.postMessage(
      {
        type: "results",
        id,
        filteredAllIdx,
        filteredArticlesIdx,
        filteredDialogsIdx,
        filteredEntitiesIdx,
        matchingEntityNames: Array.from(matchingEntityNames),
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
