# Ultrafinance

Ultrafinance allows power users to set up automations and scripting for your banking transactions and events. For example, call webhooks when transactions are made, send an email on specific events, or even write custom functions in TypeScript to build complex automations and integrations.

## Technical Architecture

Ultrafinance is built as a decoupled web app, with the backend written in Rust and the client side using React. Data is stored in a MySQL database. Ultrafinance also comes with a CLI application.

## Todo

### Server

- [ ] Update function
- [ ] Update user
- [ ] Auth sessions
- [ ] Delete account
- [ ] Filter transactions endpoint
- [ ] Filter trigger logs endpoints
- [ ] Cron / auto process queue
- [ ] Auto import transactions
- [ ] Delete transaction
- [ ] Transaction data enrichment
- [ ] Test run function
- [ ] Account balances + further metadata
- Inbuilt triggers:
  - Email
  - Webhook
  - SMS
- Events:
  - [x] `created_transaction`
  - [ ] `creating_transaction`
  - [ ] `account_balance_updated`

### Client

- [ ] Authentication
- [ ] Microcopy / NUX
- [ ] Manually run trigger queue
- [ ] Create trigger
- [ ] Create function
- [ ] Select & name accounts on link institution
