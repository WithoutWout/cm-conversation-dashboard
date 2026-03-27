// search-worker.js — Runs all filtering and sorting off the main thread.
// Receives "init" (full dataset) and "search" (query + options) messages.
// Returns "results" with filtered arrays + matchingEntityNames.

"use strict"

// ── Dataset (populated on "init") ────────────────────────────────────────────
let workerArticles = [] // items with _kind === "article"
let workerDialogs = [] // allDialogsCombined (dialogs + tDialogs w/ _kind)
let workerEntities = [] // allEntities

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
  const types = a.Outputs.map((o) => o.Type)
  if (types.includes("Answer")) return "answer"
  if (types.includes("TDialogStart")) return "tdialog"
  return "dialog"
}

function aFaqQ(a) {
  const f = a.Questions.find((q) => q.IsFaq)
  return f ? f.Text : a.Questions[0] ? a.Questions[0].Text : null
}

function aResponse(a) {
  const o = a.Outputs.find((o) => o.Type === "Answer")
  return o ? o.Text : null
}

// ── Entity cross-reference helpers ────────────────────────────────────────────
function entityArticleXrefs(entity) {
  const nameUpper = entity.name.toUpperCase()
  return workerArticles.filter((a) =>
    a.Questions.some((qs) => qs.Text.toUpperCase() === nameUpper),
  )
}

function entityDialogXrefs(entity) {
  const nameUpper = entity.name.toUpperCase()
  return workerDialogs.filter((item) =>
    (item.nodes || []).some((n) =>
      (n.links || []).some((link) => {
        const condData = (link.condition && link.condition.data) || {}
        return (
          !condData.isFallback &&
          (condData.questions || []).some(
            (qo) => qo.text && qo.text.toUpperCase() === nameUpper,
          )
        )
      }),
    ),
  )
}

// ── Match functions ───────────────────────────────────────────────────────────
let matchingEntityNames = new Set()

function matchArticle(a, q) {
  if (!q) return true
  const re = buildSearchRegex(q)
  if (!re) return false
  if (!searchContent && testRe(re, String(a.Id))) return true
  if (!searchContent && a.Questions.some((qs) => testRe(re, qs.Text)))
    return true
  if (testRe(re, strip(aResponse(a) || ""))) return true
  if (
    matchingEntityNames.size > 0 &&
    a.Questions.some((qs) => matchingEntityNames.has(qs.Text.toUpperCase()))
  )
    return true
  return false
}

function matchDialog(item, q) {
  if (!q) return true
  const re = buildSearchRegex(q)
  if (!re) return false
  if (!searchContent && testRe(re, String(item.id))) return true
  if (!searchContent && testRe(re, item.name || "")) return true
  if (!searchContent && testRe(re, item.description || "")) return true
  if (
    (item.nodes || []).some((n) => {
      if (!searchContent && testRe(re, n.name || "")) return true
      const ans = ((n.output && n.output.items) || []).find(
        (i) => i.type === "Answer",
      )
      if (ans && testRe(re, strip((ans.data && ans.data.text) || "")))
        return true
      return false
    })
  )
    return true
  if (
    matchingEntityNames.size > 0 &&
    (item.nodes || []).some((n) =>
      (n.links || []).some((link) => {
        const condData = (link.condition && link.condition.data) || {}
        return (
          !condData.isFallback &&
          (condData.questions || []).some((qo) =>
            qo.text ? matchingEntityNames.has(qo.text.toUpperCase()) : false,
          )
        )
      }),
    )
  )
    return true
  return false
}

function matchEntity(entity, q) {
  if (!q) return true
  const re = buildSearchRegex(q)
  if (!re) return false
  if (testRe(re, entity.name)) return true
  if (entity.words.some((w) => testRe(re, w.text))) return true
  return false
}

// ── Message handler ───────────────────────────────────────────────────────────
self.onmessage = function (e) {
  const msg = e.data

  if (msg.type === "init") {
    workerArticles = msg.articles || []
    workerDialogs = msg.dialogs || []
    workerEntities = msg.entities || []
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

    // Pre-compute entity names matched by the current query
    matchingEntityNames = new Set()
    if (q && workerEntities.length) {
      const re = buildSearchRegex(q)
      if (re) {
        workerEntities.forEach((entity) => {
          if (entity.words.some((w) => testRe(re, w.text)))
            matchingEntityNames.add(entity.name.toUpperCase())
        })
      }
    }

    // ── Filter: All (articles + dialogs combined) ─────────────────────────
    let filteredAll = workerArticles.concat(workerDialogs).filter((item) => {
      if (allFilterPill === "articles" && item._kind !== "article") return false
      if (allFilterPill === "dialogs" && item._kind !== "dialog") return false
      if (allFilterPill === "tdialogs" && item._kind !== "tdialog") return false
      return item._kind === "article"
        ? matchArticle(item, q)
        : matchDialog(item, q)
    })
    filteredAll = sortBy(
      filteredAll,
      allSort,
      (i) => (i._kind === "article" ? i.Id : i.id),
      (i) => (i._kind === "article" ? aFaqQ(i) || "" : i.name || ""),
    )

    // ── Filter: Articles ──────────────────────────────────────────────────
    let filteredArticles = workerArticles.filter((a) => {
      if (aFilter === "answer" && aKind(a) !== "answer") return false
      if (aFilter === "dialog" && aKind(a) === "answer") return false
      return matchArticle(a, q)
    })
    filteredArticles = sortBy(
      filteredArticles,
      aSort,
      (a) => a.Id,
      (a) => aFaqQ(a) || "",
    )

    // ── Filter: Dialogs ───────────────────────────────────────────────────
    let filteredDialogs = workerDialogs.filter((item) => {
      if (dFilter === "dialogs" && item._kind !== "dialog") return false
      if (dFilter === "tdialogs" && item._kind !== "tdialog") return false
      if (dFilter === "recognition" && item._kind === "tdialog") return false
      if (
        dFilter === "recognition" &&
        !(item.nodes || []).some((n) =>
          ((n.output && n.output.items) || []).some((i) => i.type === "Answer"),
        )
      )
        return false
      return matchDialog(item, q)
    })
    filteredDialogs = sortBy(
      filteredDialogs,
      dSort,
      (i) => i.id,
      (i) => i.name || "",
    )

    // ── Filter: Entities ──────────────────────────────────────────────────
    let filteredEntities = workerEntities.filter((entity) => {
      if (eFilter === "articles" && entityArticleXrefs(entity).length === 0)
        return false
      if (eFilter === "dialogs" && entityDialogXrefs(entity).length === 0)
        return false
      return matchEntity(entity, q)
    })
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
