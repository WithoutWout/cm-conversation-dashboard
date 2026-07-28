// Pulls named function declarations verbatim out of ../index.html, so the tests
// exercise the real source rather than a copy of it that can drift.
//
// The renderer is one <script> in one HTML file with no module boundary, so
// there is nothing to import — this is the cheapest way to get the collection
// export functions under test without restructuring the app. It is a brace
// matcher that understands strings and comments, nothing more: if it starts
// reporting "unbalanced", the function it names has grown syntax it does not
// model (a regex literal containing an unmatched brace is the usual culprit),
// and the fix is to stub that function in the harness rather than to make this
// smarter.
const fs = require("fs")
const path = require("path")

const src = fs.readFileSync(path.join(__dirname, "..", "index.html"), "utf8")

function extract(name) {
  const marker = "function " + name + "("
  const at = src.indexOf(marker)
  if (at === -1) throw new Error("not found: " + name)
  const start = src.indexOf("{", src.indexOf(")", at))
  let depth = 0
  let inStr = null
  let esc = false
  for (let j = start; j < src.length; j++) {
    const ch = src[j]
    if (inStr) {
      if (esc) {
        esc = false
        continue
      }
      if (ch === "\\") {
        esc = true
        continue
      }
      if (ch === inStr) inStr = null
      continue
    }
    // Comments must be skipped, or an apostrophe in prose ("don't") opens a
    // phantom string and the scan runs past the end of the function.
    if (ch === "/" && src[j + 1] === "/") {
      j = src.indexOf("\n", j)
      if (j === -1) break
      continue
    }
    if (ch === "/" && src[j + 1] === "*") {
      j = src.indexOf("*/", j) + 1
      continue
    }
    if (ch === '"' || ch === "'" || ch === "`") {
      inStr = ch
      continue
    }
    if (ch === "{") depth++
    else if (ch === "}") {
      depth--
      if (depth === 0) return src.slice(at, j + 1)
    }
  }
  throw new Error("unbalanced: " + name)
}

module.exports = { extract }
