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
// Article: _searchId (string), _searchQuestions (uppercased texts), _searchResponse (stripped)
// Dialog: _searchId (string), _searchName, _searchDesc, _searchNodes [{name, answerText}]
// Entity: _searchName, _searchWords [lowercased texts]

// Pre-computed entity cross-reference sets (built on init)
let entityHasArticleXref = new Set() // entity names (upper) that have article xrefs
let entityHasDialogXref = new Set() // entity names (upper) that have dialog xrefs

// Variable name maps (id → name), populated on "init" from dialogs export
let convVarMap = new Map() // ConversationVariable id → name
let ctxVarMap = new Map() // ContextVariable id → name

// ── Search options (updated on each "search" message) ────────────────────────
let searchCase = false
let searchWord = false
let searchRegex = false
let searchContent = true
let contentContextFilters = [] // [{name, value}] — active content context filters

// ── Utilities ────────────────────────────────────────────────────────────────
function buildSearchRegex(q) {
  if (!q) return null
  try {
    let pat = searchRegex ? q : q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
    if (searchWord)
      pat = "(?<![\\w\\u00C0-\\u024F])" + pat + "(?![\\w\\u00C0-\\u024F])"
    return new RegExp(pat, searchCase ? "" : "i")
  } catch (e) {
    return null
  }
}

function testRe(re, str) {
  if (!re || !str) return false
  re.lastIndex = 0
  return re.test(str)
}

// Fast plain-text test: uses indexOf (much faster than regex for literal strings)
function testPlain(needle, haystack) {
  if (!needle || !haystack) return false
  return haystack.indexOf(needle) !== -1
}

