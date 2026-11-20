# Paymos Rust SDK

Official async Rust client for the [Paymos Merchant API](https://paymos.io/docs).
It provides typed invoices, withdrawals, refunds, balances, static per-customer wallets and
their confirmed-deposit feed, cursor pagination, structured errors, automatic request signing,
safe retries, and raw-body webhook verification.

## Requirements

- Rust 1.86 or newer;
- Tokio runtime;
- a server-side Paymos Payment or Payout API key.

Never expose an API secret in browser JavaScript, a mobile binary, a URL, or a log.

## Installation

```bash
cargo add paymos
```

## Create an invoice

```rust,no_run
use paymos::{CreateInvoiceRequest, InvoiceProduct, PaymosClient};

#[tokio::main]
async fn main() -> Result<(), paymos::Error> {
    let paymos = PaymosClient::new(
        std::env::var("PAYMOS_API_KEY").expect("PAYMOS_API_KEY is required"),
        std::env::var("PAYMOS_API_SECRET").expect("PAYMOS_API_SECRET is required"),
    )?;

    // Compatible amount-only call (products omitted on wire via None)
    let invoice = paymos
        .invoices()
        .create(&CreateInvoiceRequest {
            project_id: "prj_xxxxxxxxxxxx".to_owned(),
            amount: "49.95".to_owned(),
            currency: "USD".to_owned(),
            external_order_id: "order_123".to_owned(),
            network: None,
            customer_fee_percent: None,
            client_id: None,
            products: None,
        })
        .await?;

    println!("{}", invoice.payment_url);
    Ok(())
}
```

Money is always passed and returned as a dot-decimal `String`; do not convert it
through binary floating point.

### Product line items (`products[]`)

Pass `products: Some(vec![...])` (`1..=50` `paymos::InvoiceProduct` items) to display purchase context on hosted checkout, or `products: None` for a compatible `amount-only` invoice. Each `InvoiceProduct` requires `title` (<= 200 Unicode scalar values), positive `u32` `quantity` (`1..=2_147_483_647`), and decimal `String` `unit_amount` (in the invoice currency); optional `description` (plain text, <= 2000 scalars) is an `Option<String>` and is omitted on the wire when `None`.

```rust,no_run
# use paymos::{CreateInvoiceRequest, InvoiceProduct, PaymosClient};
# async fn example(paymos: PaymosClient) -> Result<(), paymos::Error> {
// 1-item invoice with optional description
let studio_pro = InvoiceProduct {
    title: "Studio Pro".to_owned(),
    description: Some("One-month access".to_owned()),
    quantity: 2,
    unit_amount: "24.50".to_owned(),
};

let single_invoice = paymos
    .invoices()
    .create(&CreateInvoiceRequest {
        project_id: "prj_xxxxxxxxxxxx".to_owned(),
        amount: "47.00".to_owned(),
        currency: "USD".to_owned(),
        external_order_id: "order_product_single".to_owned(),
        network: None,
        customer_fee_percent: None,
        client_id: None,
        products: Some(vec![studio_pro.clone()]),
    })
    .await?;

// Multi-item cart including a minimal 3-field item on the wire (description: None)
let cart_invoice = paymos
    .invoices()
    .create(&CreateInvoiceRequest {
        project_id: "prj_xxxxxxxxxxxx".to_owned(),
        amount: "59.00".to_owned(),
        currency: "USD".to_owned(),
        external_order_id: "order_product_multi".to_owned(),
        network: None,
        customer_fee_percent: None,
        client_id: None,
        products: Some(vec![
            studio_pro,
            InvoiceProduct {
                title: "Priority Support".to_owned(),
                description: None,
                quantity: 1,
                unit_amount: "12.00".to_owned(),
            },
        ]),
    })
    .await?;

if let Some(items) = &cart_invoice.order.products {
    for item in items {
        println!("{} x{} @ {}", item.title, item.quantity, item.unit_amount);
    }
}
# let _ = single_invoice;
# Ok(())
# }
```

- **Authoritative `amount`:** `amount` is the exact order total and is never recomputed from `sum(quantity * unit_amount)`.
- **First-success replay:** the first successful create freezes `order.products` on the invoice. Replaying the same `external_order_id` with unchanged `(amount, currency, network)` returns HTTP 200 with the original `Invoice` and `order.products`, even if `products` changed; changing `(amount, currency, network)` returns `Error::Api` with HTTP `409` (`invoice_idempotency_conflict`). Legacy and `amount-only` invoices return `order.products == None`.
- **Webhooks:** signed invoice webhooks omit `products`.

### Upgrading from `1.x` to `2.0.0` (breaking change)

`paymos` `2.0.0` (`SDK_VERSION = "2.0.0"`) adds `pub products: Option<Vec<InvoiceProduct>>` to `CreateInvoiceRequest` and `Order`. Because `CreateInvoiceRequest` and `Order` have public fields and do not implement `Default`, this is a breaking source change for struct literals and exhaustive struct patterns:

- Add `products: None` to existing `CreateInvoiceRequest { ... }` struct literals to keep the exact compatible `amount-only` request behavior.
- Add `products` (or `..`) when constructing or pattern-matching `paymos::Order`.
- Existing `1.x` clients remain wire-compatible with the server for `amount-only` invoices.

## Cursor pagination

```rust,no_run
# use paymos::{InvoiceListParams, InvoiceStatus, PaymosClient};
# async fn example(paymos: PaymosClient) -> Result<(), paymos::Error> {
let mut pager = paymos.invoices().pager(
    InvoiceListParams {
        status: Some(vec![InvoiceStatus::Paid, InvoiceStatus::PaidOver]),
        ..InvoiceListParams::default()
    },
    None, // the safe default is 100 pages
)?;

while let Some(invoice) = pager.next().await? {
    println!("{}", invoice.invoice_id);
}
# Ok(())
# }
```

The pager stops at its configured page bound and rejects any cursor returned more
than once.

## Invoice refunds

```rust,no_run
# use paymos::{PaymosClient, RefundListParams, RefundMode};
# async fn example(paymos: PaymosClient) -> Result<(), paymos::Error> {
let refund = paymos
    .refunds()
    .create("inv_xxxxxxxxxxxx", RefundMode::Overpayment, "refund-order-123")
    .await?;
println!("{} {}", refund.withdrawal_id(), refund.amount());

let current = paymos.refunds().get(refund.withdrawal_id()).await?;
let page = paymos.refunds().list(&RefundListParams {
    invoice_id: Some(current.invoice_id().to_owned()),
    created_from: Some("2026-10-01T00:00:00Z".to_owned()),
    limit: Some(100),
    ..RefundListParams::default()
}).await?;
// Pass page.next_cursor as cursor to read the next page with the same filters.
# let _ = page;
# Ok(())
# }
```

Refunds require a Payment key. `Full` returns the remaining payment;
`Overpayment` returns the remaining excess. The server calculates the amount and
selects the first deposit sender as recipient. A payment of 120 against an order
of 100 returns 20 in overpayment mode, or 120 in full mode. After the overpayment
completes, a new full refund returns 100.

Create sends only `invoice_id`, `mode`, and `external_order_id`. Keep the same key
and mode after an unknown outcome: a new request returns 201, a replay returns
200 with the existing refund, and changing the mode for that key returns a
structured 409 error. Refund responses expose read-only accessors and preserve
amounts as decimal strings. List supports `invoice_id`, repeated `status`, RFC
3339 `created_from`/`created_to`, `cursor`, and `limit` filters in the credential's
project and environment scope. For sandbox completion, call
`paymos.refunds().simulate_completion(refund.withdrawal_id())`.

## Payment channels

A channel is one payer's reusable set of deposit addresses. Create it, read its
rails, then poll the confirmed-deposit feed and persist the cursor it returns.

```rust,no_run
# use paymos::{CreatePaymentChannelRequest, PaymentChannelDeposit, PaymentChannelDepositFeedParams, PaymosClient};
# fn credit_deposit(_deposit: &PaymentChannelDeposit) {}
# async fn example(paymos: PaymosClient, saved_cursor: Option<String>) -> Result<(), paymos::Error> {
let channel = paymos
    .payment_channels()
    .create(&CreatePaymentChannelRequest {
        project_id: "prj_xxxxxxxxxxxx".to_owned(),
        external_id: "customer_42".to_owned(),
    })
    .await?;

for rail in &channel.networks {
    println!("{} {} {:?}", rail.network, rail.status, rail.address);
}

let feed = paymos
    .payment_channel_deposits()
    .read(&PaymentChannelDepositFeedParams {
        cursor: saved_cursor,
        limit: Some(100),
        ..PaymentChannelDepositFeedParams::default()
    })
    .await?;

for deposit in &feed.items {
    credit_deposit(deposit); // one credit per deposit.id, shared with the webhook
}

// Persist this and pass it back as `cursor` on the next poll.
let next_cursor: String = feed.next_cursor;
# let _ = next_cursor;
# Ok(())
# }
```

Four things that bite an integrator who guesses:

- Repeating the same `external_id` returns the **same** channel — `200` instead
  of `201`, and both bodies are a channel. Call it on every checkout; a repeat
  is not a duplicate and not an error.
- A rail's `address` is `None` **until that rail finishes provisioning**, and
  never changes once it appears. `None` means "not yet", not "no address" —
  offer the payer only the rails that already have one.
- `minimum_deposit: None` means "we cannot quote a minimum right now", not
  "there is no minimum". Treating `None` as zero is how a merchant accepts a
  deposit that lands below the live minimum and is never credited.
- `next_cursor` is a plain `String` and is **never** empty, not even on a page
  with no items. Store it and resume from it. Do not loop until it is empty the
  way the invoice pager ends: that loop either spins forever or, worse, stops on
  the first quiet page and leaves the merchant's reconciliation silently behind.
  `confirmed_from` is only the first poll's lower bound — after that the stored
  cursor is the resume mechanism.

## 401 and 403 are different problems

A **401** means the credential is absent, malformed or unverifiable — fix the key or the signing
clock, and stop retrying.

A **403** means the credential was accepted and something else refused. It covers two very
different situations:

- **the key is wrong for this call** — `payout_key_required`, `payment_key_required`,
  `forbidden`;
- **the key is fine, the account or project state says no** — `whitelist_required`,
  `merchant_suspended`, `widget_inactive`, `terminal_not_enabled`, `not_sandbox`.

A customer entering a payout address that is not whitelisted yet is the second kind: nothing is
wrong with your credentials, and paging the on-call engineer about them is the wrong move. Always
switch on the error `code` before deciding what to do.

`ApiErrorKind::Permission` is a separate variant from `ApiErrorKind::Authentication`. The enum is
`#[non_exhaustive]`, so adding it does not break an existing `match`.

## Errors and retries

```rust,no_run
# use paymos::{ApiErrorKind, Error, InvoiceListParams, PaymosClient};
# async fn example(paymos: PaymosClient) {
match paymos.invoices().list(&InvoiceListParams::default()).await {
    Ok(page) => println!("{} invoices", page.items.len()),
    Err(Error::Api(error)) if error.kind == ApiErrorKind::RateLimit => {
        eprintln!("rate limited; retry-after = {:?}", error.retry_after);
    }
    Err(error) => eprintln!("{error}"),
}
# }
```

The client retries transport errors and 5xx responses only for idempotent methods.
HTTP 429 may also retry a POST because the API rejected it before processing.
`Retry-After` is honored. A mutating request is never repeated after an ambiguous
transport or generic server failure.

## Verify a webhook

```rust,no_run
# use paymos::{Invoice, WebhookEvent, WebhookVerifier};
# fn example(signature: &str, raw_body: &[u8]) -> Result<(), paymos::WebhookError> {
let verifier = WebhookVerifier::new(
    std::env::var("PAYMOS_WEBHOOK_SECRET").expect("PAYMOS_WEBHOOK_SECRET is required"),
)?;

let event: WebhookEvent<Invoice> = verifier.construct_event(signature, raw_body)?;
println!("{} {}", event.event_id, event.event_type);
# Ok(())
# }
```

Pass the exact request bytes before JSON parsing. API request signatures use
base64 HMAC-SHA256 over the canonical request; webhook signatures use lowercase
hex HMAC-SHA256 over `{timestamp}.{raw_body}`. They are intentionally different.

### Payment-channel deposit events

The three `payment_channel.deposit.*` events each carry a full `PaymentChannelDeposit`
as their payload, so pass it as the type parameter:

```rust,no_run
# use paymos::{PaymentChannelDeposit, WebhookEvent, WebhookVerifier};
# fn credit_deposit(_deposit: &PaymentChannelDeposit) {}
# fn example(signature: &str, raw_body: &[u8]) -> Result<(), paymos::WebhookError> {
# let verifier = WebhookVerifier::new("whsec_x")?;
let event: WebhookEvent<PaymentChannelDeposit> = verifier.construct_event(signature, raw_body)?;

if event.event_type == "payment_channel.deposit.confirmed" && event.data.is_final {
    credit_deposit(&event.data); // once per event.data.id, shared with the feed
}
# Ok(())
# }
```

The feed and the deposit webhook report the same deposits, so both go through one
`credit_deposit` keyed by the deposit's own `id` (`pcd_...`). Check `project_id` and
`is_test` against your configuration, then in one database transaction insert `id` into a
table where it is the primary key (`INSERT ... ON CONFLICT (deposit_id) DO NOTHING` in
PostgreSQL) and add `net` to the payer's balance only when that insert added a row. A
second delivery, a second endpoint's copy, or a re-read feed page then inserts nothing.

`event.event_id` identifies one delivery, not one payment. Paymos creates a separate
event, with its own `event_id`, for every webhook endpoint subscribed to the event, and
every later event about the same invoice or deposit has its own `event_id` too. Store
`event_id` to skip a redelivered request, but key money and fulfilment on the business
identity: the invoice/order for invoice events, the deposit `id` for deposit events.

**`confirming` and `reorged` are advisory and may arrive out of order.** A `confirming`
can land after the `confirmed` for the same deposit, and a `reorged` can be superseded by
a later `confirmed`. Credit only on `payment_channel.deposit.confirmed` with `is_final`
true, and never let an advisory event regress a deposit you already know is confirmed.
Crediting on `confirming` releases goods against money a reorg can still take back.

## Release integrity

Every release is built from an immutable `vMAJOR.MINOR.PATCH` tag in
[`Paymos-labs/rust-sdk`](https://github.com/Paymos-labs/rust-sdk). The crate version
and `paymos-rust/<version>` user agent are stamped from the same release plan.
The full language-neutral conformance contract is shipped in
`conformance/contract.json` and executed by the test suite.

- Documentation: [paymos.io/docs/server-sdks](https://paymos.io/docs/server-sdks)
- API reference: [docs.rs/paymos](https://docs.rs/paymos)
- Security reports: [security@paymos.io](mailto:security@paymos.io)
