# Processor descriptor formats

Processor identities and their researched descriptor formats are separate database
records. Schema 24 stores lean `processor_formats`, linked to existing
`payment_processor` identities. Enrichment matches enabled formats before merchant
retrieval. A unique processor and merchant-suffix capture assigns the processor
and prepares the merchant lookup text. This stage makes no Jev requests.

Schema 25 adds a revision trigger for this table. Runtime matching caches compiled
rules and linked processor identities; catalog revisions invalidate that cache
after rule edits or identity changes, including writes from other connections.

The original descriptor remains available for kind classification, location and
audit. Unknown formats retain the original text. Different processor identities
or different suffix captures matching one descriptor are ambiguous: no processor
is assigned and no text is stripped. Equivalent rules for the same identity and
capture can coexist. Reviewed whole-descriptor processor identity matches remain
supported, but do not strip any text.

## Import and inspect

```sh
cargo run --locked -- database init
cargo run --locked -- processor-formats import data/processor-formats/researched.json --dry-run
cargo run --locked -- processor-formats import data/processor-formats/researched.json
cargo run --locked -- processor-formats list
cargo run --locked -- processor-formats match 'SUMUP*Example Cafe'
```

`database init` upgrades the configured database. The import resolves the exact
`processor` name to one existing payment-processor catalog identity, so research
files do not embed database-specific IDs. Missing or ambiguous names fail rather
than selecting a fuzzy match. Imports are atomic, update by stable rule ID, and
preserve rules absent from the input. Importing a record with `enabled: false`
disables it. `--dry-run` performs the same target, syntax and provenance checks in a
rollback-only transaction.

Each researched entry contains:

- A stable `id` and exact catalog `processor` name.
- A regex with a named `merchant` suffix capture. The engine wraps the entire
  expression in full-input anchors. The capture must be nonempty, follow a
  nonempty prefix, and extend to the end of the original descriptor. Captured
  text is lookup evidence, not a verified merchant identity.
- Source URLs, `reviewed_on`, scope and notes describing the evidence and limits.

The non-backtracking Rust regex engine rejects unsupported constructs. Patterns
are limited to 4096 bytes and compiled representations to 1 MiB. Validation
checks regex syntax, capture presence, size limits, source URLs and required
metadata. Runtime matching rejects empty or non-suffix captures and ambiguous
processor/capture results. Invalid rules are rejected, not silently repaired or
enabled.

Positive and negative regression examples live only in
[`tests/fixtures/processor_formats.json`](../crates/ultrafinance-core/tests/fixtures/processor_formats.json).
The core tests check every researched rule against its fixtures and detect
conflicts on positive cases across the complete rule set. These synthetic cases
test our implementation; official sources supply the evidence for a real
statement format. Fixtures are not imported, stored, or returned by the CLI.

`enrich --details` shows the stored rule ID, pattern, scope, source URLs, extracted
merchant lookup text, and whether ambiguity prevented application. The same
evidence is retained in enrichment history. Descriptor-format sources are
included in attribution when a rule supplies the processor match.

## Research, reviewed 2026-10-19

The initial review covered the 113 identities present before the importer
selection fix. That importer now also accepts payment systems, brands, services,
financial products and online platforms, with shared default-language labels as
a fallback. Square and PayPal were added in a follow-up review. Additional newly
discovered identities require their own format research; the original report is
not an exhaustive review of the expanded catalog.

The import file contains seven rules for processors already in the local
catalog. The [catalog research report](processor-format-research.md) covers all
113 processor identities, including the 110 researched with three GPT-6 Luna
subagents after the initial three rules. The regexes are implementations derived from the cited documentation;
providers do not publish these exact regexes. Case-insensitive spelling and
horizontal whitespace around explicit separators are normalization choices.
Positive examples include synthetic regression cases, clearly not observations
of private transactions.

| Processor | Enabled form | Evidence and limitations |
| --- | --- | --- |
| SumUp | `SUMUP*<merchant>` | [SumUp support](https://help.sumup.com/en-GB/articles/fQAR6AB9o38yT8RkGaM5H-understanding-sumup-charges-on-my-bank-statement) and [SumUp security guidance](https://www.sumup.com/en-us/security/) document the asterisk prefix for card purchases. Debit descriptors can differ. The space-only form `SumUp STARBUCKS` is deliberately not enabled. |
| Airwallex | `AWX*<merchant>` | [Airwallex Merchant of Record documentation](https://www.airwallex.com/docs/payments/merchant-of-record) documents this prefix for its MoR product. This does not cover every Airwallex payment. |
| Xsolla | `XSOLLA*<label>` or `XSOLLA_<label>` | [Xsolla support](https://help.xsolla.com/en/unauthorized-charges/i-see-unknown-charges-for-xsolla) provides examples with both separators. The suffix can be a game/service label and still needs normal merchant identity resolution. Standalone company names and unrelated descriptors are excluded. |
| Square | `SQ *<merchant lookup text>` | [Square developer documentation](https://developer.squareup.com/docs/payments-api/take-payments/card-payments/statement-descriptions) specifies the prefix and optional trailing identifier. The rule preserves the whole suffix, including store identifiers, for subsequent interpretation. |
| PayPal | `PAYPAL *<seller>` | [PayPal support](https://www.paypal.com/us/cshelp/article/how-do-i-update-my-business-name-on-customers-credit-card-statements-help336) documents the seller statement name after this prefix. Bank-transfer `PAYPALINST XFER` and ambiguous `PP*` are excluded. |
| WePay | `WPY*<merchant>` | [WePay support](https://support.wepay.com/hc/en-us/articles/203609683-Unrecognized-Charge) explicitly documents this prefix for payments through supported merchants or partners. This does not cover every configurable WePay soft descriptor. |
| Zettle | `PayPal Point of Sale *<company>` | [PayPal Point of Sale / Zettle support](https://www.zettle.com/gb/help/articles/1084729-taking-card-and-contactless-payments) documents this customer-statement form for card-reader and terminal payments. Generic `PP*` cannot identify Zettle; legacy `IZ *` and `IZETTLE*` remain unconfirmed in this research pass. |

No universal Stripe rule was enabled: [Stripe's documentation](https://docs.stripe.com/get-started/account/statement-descriptors)
allows merchant-configured prefixes, so a generic `PREFIX*SUFFIX` is not evidence
that Stripe processed a payment. [Adyen's documentation](https://docs.adyen.com/account/transaction-description/)
also supports configurable transaction descriptions; no universal extraction rule
was established. Venmo searches did not establish a sufficiently specific
merchant-extracting format. Shift4's generic `PF*` notation was rejected, and
Paysafe's illustrative `NBX*` example remains an unconfirmed lead. These are
research limits, not claims that those processors have no recognizable formats.

The legacy `interpret --list-formats` prefix inventory is not the source of truth
for processor assignment. `processor-formats list` shows the DB-backed rules used
by enrichment. New formats require research and import, not new Rust branches.
