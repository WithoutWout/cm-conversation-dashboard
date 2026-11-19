# Advanced tools

The wrench in the header opens `#toolsModal`: a list of tools beside the open
one. A new tool is a `.tools-nav-item` with a `data-tool` and a pane (`selectTool`).

## Convert IDs to links

`parseIdRefs(text, known)` cuts free text into strings and references. It runs
in the renderer on every keystroke and touches nothing outside the modal.
`known` answers what the loaded export holds; the tests pass a fixture.

**A word in front of a number decides what it is, and the export never overrules
it.** On the real export 181 numbers are both an Article and a Dialog, so the type
cannot be looked up. A prefixed ID always links, marked orange when the export does
not have it, because a newer item than the export is a real case.

| Written as | Links to |
| --- | --- |
| `D-1`, `D#1`, `DN-1`, `DN1`, `dialog(s)`, `dialoog`, `dialogen`, `dlg` | Dialog |
| `Q-1`, `QA-1`, `artikel(s/en)`, `article(s)`, `art`, `faq`, `A-1` | Article |
| `TD-1`, `T-1`, `T-dialoog 1`, `transactional dialog 1`, `transactionele dialoog 1` | Transactional Dialog |
| `1234-123`, `D-1234-123`, `1234 node 123`, `D-1234, node 123` | Dialog node |
| a bare number | Article |

- **A Dialog number that exists only as a Transactional Dialog goes there.** Its
  URL differs, and `dn-` names both.
- **Articles and Transactional Dialogs have no nodes:** on `qa-1234-123` the Article
  is linked and `-123` stays text.
- **`node 123` alone is not linked.** Node ids repeat across Dialogs.
- **Lists carry the type:** `dialogen 1, 2 en 3`, `D-6042, node 11602 en 11603`. An
  inherited number must exist, or `dialog 42, 5 tickets` would link the 5.

**A bare number or `N-N` has to earn it, since free text is full of numbers.** Real
Article ids start at 1, so size cannot separate them.

- It links only if the export has it. What was refused at three digits and up is
  listed as left as text, with the way out: write a prefix.
- One or two digits never link when bare, though 24 Articles have such ids: "3
  tickets" is far commoner than a reference to Article 3. `qa-3` still links.
- With no export loaded nothing can be checked, so three digits and up link, and a
  bare `N-N` needs three in its first half: `10-12` is more likely a date.
- Dates, times, decimals, amounts, percentages, phone numbers, leading zeros, URLs
  and addresses never link. One letter on a small number (`Q4`, `A4`, `D3`) doesn't
  either, without a separator.

**Formatted input.** The input is an editable box, not a textarea, so a paste
from TinyMCE, Teams, Word or a web page keeps its formatting and the result
keeps it too, IDs linked inside it.

- **A paste is cleaned before it lands** (`sanitizeIdLinksHtml`): structure,
  emphasis, lists, tables, images and `http(s)`/`mailto` links stay, with a few
  inline styles (colour, weight, alignment, indent). Scripts, handlers, classes,
  Word's `mso-` and TinyMCE's `data-mce-` go; an unknown tag keeps its content.
  A drop is refused, since it would bypass the cleaning.
- **Typed text stays plain.** The box writes its own `<div>`/`<br>` lines, and
  those are read back as lines (`_idLinksPlainText`) through the old path, `* item`
  lists included. Any formatting (`_idLinksIsRich`) switches to the rich path and
  the label says "formatting kept".
- **The rich path reads all text nodes as one text** (`idRefsInChunks`), so
  `dialog <b>42</b>` still links, then cuts each reference back at the node
  boundaries: a reference split by a tag becomes one `<a>` per piece, with the
  same URL. Block elements and `<br>` break the text into lines, as a newline
  does in plain text. Any whitespace reads as a space (`&nbsp;`, source newlines).
- **A link already in the paste that holds an ID is replaced by the ID's link.**
  Pasted lists are often linked already — to an old URL, another environment —
  and a first version that left them alone found "0 IDs" on exactly that paste.
  A link with no ID in it is kept as it is.
- Copy with links writes the formatted HTML; its plain-text half and Copy as
  Markdown are the text with links, without the formatting.

The modal takes most of the window, input and result side by side, stacked
below 900 px.

**Copying.** Teams, Jira and TinyMCE all paste the clipboard's `text/html`, so
**Copy with links** writes that (`idLinksHtml`), with `* item` lines as a real list,
and Markdown as its `text/plain` half (`idLinksMarkdown`), so a plain-text field
still gets the links. **Copy as Markdown** writes only the Markdown, for a field
that would otherwise take the HTML. A link reads as it was typed, the whole phrase:
`D-4066, node 1340`. Without a CM.com Context URL both are disabled.

## Check links

Finds every web address in the loaded export and asks each one whether it still
answers. It only reads; nothing in CM.com changes.

