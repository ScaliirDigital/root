// Shared corporate layout, imported by every document type.
//
// This is where composition happens: one place for branding, all document types
// build on it. Note that the pieces are functions taking content, so real layout
// blocks nest inside each other -- no string assembly anywhere.

#let colors = (
  accent: rgb("#1d4ed8"),
  muted: rgb("#6b7280"),
  rule: rgb("#e5e7eb"),
  divider: rgb("#d1d5db"),
  surface: rgb("#f9fafb"),
)

#let footer_column(heading, lines) = {
  text(weight: "bold")[#heading]
  linebreak()
  lines.filter(line => line != none).join(linebreak())
}

#let base(title: none, seller: (:), payment: (:), strings: (:), lang: "de", body) = {
  set page(
    paper: "a4",
    margin: (x: 22mm, top: 24mm, bottom: 42mm),

    footer: context [
      #set text(size: 7.5pt, fill: colors.muted)

      #line(length: 100%, stroke: 0.5pt + colors.rule)
      #v(6pt)

      #grid(
        columns: (1fr, 1fr, 1fr),
        column-gutter: 10pt,
        align: (left, left, left),

        footer_column(
          seller.name,
          (
            seller.address.street,
            seller.address.postcode + " " + seller.address.city,
            if "vat_id" in seller {
              strings.vat_id + " " + seller.vat_id
            },
          ),
        ),

        footer_column(
          strings.contact,
          (
            seller.at("email", default: none),
            seller.at("phone", default: none),
          ),
        ),

        footer_column(
          strings.bank_details,
          (
            payment.at("holder", default: none),
            payment.at("iban", default: none),
            if "bic" in payment { "BIC " + payment.bic },
          ),
        ),
      )

      #v(4pt)

      #align(center)[
        #strings.page #counter(page).display()
        #strings.of #counter(page).final().first()
      ]
    ],
  )

  set text(size: 10pt, lang: lang)

  body
}

// The logo is a bundle asset, not invoice data: the template knows its own
// files, and a path in the data would be a way to reference something outside
// the bundle.
#let letterhead(seller) = {
  image("logo.svg", height: 16mm)
  v(4pt)

  text(size: 8.5pt, fill: colors.muted)[
    #underline(stroke: 0.4pt + colors.muted, offset: 2pt)[
      #seller.name · #seller.address.street · #seller.address.postcode #seller.address.city
    ]
  ]

  v(8pt)
}

#let recipient_block(buyer) = {
  v(10pt)

  text(size: 10pt)[
    #buyer.name
    #linebreak()
    #buyer.address.street
    #linebreak()
    #buyer.address.postcode #buyer.address.city
    #linebreak()
    #buyer.address.country
  ]
}

#let summary(rows) = {
  let pairs = rows
    .chunks(2)
    .map(row => (row.at(0), row.at(1)))

  align(right)[
    #block(
      width: 82mm,
      fill: white,
      radius: 9pt,
      stroke: 0.9pt + rgb("#cbd5e1"),
      clip: true,
    )[
      #table(
        columns: (1fr, auto),
        align: (left, right),
        stroke: none,
        inset: (x: 14pt, y: 9pt),

        ..pairs
          .enumerate()
          .map(((index, row)) => {
            let is_total = index == pairs.len() - 1

            (
              table.cell(
                stroke: if is_total {
                  (top: 0.6pt + colors.divider)
                } else {
                  none
                },
              )[
                #row.at(0)
              ],

              table.cell(
                stroke: if is_total {
                  (top: 0.6pt + colors.divider)
                } else {
                  none
                },
              )[
                #row.at(1)
              ],
            )
          })
          .flatten(),
      )
    ]
  ]
}


// Amounts arrive as decimal strings, never as floats. The host has already
// computed and validated them; parsing to a float here would reintroduce
// exactly the rounding the string representation exists to avoid.
#let money(amount, currency: "EUR") = {
  let parts = str(amount).split(".")
  let whole = parts.at(0)
  let fraction = if parts.len() > 1 {
    (parts.at(1) + "00").slice(0, 2)
  } else {
    "00"
  }

  [#whole,#fraction #currency]
}

// `19` and `19.0` both print as "19".
#let percent(rate) = {
  let value = str(rate)
  if "." in value {
    value.trim("0", at: end).trim(".", at: end)
  } else {
    value
  }
}
