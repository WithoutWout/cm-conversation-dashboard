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

// ── Search options (updated on each "search" message) ────────────────────────
let searchCase = false
let searchWord = false
let searchRegex = false
let searchContent = true

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

function strip(t) {
  return (t || "")
    .replace(/\{[^}]*\}/g, "")
    .replace(/%\{[^}]*\}/g, "")
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

  // Pre-compute search fields
  a._searchId = String(a.Id)
  a._searchResponse = strip(a._response || "")
  a._searchQuestionsUpper = a.Questions.map((qs) => qs.Text.toUpperCase())
}

function precomputeDialog(item) {
  item._searchId = String(item.id)
  item._searchName = item.name || ""
  item._searchDesc = item.description || ""

  // Pre-compute per-node search data
  const nodes = item.nodes || []
  item._searchNodes = nodes.map((n) => {
    const ans = ((n.output && n.output.items) || []).find(
      (i) => i.type === "Answer",
    )
    return {
      name: n.name || "",
      answerText: ans ? strip((ans.data && ans.data.text) || "") : "",
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
        ? testPlain(needle, a._searchResponse)
        : testPlainCI(needle, a._searchResponse)
    )
      return true
  } else {
    if (testRe(re, a._searchResponse)) return true
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
    if (sn.answerText) {
      if (isPlain) {
        if (
          searchCase
            ? testPlain(needle, sn.answerText)
            : testPlainCI(needle, sn.answerText)
        )
          return true
      } else {
        if (testRe(re, sn.answerText)) return true
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
    workerArticles = msg.articles || []
    workerDialogs = msg.dialogs || []
    workerEntities = msg.entities || []

    // Pre-compute searchable fields once on data load
    for (const a of workerArticles) precomputeArticle(a)
    for (const d of workerDialogs) precomputeDialog(d)
    for (const ent of workerEntities) precomputeEntity(ent)

    // Pre-build combined array (avoids concat on every search)
    allItems = workerArticles.concat(workerDialogs)

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

    // ── Filter: All (articles + dialogs combined) ─────────────────────────
    let filteredAll
    if (noQuery && allFilterPill === "all") {
      filteredAll = allItems
    } else {
      filteredAll = allItems.filter((item) => {
        if (allFilterPill === "articles" && item._kind !== "article")
          return false
        if (allFilterPill === "dialogs" && item._kind !== "dialog") return false
        if (allFilterPill === "tdialogs" && item._kind !== "tdialog")
          return false
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
    if (noQuery && aFilter === "all") {
      filteredArticles = workerArticles
    } else {
      filteredArticles = workerArticles.filter((a) => {
        if (aFilter === "answer" && aKind(a) !== "answer") return false
        if (aFilter === "dialog" && aKind(a) === "answer") return false
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
    if (noQuery && dFilter === "all") {
      filteredDialogs = workerDialogs
    } else {
      filteredDialogs = workerDialogs.filter((item) => {
        if (dFilter === "dialogs" && item._kind !== "dialog") return false
        if (dFilter === "tdialogs" && item._kind !== "tdialog") return false
        if (dFilter === "recognition" && item._kind === "tdialog") return false
        if (dFilter === "recognition" && !item._hasAnswerOutput) return false
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

    self.postMessage({
      type: "results",
      id,
      filteredAll,
      filteredArticles,
      filteredDialogs,
      filteredEntities,
      matchingEntityNames: Array.from(matchingEntityNames),
    })
  }
}
