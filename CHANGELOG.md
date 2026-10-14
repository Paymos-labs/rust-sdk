# Changelog

All notable changes to the official Paymos Rust SDK are documented here.

## [Unreleased]

## [1.3.0] - 2026-10-14

- Make the SDK CI tier green
- Fix invoice-late-payment audit findings (tasks 4.1-4.8)
- feat(invoices): accept full late payments without reopening invoices

- **403 is no longer reported as an authentication failure.** `ApiErrorKind::Permission` is a new variant beside `Authentication`. The enum is `#[non_exhaustive]`, so an existing `match` still compiles.
- **Base URL must use HTTPS.** Plain HTTP is accepted only against a loopback host, which is what a local integration test points at. The API secret signs every request, so a plaintext base URL put that credential on the wire.

## [1.2.1] - 2026-09-13

- fix(sdk-conformance): восемь копий контракта разошлись с каноном — 403 снова permission
- fix(sdk): HTTPS обязателен, а 403 перестал притворяться ошибкой ключа

- **403 is no longer reported as an authentication failure.** `ApiErrorKind::Permission` is a new variant beside `Authentication`. The enum is `#[non_exhaustive]`, so an existing `match` still compiles.
- **Base URL must use HTTPS.** Plain HTTP is accepted only against a loopback host, which is what a local integration test points at. The API secret signs every request, so a plaintext base URL put that credential on the wire.

## [1.2.0] - 2026-08-28

- release: the changelog rot had a cause, and it was not the one I named
- docs(sdk): the READMEs say static wallets, not payment channels
- feat(sdk): payment-channel webhooks and the flow in every README
- feat(rust-sdk): payment channels, and a feed reader that cannot stop early
- feat(sdk-conformance): declare the payment-channel operations
- docs(api): pin the empty-body signing rule and fix the published POST vector

## [1.1.0] - 2026-08-02

- fix(ecosystem): recover SDK releases
- feat(docs): make the developer surface consumable by LLM agents
- fix: align SDK problem details semantics

## [1.0.0] - 2026-07-12

- Initial async Merchant API client.
- Complete invoice, withdrawal, balance, and server-time resource surface.
- Byte-exact HMAC signing, bounded cursor pagination, safe retries, structured errors, and webhook verification.

