// Render data is injected by the host as an in-memory virtual file. No
// fallback on purpose: missing or malformed data must fail here.
//
// The path is relative because an absolute one resolves against the Typst
// project root, which is the bundle on the server but the surrounding
// directory in an editor.
#let request = json("__data/request.json")
#let data = request.data

#set page(paper: "a4", margin: 3cm)
#set text(font: "Roboto", size: 12pt)

#align(center + horizon)[
  #text(size: 28pt, weight: "bold")[#data.title]

  #v(1em)

  #text(size: 14pt)[#data.message]
]
