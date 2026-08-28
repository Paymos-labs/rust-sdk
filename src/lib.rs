//! Official async Rust SDK for the Paymos Merchant API.
//!
//! Amounts are always dot-decimal strings. API secrets belong only on trusted
//! servers. The client signs every request and provides a separate raw-body
//! verifier for webhook signatures.

mod client;
mod error;
mod models;
pub mod signing;
mod version;
mod webhook;

pub use client::{
    Balances, ClientBuilder, InvoicePager, Invoices, PaymentChannelDeposits, PaymentChannelPager,
    PaymentChannels, PaymosClient, System, WithdrawalPager, Withdrawals,
};
pub use error::{ApiError, ApiErrorKind, Error, ProblemError};
pub use models::{
    Balance, CreateInvoiceRequest, CreatePaymentChannelRequest, CreateWithdrawalRequest,
    CursorPage, Invoice, InvoiceListItem, InvoiceListParams, InvoiceSimulationStage, InvoiceStatus,
    Order, Payment, PaymentChannel, PaymentChannelDeposit, PaymentChannelDepositFeedBlockage,
    PaymentChannelDepositFeedPage, PaymentChannelDepositFeedParams, PaymentChannelListParams,
    PaymentChannelNetwork, PaymentChannelToken, ServerTime, SimulateDepositRequest,
    SimulateDepositStage, Transfer, Withdrawal, WithdrawalListParams, WithdrawalStatus,
};
pub use version::SDK_VERSION;
pub use webhook::{WebhookError, WebhookEvent, WebhookVerifier};
