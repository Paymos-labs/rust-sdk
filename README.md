# Paymos Rust SDK

Official async Rust client for the [Paymos Merchant API](https://paymos.io/docs).
It provides typed invoices, withdrawals, balances, cursor pagination, structured
errors, automatic request signing, safe retries, and raw-body webhook verification.

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
use paymos::{CreateInvoiceRequest, PaymosClient};

#[tokio::main]
async fn main() -> Result<(), paymos::Error> {
    let paymos = PaymosClient::new(
        std::env::var("PAYMOS_API_KEY").expect("PAYMOS_API_KEY is required"),
        std::env::var("PAYMOS_API_SECRET").expect("PAYMOS_API_SECRET is required"),
    )?;

    let invoice = paymos
        .invoices()
        .create(&CreateInvoiceRequest {
            project_id: "prj_xxxxxxxxxxxx".to_owned(),
            amount: "49.95".to_owned(),
            currency: "USD".to_owned(),
            external_order_id: "order_123".to_owned(),
            network: None,
            allow_multiple_payments: None,
            customer_fee_percent: None,
            client_id: None,
        })
        .await?;

    println!("{}", invoice.payment_url);
    Ok(())
}
```

Money is always passed and returned as a dot-decimal `String`; do not convert it
through binary floating point.

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

## Release integrity

Every release is built from an immutable `vMAJOR.MINOR.PATCH` tag in
[`Paymos-labs/rust-sdk`](https://github.com/Paymos-labs/rust-sdk). The crate version
and `paymos-rust/<version>` user agent are stamped from the same release plan.
The full language-neutral conformance contract is shipped in
`conformance/contract.json` and executed by the test suite.

- Documentation: [paymos.io/docs/server-sdks](https://paymos.io/docs/server-sdks)
- API reference: [docs.rs/paymos](https://docs.rs/paymos)
- Security reports: [security@paymos.io](mailto:security@paymos.io)