- **Where addresses come from** (`collectContentUrls`, `urlsInOutput`): every
  Article Answer output and every Dialog node output, walked field by field, so
  `Text`, `Links`, `Images`, `Videos` — and any field a later export adds — are
  covered without naming them. In text it finds bare URLs, Markdown links,
  `<a href>`, `<img src>` and CTA buttons (`{message:cta buttonLink="…"}`, 999 of
  them on the 2026-10-30 export: 753 web links, the rest `eftelingapp://`,
  `mailto:`, `tel:` or empty, which are not web addresses and are not checked);
  `&amp;` and Markdown backslash escapes (`\_`) are undone — CM.com's editor
  escapes underscores inside CTA links too, and read raw the backslash cut 33
  of them short — and trailing punctuation dropped.
  **Unreachable content** (a non-default response with no context, the rule
  `pruneUnreachable` uses) is the tool's own choice, not Settings'. Pruning at
  load keeps what it removed on the owner as a non-enumerable `_pruned`
  (`_setPruned`) — invisible to JSON, the worker's structured clone and every
  other reader — and the collector reads `Outputs`/`items` plus `_pruned`, so it
  sees the same content whatever *Show unreachable content* says. Each use is
  tagged `unreachable`; an address used **only** there is skipped while the
  *Skip unreachable (N)* pill is on (`_urlPool`: counts, list, check, both
  copies). On by default, remembered in `cm-url-skip-unreachable`. Turned off,
  they are listed and their uses drawn dashed; with the Settings toggle off, the
  Article or Dialog a chip opens does not show that pruned response. The
  2026-10-30 export has 74 such addresses, 1469 in all.
  An address is an **image** when it came from an `<img>`, from a field whose key
  says image, or ends in an image extension. Unreachable outputs are already
  pruned at load unless Settings shows them. On the 2026-10-30 export: 1395
  addresses, 414 of them images.
- **The check** is `check_urls(urls)` in Rust, at most 64 URLs a call, eight at
  a time, 15 s each. A plain GET — many servers and CDNs answer HEAD
  differently — with **redirects followed by hand** (`Policy::none`), up to 10,
  so every hop's status comes back in `redirects`: whether a page moved
  (301/308) or is temporarily elsewhere (302/303/307) is in the hops, which an
  automatic policy hides. Of an HTML page the first 32 KB are read for its
  `<title>` (`html_title`); no other body is read. Only absolute http(s) URLs
  are requested (`url_check_target`). The renderer sends batches of 24, so
  progress shows and **Stop** works between batches.
- **The button checks what the filters show** (`_urlCheckTargets`): with a
  filter, search or type narrowing the list it reads *Check N shown* and checks
  exactly those, already-checked ones again — to try a few, or to re-check the
  broken ones. With nothing narrowing it checks everything not yet checked
  (*Check all links* / *Check the rest*), or everything again once all are.
- **Reading a result** (`urlCheckVerdict` → `{cat, why}`), in this order:
  - *Broken* — no answer (timeout, DNS, refused, TLS, redirect loop); any error
    status but 401/403/429; a page answering 200 whose title says not found
    (`_URL_NOT_FOUND_TITLE`: 404, niet gevonden, nicht gefunden…); a redirect
    to a not-found address (`_URL_NOT_FOUND_PATH`); or a redirect to **its own
    site's** home page or language root from a page that was not one
    (`_URL_HOME_PATH`). Same site only: the account pages redirect to
    `login.efteling.com/nl`, whose path also looks like a root.
  - *Restricted* — 401/403/429: an automated check was refused; open by hand.
  - *OK* — answered directly, or a redirect that only adds https / `www.` / a
    trailing slash or drops the query (`_urlSameish`, apps.apple.com's `?mt=8`),
    or **any chain with a temporary hop**: on the 2026-10-30 export every
    account page reaches the login through 307/302 chains, and those work.
  - *Moved* — every hop permanent (301/308) to another page. It works, but the
    link should be updated (Copy → Moved gives old and new address and where
    it is used). "to a broader page" marks a move up the same path
    (`/bus/kaatsheuvel-efteling` → `/bus`), which is often a dead page in
    disguise and worth a look.
  - Tuned on a 192-address sample of the real export: 175 OK (14 through
    temporary redirects), 10 moved (2 to a broader page), 7 broken.
  - `frontend/tests/url-check.test.js` pins each rule with those real answers.
  - Not caught: a soft 404 whose title says nothing, and a permanent move to an
    unrelated page on the same site.
- Each row names the Articles and Dialog nodes using the address; a chip opens
  that item's info modal.
- **Copy ▾** opens a menu: *what* — Problems (Broken + Restricted), Moved, or
  What's shown (the filtered list) — and *as*:
  - **Table with links** — HTML `<table>` with the URLs and each Article / Dialog
    node linked to CM.com (`buildItemUrl` / `buildNodeUrl`, plain ids without a
    Context URL, which the menu says), and Markdown as the `text/plain` half.
    Jira, Teams, TinyMCE and Word paste the HTML as a real table; a plain-text
    field gets the Markdown. The same two-flavour write as Convert IDs to links.
  - **Markdown** (a table, `|` escaped), **Spreadsheet** (TSV, for Excel and
    Sheets), **CSV** (RFC 4180 quoting).
  - All four come from one row list (`urlCheckCopyRows`) through pure
    formatters (`urlCheckTableHtml`, `urlCheckMarkdown`, `urlCheckTsv`,
    `urlCheckCsv`), columns Status · Type · URL · Goes to · Used in, pinned in
    `url-check.test.js`.
- **Answers are kept across restarts** in `localStorage["cm-url-checks"]`, each
  with its `checkedAt`; the summary says when the last check ran (and the
  oldest, when they span more than an hour), and each status chip's tooltip
  when that address was checked. Saved after every batch, so a stopped or
  crashed check keeps what it got. Capped at 6000 answers, newest kept; read
  entry by entry. A re-check replaces answers as they arrive and never clears
  them first, so stopping it loses nothing. It is a cache, not a setting, so it
  is in `SETTINGS_EXPORT_EXCLUDED`. An address that leaves the export keeps its
  answer until the cap pushes it out — harmless, it is never shown.
