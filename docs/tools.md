# Advanced tools

The wrench in the header opens `#toolsModal`: a list of tools beside the open
one. A new tool is a `.tools-nav-item` and a pane. It has one so far.

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
