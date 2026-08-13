// Entrypoint. Composition and conditionals happen HERE, in typst -- not by
// assembling strings in Rust. That is the whole point of the engine choice:
// the data stays data and never becomes syntax.

#import "brand.typ": base, letterhead, recipient_block, summary, money, percent, colors

// Two sources, two lifetimes.
//
// `fixture.json` is part of the template: the issuer's own details, fixed at
// publish time and covered by the content hash. Moving offices means a new
// version, which is what reproducing an old invoice requires.
//
// The request is per document, injected by the host as an in-memory virtual
// file. No fallback on purpose: missing or malformed data must fail here.
//
// The path is relative on purpose: an absolute one resolves against the Typst
// project root, which is the bundle on the server but the surrounding
// directory in an editor. The cost is that the entrypoint has to sit at the
// bundle root -- from a subdirectory, `__data/` would be looked for there.
#let issuer = json("fixture.json")
#let request = json("__data/request.json")
#let data = request.data

#if data == none {
  panic("render data is required")
}

// The XML half of a hybrid invoice, when one is sent. The XMP metadata that
// declares what this attachment means is added after the render -- Typst
// cannot write it.
#if request.at("has_xml", default: false) {
  pdf.attach(
    "factur-x.xml",
    read("/__data/factur-x.xml", encoding: none),
    relationship: "alternative",
    mime-type: "text/xml",
    description: "Factur-X invoice data",
  )
}


// ---------------------------------------------------------------------------
// Locale
// ---------------------------------------------------------------------------

#let lang = request.at("lang", default: "de")

#assert(
  lang in ("de", "en"),
  message: "unsupported language `" + lang + "`",
)

#let strings = json(lang + ".json")

// ---------------------------------------------------------------------------
// Shape
//
// The host validates against the canonical invoice schema before this runs,
// so these are a second line of defence, not the first one.
// ---------------------------------------------------------------------------

#let require(dict, keys, where: "root") = {
  for key in keys {
    assert(key in dict, message: "missing field `" + key + "` in " + where)
  }
}

#require(issuer, ("seller",), where: "fixture")
#require(data, ("number", "issued", "currency", "buyer", "lines", "totals"))
#assert(data.lines.len() > 0, message: "invoice must have at least one line")

// ---------------------------------------------------------------------------
// Presentation
// ---------------------------------------------------------------------------

#let currency = data.currency

// The data carries ISO dates; the document shows the local form.
#let show_date(iso) = {
  let parts = iso.split("-")
  parts.at(2) + "." + parts.at(1) + "." + parts.at(0)
}

// UNTDID 5305 `AE`: the buyer owes the VAT, so the invoice charges none.
#let reverse_charge = data.totals.breakdown.any(entry => entry.category == "AE")

// ---------------------------------------------------------------------------

#show: base.with(
  title: strings.invoice + " " + data.number,
  seller: issuer.seller,
  payment: issuer.at("payment", default: (:)),
  strings: strings,
  lang: lang,
)

#letterhead(issuer.seller)
#recipient_block(data.buyer)

#v(16pt)

#grid(
  columns: (1fr, auto),
  align: (left, right),
  text(size: 14pt, weight: "bold")[
    #strings.invoice #data.number
  ],
  text(fill: colors.muted)[#show_date(data.issued)],
)

#v(12pt)

#block(
  width: 100%,
  fill: white,
  radius: 9pt,
  stroke: 0.9pt + rgb("#cbd5e1"),
  clip: true,
)[
  #table(
    columns: (1fr, auto, auto, auto),
    align: (left, right, right, right),
    stroke: (x: none, y: 0.6pt + rgb("#d1d5db")),
    inset: (x: 12pt, y: 11pt),

    table.header(
      table.cell(fill: colors.accent.lighten(86%), inset: (x: 12pt, y: 10pt))[*#strings.position*],
      table.cell(fill: colors.accent.lighten(86%), inset: (x: 12pt, y: 10pt))[*#strings.quantity*],
      table.cell(fill: colors.accent.lighten(86%), inset: (x: 12pt, y: 10pt))[*#strings.unit_price*],
      table.cell(fill: colors.accent.lighten(86%), inset: (x: 12pt, y: 10pt))[*#strings.amount*],
    ),

    ..data.lines
      .map(line => (
        table.cell(fill: white)[#line.name],
        table.cell(fill: white)[#line.quantity],
        table.cell(fill: white)[#money(line.unit_price, currency: currency)],
        table.cell(fill: white)[#money(line.net, currency: currency)],
      ))
      .flatten(),
  )
]

#v(8pt)

// Totals come from the host, never from a sum computed here: a second
// calculation is a second source of truth, and it would be the one that
// disagrees with the embedded XML.
#summary((
  [#strings.net], money(data.totals.net, currency: currency),

  ..data.totals.breakdown
    .map(entry => (
      if entry.category == "AE" {
        [#strings.vat]
      } else {
        [#strings.vat #percent(entry.rate) %]
      },
      if entry.category == "AE" {
        [--]
      } else {
        money(entry.tax, currency: currency)
      },
    ))
    .flatten(),

  text(weight: "bold")[#strings.total],
  text(weight: "bold", size: 11pt)[#money(data.totals.gross, currency: currency)],
))

#if reverse_charge [
  #v(14pt)

  #text(size: 9pt, fill: colors.muted)[
    #strings.reverse_charge
    #linebreak()
    #strings.recipient_vat_id: #data.buyer.vat_id
  ]
]

#if "terms" in data [
  #v(14pt)
  #text(size: 9pt)[#data.terms]
]

#if "note" in data [
  #v(6pt)
  #text(size: 9pt)[#data.note]
]