function testPlainCI(needleLower, haystack) {
  if (!needleLower || !haystack) return false
  return haystack.toLowerCase().indexOf(needleLower) !== -1
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

function aResponse(a) {
  return a._response // pre-computed on init
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
  const o = a.Outputs.find((o) => o.Type === "Answer")
  a._response = o ? o.Text : null

  // Build per-article link label map (TagId → Label) for expanding %{Link(N)}
  const linkMap = new Map()
  ;((o && o.Links) || []).forEach((l) => {
    if (l.TagId && l.Label) linkMap.set(l.TagId, l.Label)
  })
  const expandedWithLinks = (a._response || "").replace(
    /%\{Link\((\d+)\)\}/g,
    (_, n) => linkMap.get(Number(n)) || "Link " + n,
  )

  // Pre-compute search fields
  a._searchId = String(a.Id)
  a._searchResponse = strip(a._response || "")
  a._searchResponseRaw = a._response || ""
  a._searchResponseExpanded = expandVarNames(expandedWithLinks)
  a._searchQuestionsUpper = a.Questions.map((qs) => qs.Text.toUpperCase())

  // Index ALL Answer outputs (contextual alternatives not covered above)
  const _primaryAnsIdx = a.Outputs.findIndex((o) => o.Type === "Answer")
  a._searchCtxAnswers = []
  for (let _oi = 0; _oi < a.Outputs.length; _oi++) {
    const _co = a.Outputs[_oi]
    if (_co.Type !== "Answer" || _oi === _primaryAnsIdx) continue
    const _lm = new Map()
    ;(_co.Links || []).forEach((l) => {
      if (l.TagId && l.Label) _lm.set(l.TagId, l.Label)
    })
    const _exp = (_co.Text || "").replace(
      /%\{Link\((\d+)\)\}/g,
      (_, n) => _lm.get(Number(n)) || "Link " + n,
    )
    a._searchCtxAnswers.push({
      s: strip(_exp),
      r: _co.Text || "",
      e: expandVarNames(_exp),
    })
  }

  // Build context sets for contextual Answer outputs
  a._ctxSets = []
  for (const o of a.Outputs) {
    if (o.Type !== "Answer") continue
    const cvs = o.ContextVariables || []
    if (!cvs.some((cv) => cv.Values && !cv.Values.includes("any"))) continue
    const ctxSet = {}
    for (const cv of cvs) {
      const name = ctxVarMap.get(cv.Id)
      if (!name) continue
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
    a._answerItems.push({
      s: strip(_aExp),
      r: _arT,
      e: expandVarNames(_aExp),
      ctxSet: _aCtx,
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
    const ans = nodeAnsItems[0] || null
    const rawText = (ans && ans.data && ans.data.text) || ""
    // Expand %{Link(N)} tokens for the primary answer using its hyperlinks
    const _primaryLm = new Map()
    ;((ans && ans.data && ans.data.hyperlinks) || []).forEach((h) => {
      if (h.id !== undefined && h.label) _primaryLm.set(h.id, h.label)
    })
    const _primaryExp = rawText.replace(
      /%\{Link\((\d+)\)\}/g,
      (_, n) => _primaryLm.get(Number(n)) || "Link " + n,
    )
    // Build search entries for all non-primary Answer items (contextual alternatives)
    const ctxAnswerTexts = []
    for (let _i = 1; _i < nodeAnsItems.length; _i++) {
      const _ci = nodeAnsItems[_i]
      const _rawT = (_ci.data && _ci.data.text) || ""
      const _lm = new Map()
      ;((_ci.data && _ci.data.hyperlinks) || []).forEach((h) => {
        if (h.id !== undefined && h.label) _lm.set(h.id, h.label)
      })
      const _exp = _rawT.replace(
        /%\{Link\((\d+)\)\}/g,
        (_, n) => _lm.get(Number(n)) || "Link " + n,
      )
      ctxAnswerTexts.push({ s: strip(_exp), r: _rawT, e: expandVarNames(_exp) })
    }
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
      _nodeAnsItems.push({
        s: strip(_nExp),
        r: _nrT,
        e: expandVarNames(_nExp),
        ctxSet: _nCtx,
      })
    }
    return {
      name: n.name || "",
      answerText: ans ? strip(_primaryExp) : "",
      answerTextRaw: rawText,
      answerTextExpanded: expandVarNames(_primaryExp),
      ctxAnswerTexts,
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
  for (const n of nodes) {
    for (const oi of (n.output && n.output.items) || []) {
      if (oi.type !== "Answer") continue
      const cvs = oi.contextVariables || []
      if (!cvs.length) continue
      const ctxSet = {}
      for (const cv of cvs) {
        const name = ctxVarMap.get(cv.id)
        if (!name || !cv.value) continue
        const vals = cv.value
          .split(",")
          .map((v) => v.trim())
          .filter(Boolean)
        if (vals.length) ctxSet[name] = vals
      }
      if (Object.keys(ctxSet).length) item._ctxSets.push(ctxSet)
    }
  }
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
function matchesContentContext(item) {
  if (!contentContextFilters.length) return true
  return (item._ctxSets || []).some((ctxSet) =>
    contentContextFilters.every((f) => {
      const vals = ctxSet[f.name]
      return vals && vals.includes(f.value)
    }),
  )
}

// Check if a single answer's ctxSet satisfies all active content context filters.
// An empty ctxSet (default/unconditional answer) never satisfies a non-empty filter.
function ctxSetMatchesFilters(ctxSet) {
  if (!contentContextFilters.length) return true
  if (!Object.keys(ctxSet).length) return false
  return contentContextFilters.every((f) => {
    const vals = ctxSet[f.name]
    return vals && vals.includes(f.value)
  })
}

// Check if a single answer item matches the text query.
function answerMatchesText(ai, re, isPlain, needle) {
  if (isPlain) {
    return searchCase
      ? testPlain(needle, ai.s) ||
          testPlain(needle, ai.r) ||
          testPlain(needle, ai.e)
      : testPlainCI(needle, ai.s) ||
          testPlainCI(needle, ai.r) ||
          testPlainCI(needle, ai.e)
  }
  return testRe(re, ai.s) || testRe(re, ai.r) || testRe(re, ai.e)
}

// Combined match: the SAME answer must satisfy both context filter AND text query.
function matchArticleCombined(a, re, isPlain, needle) {
  return (a._answerItems || []).some(
    (ai) =>
      ctxSetMatchesFilters(ai.ctxSet) &&
      answerMatchesText(ai, re, isPlain, needle),
  )
}

function matchDialogCombined(item, re, isPlain, needle) {
  return (item._searchNodes || []).some((sn) =>
    (sn._answerItems || []).some(
      (ai) =>
        ctxSetMatchesFilters(ai.ctxSet) &&
        answerMatchesText(ai, re, isPlain, needle),
    ),
  )
}

function matchArticle(a, re, isPlain, needle) {
  if (!searchContent) {
    if (isPlain) {
      if (searchCase) {
        if (testPlain(needle, a._searchId)) return true
        if (a.Questions.some((qs) => testPlain(needle, qs.Text))) return true
      } else {
        if (testPlainCI(needle, a._searchId)) return true
        if (a.Questions.some((qs) => testPlainCI(needle, qs.Text))) return true
      }
    } else {
      if (testRe(re, a._searchId)) return true
      if (a.Questions.some((qs) => testRe(re, qs.Text))) return true
    }
  }
  // Search response text (always searched)
  if (isPlain) {
    if (
      searchCase
        ? testPlain(needle, a._searchResponse) ||
          testPlain(needle, a._searchResponseRaw) ||
          testPlain(needle, a._searchResponseExpanded)
        : testPlainCI(needle, a._searchResponse) ||
          testPlainCI(needle, a._searchResponseRaw) ||
          testPlainCI(needle, a._searchResponseExpanded)
    )
      return true
  } else {
    if (
      testRe(re, a._searchResponse) ||
      testRe(re, a._searchResponseRaw) ||
      testRe(re, a._searchResponseExpanded)
    )
      return true
  }
  // Search contextual / alternative Answer outputs
  for (const ca of a._searchCtxAnswers || []) {
    if (isPlain) {
      if (
        searchCase
          ? testPlain(needle, ca.s) ||
            testPlain(needle, ca.r) ||
            testPlain(needle, ca.e)
          : testPlainCI(needle, ca.s) ||
            testPlainCI(needle, ca.r) ||
            testPlainCI(needle, ca.e)
      )
        return true
    } else {
      if (testRe(re, ca.s) || testRe(re, ca.r) || testRe(re, ca.e)) return true
    }
  }
  // Entity-word enrichment
  if (
    matchingEntityNames.size > 0 &&
    a._searchQuestionsUpper.some((t) => matchingEntityNames.has(t))
  )
    return true
  return false
}

function matchDialog(item, re, isPlain, needle) {
  if (!searchContent) {
    if (isPlain) {
      if (searchCase) {
        if (testPlain(needle, item._searchId)) return true
        if (testPlain(needle, item._searchName)) return true
        if (testPlain(needle, item._searchDesc)) return true
      } else {
        if (testPlainCI(needle, item._searchId)) return true
        if (testPlainCI(needle, item._searchName)) return true
        if (testPlainCI(needle, item._searchDesc)) return true
      }
    } else {
      if (testRe(re, item._searchId)) return true
      if (testRe(re, item._searchName)) return true
      if (testRe(re, item._searchDesc)) return true
    }
  }
  // Check node content
  for (const sn of item._searchNodes) {
    if (!searchContent) {
      if (isPlain) {
        if (
          searchCase ? testPlain(needle, sn.name) : testPlainCI(needle, sn.name)
        )
          return true
      } else {
        if (testRe(re, sn.name)) return true
      }
    }
    if (sn.answerText || sn.answerTextRaw) {
      if (isPlain) {
        if (
          searchCase
            ? testPlain(needle, sn.answerText) ||
              testPlain(needle, sn.answerTextRaw) ||
              testPlain(needle, sn.answerTextExpanded)
            : testPlainCI(needle, sn.answerText) ||
              testPlainCI(needle, sn.answerTextRaw) ||
              testPlainCI(needle, sn.answerTextExpanded)
        )
          return true
      } else {
        if (
          testRe(re, sn.answerText) ||
          testRe(re, sn.answerTextRaw) ||
          testRe(re, sn.answerTextExpanded)
        )
          return true
      }
    }
    // Search contextual / alternative Answer items in this node
    for (const ca of sn.ctxAnswerTexts || []) {
      if (isPlain) {
        if (
          searchCase
            ? testPlain(needle, ca.s) ||
              testPlain(needle, ca.r) ||
              testPlain(needle, ca.e)
            : testPlainCI(needle, ca.s) ||
              testPlainCI(needle, ca.r) ||
              testPlainCI(needle, ca.e)
        )
          return true
      } else {
        if (testRe(re, ca.s) || testRe(re, ca.r) || testRe(re, ca.e))
          return true
      }
    }
  }
  // Entity-word enrichment
  if (
    matchingEntityNames.size > 0 &&
    item._entityQuestionTexts.some((t) => matchingEntityNames.has(t))
  )
    return true
  return false
}

function matchEntity(entity, re, isPlain, needle) {
  if (isPlain) {
    if (searchCase) {
      if (testPlain(needle, entity._searchName)) return true
      if (entity._searchWords.some((w) => testPlain(needle, w))) return true
    } else {
      if (testPlainCI(needle, entity._searchName)) return true
      if (entity._searchWords.some((w) => testPlainCI(needle, w))) return true
    }
  } else {
    if (testRe(re, entity._searchName)) return true
    if (entity._searchWords.some((w) => testRe(re, w))) return true
  }
  return false
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
    contentContextFilters = msg.contentContextFilters || []

    const q = query

    // ── Build regex ONCE for this search ───────────────────────────────
    const re = q ? buildSearchRegex(q) : null
    const isPlain = q ? canUsePlainMatch() : false
    // For plain-text mode, prepare the needle string
    const needle = isPlain ? (searchCase ? q : q.toLowerCase()) : null

    // Pre-compute entity names matched by the current query
    matchingEntityNames = new Set()
    if (q && workerEntities.length && (re || isPlain)) {
      for (const entity of workerEntities) {
        if (isPlain) {
          if (
            entity._searchWords.some((w) =>
              searchCase ? testPlain(needle, w) : testPlainCI(needle, w),
            )
          )
            matchingEntityNames.add(entity._nameUpper)
        } else if (re) {
          if (entity._searchWords.some((w) => testRe(re, w)))
            matchingEntityNames.add(entity._nameUpper)
        }
      }
    }

    // Short-circuit: no query and no filter → return everything
    const noQuery = !q
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
        if (hasCtxFilter && !noQuery) {
          if (!re && !isPlain) return false
          return item._kind === "article"
            ? matchArticleCombined(item, re, isPlain, needle)
            : matchDialogCombined(item, re, isPlain, needle)
        }
        if (!matchesContentContext(item)) return false
        if (noQuery) return true
        if (!re && !isPlain) return false
        return item._kind === "article"
          ? matchArticle(item, re, isPlain, needle)
          : matchDialog(item, re, isPlain, needle)
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
        if (hasCtxFilter && !noQuery) {
          if (!re && !isPlain) return false
          return matchArticleCombined(a, re, isPlain, needle)
        }
        if (!matchesContentContext(a)) return false
        if (noQuery) return true
        if (!re && !isPlain) return false
        return matchArticle(a, re, isPlain, needle)
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
        if (hasCtxFilter && !noQuery) {
          if (!re && !isPlain) return false
          return matchDialogCombined(item, re, isPlain, needle)
        }
        if (!matchesContentContext(item)) return false
        if (noQuery) return true
        if (!re && !isPlain) return false
        return matchDialog(item, re, isPlain, needle)
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
        if (!re && !isPlain) return false
        return matchEntity(entity, re, isPlain, needle)
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
