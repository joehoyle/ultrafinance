# Descriptor format research

The supported-format inventory comes from the executable parser definitions:

```sh
cargo run --locked -- interpret --list-formats
cargo run --locked -- interpret --list-formats --json
```

The JSON output includes exact patterns and the active processor-prefix table.
Both commands run without database access or provider calls. Preview a specific
bank description with `ultrafinance interpret 'DESCRIPTION'`.

The sources below informed the bank-format work on 2026-10-09. They are research
provenance; use the CLI for current supported formats and parsing behavior.

Sources:

- [Square statement examples](https://my.squareup.com/help/ca/en/article/5103-square-purchases-and-cardholder-help)
- [PayPal statement phone number](https://securepayments.paypal.com/us/cshelp/article/why-is-the-number-402-935-7733-showing-on-my-bank-or-credit-card-statement-help594)
- [Stripe statement descriptors](https://docs.stripe.com/get-started/account/statement-descriptors)
- [Adyen dynamic descriptions and localization](https://docs.adyen.com/account/transaction-description)
- [Adyen default name/city/country description](https://help.adyen.com/knowledge/account/transaction-descriptions-and-notifications/what-is-the-transaction-description)
- [Paddle statement formats](https://www.paddle.com/help/manage/your-customers/what-will-customers-see-on-their-statement)
- [Nacha ACH fields](https://achdevguide.nacha.org/ach-file-details)
- [Public municipal ACH presentation examples](https://cityofsandyoaks.com/wp-content/uploads/2024/12/StatementofActivityDetail-5.pdf)
- [Wise transfer references and UTR terminology](https://wise.com/help/articles/2977938/whats-a-banking-partner-reference-number)

## Further research

Further formats worth collecting real failures for include international phone
numbers, bank-specific compact dates, wire beneficiary/intermediary fields,
SEPA creditor/mandate/end-to-end references, UK payment-rail abbreviations,
Interac/e-transfer messages, and FX/ATM fee or reversal narratives. These need
context about payment direction and roles: a sender, beneficiary, intermediary
bank, mandate reference or free-form memo is not automatically a merchant.
The current parser does not claim general support for these families, and it
does not parse fixed-width Nacha files. Merchant websites, emails and arbitrary
asterisk suffixes are retained because they can be useful identity evidence.
