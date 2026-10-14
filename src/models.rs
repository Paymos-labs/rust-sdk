use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! extensible_status {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[doc = concat!("A forward-compatible `", stringify!($name), "` value.")]
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        #[non_exhaustive]
        pub enum $name {
            $(#[doc = concat!("Wire value `", $wire, "`.")] $variant,)+
            /// A newer server value not known by this SDK version.
            Unknown(String),
        }

        impl $name {
            /// Returns the exact snake-case wire value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $wire,)+
                    Self::Unknown(value) => value,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where S: Serializer {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where D: Deserializer<'de> {
                let value = String::deserialize(deserializer)?;
                Ok(match value.as_str() {
                    $($wire => Self::$variant,)+
                    _ => Self::Unknown(value),
                })
            }
        }
    };
}

extensible_status!(InvoiceStatus {
    AwaitingClient => "awaiting_client",
    AwaitingPayment => "awaiting_payment",
    Confirming => "confirming",
    UnderpaidWaiting => "underpaid_waiting",
    Paid => "paid",
    PaidOver => "paid_over",
    Underpaid => "underpaid",
    Expired => "expired",
    Cancelled => "cancelled",
});

extensible_status!(WithdrawalStatus {
    Created => "created",
    PendingReview => "pending_review",
    Signed => "signed",
    Cancelling => "cancelling",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
});

/// Input for creating an invoice. Monetary amounts stay decimal strings.
#[derive(Clone, Debug, Serialize)]
pub struct CreateInvoiceRequest {
    /// Project that owns the invoice.
    pub project_id: String,
    /// Dot-decimal order amount.
    pub amount: String,
    /// Order currency, such as `USD`.
    pub currency: String,
    /// Merchant idempotency identifier.
    pub external_order_id: String,
    /// Optional preselected network code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Whether the invoice accepts multiple transfers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_multiple_payments: Option<bool>,
    /// Optional customer-paid fee percentage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_fee_percent: Option<f64>,
    /// Optional merchant customer identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

/// Filters accepted by the invoice list endpoint.
#[derive(Clone, Debug, Default)]
pub struct InvoiceListParams {
    /// Maximum items returned by one page.
    pub limit: Option<u32>,
    /// Opaque cursor returned by the preceding page.
    pub cursor: Option<String>,
    /// Status filters. Repeated query values are emitted in sorted order.
    pub status: Option<Vec<InvoiceStatus>>,
    /// Merchant order identifier filter.
    pub external_order_id: Option<String>,
    /// Project filter.
    pub project_id: Option<String>,
    /// Inclusive Unix-seconds lower creation bound.
    pub created_from: Option<i64>,
    /// Inclusive Unix-seconds upper creation bound.
    pub created_to: Option<i64>,
}

impl InvoiceListParams {
    pub(crate) fn query_pairs(&self) -> Result<Vec<(String, String)>, crate::Error> {
        let mut values = Vec::new();
        push_option(&mut values, "limit", self.limit);
        push_string(&mut values, "cursor", self.cursor.as_deref());
        if let Some(statuses) = &self.status {
            if statuses.is_empty() {
                return Err(crate::Error::InvalidArgument(
                    "invoice status filter cannot be empty".to_owned(),
                ));
            }
            values.extend(
                statuses
                    .iter()
                    .map(|status| ("status".to_owned(), status.as_str().to_owned())),
            );
        }
        push_string(
            &mut values,
            "external_order_id",
            self.external_order_id.as_deref(),
        );
        push_string(&mut values, "project_id", self.project_id.as_deref());
        push_option(&mut values, "created_from", self.created_from);
        push_option(&mut values, "created_to", self.created_to);
        Ok(values)
    }
}

/// Merchant order embedded in an invoice.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Order {
    /// Merchant order identifier.
    pub external_id: String,
    /// Optional merchant customer identifier.
    pub client_id: Option<String>,
    /// Dot-decimal order amount.
    pub amount: String,
    /// Order currency.
    pub currency: String,
    /// Selected network, when known.
    pub network: Option<String>,
}

/// Blockchain transfer contributing to an invoice.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Transfer {
    /// Transaction hash.
    pub tx_hash: String,
    /// Dot-decimal transfer amount.
    pub amount: String,
    /// Transfer confirmation status.
    pub status: String,
    /// Creation timestamp in Unix seconds.
    pub created_at: i64,
    /// Confirmation timestamp in Unix seconds.
    pub confirmed_at: Option<i64>,
    /// Confirmations required for settlement.
    pub required_confirmations: Option<u32>,
    /// Estimated confirmation timestamp in Unix seconds.
    pub estimated_confirmation_at: Option<i64>,
    /// Public explorer URL.
    pub explorer_url: Option<String>,
}

/// Payment details selected for an invoice.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Payment {
    /// Token currency.
    pub currency: String,
    /// Network code.
    pub network: String,
    /// Numeric chain identifier.
    pub chain_id: i64,
    /// Token contract address, when applicable.
    pub contract_address: Option<String>,
    /// Expected dot-decimal token amount.
    pub expected: String,
    /// Deposit address, when allocated.
    pub address: Option<String>,
    /// Applied exchange rate.
    pub exchange_rate: Option<String>,
    /// Amount observed on chain.
    pub paid: Option<String>,
    /// Amount still due.
    pub remaining: Option<String>,
    /// Merchant fee.
    pub fee: Option<String>,
    /// Merchant net amount.
    pub net: Option<String>,
    /// Observed transfers.
    pub transfers: Option<Vec<Transfer>>,
}

/// Full invoice contract returned by the Merchant API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Invoice {
    /// Paymos invoice identifier.
    pub invoice_id: String,
    /// Owning project identifier.
    pub project_id: String,
    /// Current invoice status.
    pub status: InvoiceStatus,
    /// Whether the status can no longer change.
    pub is_final: bool,
    /// Whether this is a sandbox invoice.
    pub is_test: bool,
    /// Hosted payment URL.
    pub payment_url: String,
    /// Merchant order data.
    pub order: Order,
    /// Payment selection and settlement details.
    pub payment: Option<Payment>,
    /// Creation timestamp in Unix seconds.
    pub created_at: i64,
    /// Last update timestamp in Unix seconds.
    pub updated_at: i64,
    /// Expiration timestamp in Unix seconds.
    pub expires_at: Option<i64>,
    /// Completion timestamp in Unix seconds.
    pub completed_at: Option<i64>,
    /// invoice-late-payment: the late-payment fact — money crossed the invoice's total after it
    /// had already closed as expired/underpaid. Unix seconds; None while the invoice has no late
    /// mark — the server omits the field then. `status` keeps its terminal
    /// meaning — this marker travels beside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid_late_at: Option<i64>,
}

/// Compact invoice returned from the list endpoint.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InvoiceListItem {
    /// Paymos invoice identifier.
    pub invoice_id: String,
    /// Owning project identifier.
    pub project_id: String,
    /// Merchant order identifier.
    pub external_order_id: String,
    /// Optional merchant customer identifier.
    pub client_id: Option<String>,
    /// Current status.
    pub status: InvoiceStatus,
    /// Whether the status is terminal.
    pub is_final: bool,
    /// Whether this is a sandbox invoice.
    pub is_test: bool,
    /// Dot-decimal order amount.
    pub amount: String,
    /// Order currency.
    pub currency: String,
    /// Selected network, when known.
    pub network: Option<String>,
    /// Creation timestamp in Unix seconds.
    pub created_at: i64,
    /// Expiration timestamp in Unix seconds.
    pub expires_at: Option<i64>,
    /// Completion timestamp in Unix seconds.
    pub completed_at: Option<i64>,
    /// invoice-late-payment: additive late mark, Unix seconds; None while there is no mark (field omitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid_late_at: Option<i64>,
}

/// Input for creating a withdrawal. Monetary amounts stay decimal strings.
#[derive(Clone, Debug, Serialize)]
pub struct CreateWithdrawalRequest {
    /// Destination blockchain address.
    pub destination_address: String,
    /// Network code.
    pub network: String,
    /// Token currency.
    pub currency: String,
    /// Dot-decimal withdrawal amount.
    pub amount: String,
    /// Merchant idempotency identifier.
    pub external_order_id: String,
}

/// Filters accepted by the withdrawal list endpoint.
#[derive(Clone, Debug, Default)]
pub struct WithdrawalListParams {
    /// Maximum items returned by one page.
    pub limit: Option<u32>,
    /// Opaque cursor returned by the preceding page.
    pub cursor: Option<String>,
    /// Status filters.
    pub status: Option<Vec<WithdrawalStatus>>,
    /// Merchant order identifier filter.
    pub external_order_id: Option<String>,
    /// Inclusive Unix-seconds lower creation bound.
    pub created_from: Option<i64>,
    /// Inclusive Unix-seconds upper creation bound.
    pub created_to: Option<i64>,
}

impl WithdrawalListParams {
    pub(crate) fn query_pairs(&self) -> Result<Vec<(String, String)>, crate::Error> {
        let mut values = Vec::new();
        push_option(&mut values, "limit", self.limit);
        push_string(&mut values, "cursor", self.cursor.as_deref());
        if let Some(statuses) = &self.status {
            if statuses.is_empty() {
                return Err(crate::Error::InvalidArgument(
                    "withdrawal status filter cannot be empty".to_owned(),
                ));
            }
            values.extend(
                statuses
                    .iter()
                    .map(|status| ("status".to_owned(), status.as_str().to_owned())),
            );
        }
        push_string(
            &mut values,
            "external_order_id",
            self.external_order_id.as_deref(),
        );
        push_option(&mut values, "created_from", self.created_from);
        push_option(&mut values, "created_to", self.created_to);
        Ok(values)
    }
}

/// Full withdrawal contract returned by the Merchant API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Withdrawal {
    /// Paymos withdrawal identifier.
    pub withdrawal_id: String,
    /// Merchant order identifier.
    pub external_order_id: String,
    /// Current withdrawal status.
    pub status: WithdrawalStatus,
    /// Whether the status is terminal.
    pub is_final: bool,
    /// Whether this is a sandbox withdrawal.
    pub is_test: bool,
    /// Dot-decimal requested amount.
    pub amount: String,
    /// Dot-decimal fee.
    pub fee: Option<String>,
    /// Token currency.
    pub currency: String,
    /// Network code.
    pub network: String,
    /// Destination blockchain address.
    pub destination_address: String,
    /// Broadcast transaction hash.
    pub tx_hash: Option<String>,
    /// Public explorer URL.
    pub explorer_url: Option<String>,
    /// Creation timestamp in Unix seconds.
    pub created_at: i64,
    /// Completion timestamp in Unix seconds.
    pub completed_at: Option<i64>,
    /// Failure timestamp in Unix seconds.
    pub failed_at: Option<i64>,
    /// Cancellation timestamp in Unix seconds.
    pub cancelled_at: Option<i64>,
}

/// Input for creating a payment channel.
///
/// The pair `project_id` + `external_id` IS the idempotency key: repeating it
/// returns the channel that already exists with `200` instead of creating a
/// second one, so this is safe to call on every checkout.
#[derive(Clone, Debug, Serialize)]
pub struct CreatePaymentChannelRequest {
    /// Project that owns the channel.
    pub project_id: String,
    /// Your own stable identifier for the payer, at most 128 characters.
    pub external_id: String,
}

/// Payment-channel sandbox deposit simulation stage.
///
/// Omit it entirely to settle the deposit; sending an unknown value is rejected.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulateDepositStage {
    /// Publish the deposit as seen on chain but not yet settled.
    Confirming,
    /// Drive the simulated deposit through a chain reorganisation.
    Reorged,
    /// Settle the deposit. This is the default when the stage is omitted.
    Confirmed,
}

/// Input for the sandbox deposit simulation. Amounts stay decimal strings.
#[derive(Clone, Debug, Serialize)]
pub struct SimulateDepositRequest {
    /// Dot-decimal deposit amount.
    pub amount: String,
    /// Token currency, such as `USDT`.
    pub currency: String,
    /// Required rail code the simulated deposit lands on, such as `TRC20`.
    pub network: String,
    /// Lifecycle stage to drive the deposit to. Omitted defaults to confirmed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<SimulateDepositStage>,
}

/// One token accepted on a payment-channel rail.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PaymentChannelToken {
    /// Token symbol, such as `USDT`.
    pub symbol: String,
    /// Dot-decimal minimum deposit. Absent means "cannot quote a minimum right
    /// now", never "no minimum".
    pub minimum_deposit: Option<String>,
}

/// One rail of a payment channel.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PaymentChannelNetwork {
    /// Rail code, such as `TRC20`.
    pub network: String,
    /// Rail status: `provisioning` or `active`.
    pub status: String,
    /// Deposit address. Absent until this rail provisions, and never changes
    /// once it appears.
    pub address: Option<String>,
    /// Tokens accepted on this rail.
    pub tokens: Vec<PaymentChannelToken>,
}

/// A payment channel: one payer's reusable set of deposit addresses.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PaymentChannel {
    /// Paymos channel identifier, prefixed `pc_`.
    pub id: String,
    /// Owning project identifier.
    pub project_id: String,
    /// Merchant identifier for the payer.
    pub external_id: String,
    /// Channel status: `active`, `blocked` or `provisioning`.
    pub status: String,
    /// Whether deposits are currently credited.
    pub is_accepting_payments: bool,
    /// Whether every configured rail has an address.
    pub is_fully_provisioned: bool,
    /// Whether this is a sandbox channel.
    pub is_test: bool,
    /// Merchant fee percentage applied to deposits.
    pub applied_fee_percent: f64,
    /// Customer-paid fee percentage.
    pub customer_fee_percent: f64,
    /// Rails provisioned for this channel.
    pub networks: Vec<PaymentChannelNetwork>,
    /// Creation timestamp in Unix seconds.
    pub created_at: i64,
    /// Last update timestamp in Unix seconds.
    pub updated_at: i64,
}

/// One deposit credited to a payment channel.
///
/// This is both the API resource and the `data` payload of every
/// `payment_channel.deposit.*` webhook event (`confirming`, `reorged`,
/// `confirmed`), so a handler decodes one with
/// `verifier.construct_event::<PaymentChannelDeposit>(header, raw_body)`.
///
/// ORDERING — `confirming` and `reorged` are ADVISORY and may arrive out of
/// order: a `confirming` can land after the `confirmed` for the same deposit,
/// and a `reorged` can be superseded by a later `confirmed`. Credit the
/// merchant only on `payment_channel.deposit.confirmed` with `is_final` true,
/// and never let an advisory event regress a deposit already known to be
/// confirmed. Crediting on `confirming` releases goods against money a reorg
/// can still take back.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PaymentChannelDeposit {
    /// Paymos deposit identifier, prefixed `pcd_`.
    pub id: String,
    /// Channel that received the deposit.
    pub payment_channel_id: String,
    /// Owning project identifier.
    pub project_id: String,
    /// Merchant identifier of the payer that owns the channel.
    pub payment_channel_external_id: String,
    /// Deposit status: `confirming`, `reorged` or `confirmed`.
    pub status: String,
    /// Whether the status can no longer change.
    pub is_final: bool,
    /// Whether this is a sandbox deposit.
    pub is_test: bool,
    /// Token currency.
    pub currency: String,
    /// Rail code the deposit landed on.
    pub network: String,
    /// Numeric chain identifier.
    pub chain_id: i64,
    /// Token contract address, when applicable.
    pub contract_address: Option<String>,
    /// Dot-decimal amount received.
    pub gross: String,
    /// Dot-decimal Paymos fee.
    pub fee: String,
    /// Dot-decimal merchant net amount.
    pub net: String,
    /// Merchant fee percentage applied to this deposit.
    pub applied_fee_percent: f64,
    /// Customer-paid fee percentage.
    pub customer_fee_percent: f64,
    /// Transaction hash.
    pub tx_hash: String,
    /// Identifier of the underlying transfer.
    pub transfer_id: String,
    /// Sending address, when the chain exposes one.
    pub source_address: Option<String>,
    /// Channel address that received the deposit.
    pub destination_address: String,
    /// Block that included the transfer.
    pub block_height: i64,
    /// Timestamp in Unix seconds of the first including block.
    pub first_included_block_timestamp: Option<i64>,
    /// Public explorer URL.
    pub explorer_url: Option<String>,
    /// Creation timestamp in Unix seconds.
    pub created_at: i64,
    /// Last update timestamp in Unix seconds.
    pub updated_at: i64,
    /// Settlement timestamp in Unix seconds.
    pub confirmed_at: Option<i64>,
}

/// The one feed position that could not be rendered, and why.
///
/// Without it a stuck feed is byte-identical to a quiet day, and a merchant
/// cannot tell "no new payments" from "I am not seeing my money". Surface it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PaymentChannelDepositFeedBlockage {
    /// The `pcd_` deposit that could not be rendered.
    pub deposit_id: String,
    /// Stable reason code: `booked_auth_missing`, `inconsistent`,
    /// `deposit_missing`, `channel_missing`, `transfer_missing` or
    /// `not_confirmed`. Contact support with the deposit identifier.
    pub reason: String,
}

/// One page of the confirmed-deposit feed.
///
/// This is deliberately NOT a [`CursorPage`]: `next_cursor` is not optional.
/// Feed positions are global across all merchants, so a page filtered down to
/// nothing still hands back a cursor past everything it skipped. The feed has
/// no last page — money keeps arriving — so never stop polling because the
/// cursor looks final. Store it and resume from it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PaymentChannelDepositFeedPage {
    /// Deposits in this page, oldest first.
    pub items: Vec<PaymentChannelDeposit>,
    /// Cursor to resume from. Always present, including on an empty page.
    pub next_cursor: String,
    /// Present only when one position could not be rendered.
    pub blocked: Option<PaymentChannelDepositFeedBlockage>,
}

/// Filters accepted by the payment-channel list endpoint.
///
/// The endpoint rejects unknown query fields outright, so this set is closed.
#[derive(Clone, Debug, Default)]
pub struct PaymentChannelListParams {
    /// Maximum items returned by one page.
    pub limit: Option<u32>,
    /// Opaque cursor returned by the preceding page.
    pub cursor: Option<String>,
    /// Status filters: `active`, `blocked` or `provisioning`. Repeated query
    /// values are emitted in sorted order.
    pub status: Option<Vec<String>>,
    /// Merchant payer identifier filter.
    pub external_id: Option<String>,
    /// Project filter.
    pub project_id: Option<String>,
}

impl PaymentChannelListParams {
    pub(crate) fn query_pairs(&self) -> Result<Vec<(String, String)>, crate::Error> {
        let mut values = Vec::new();
        push_option(&mut values, "limit", self.limit);
        push_string(&mut values, "cursor", self.cursor.as_deref());
        if let Some(statuses) = &self.status {
            if statuses.is_empty() {
                return Err(crate::Error::InvalidArgument(
                    "payment channel status filter cannot be empty".to_owned(),
                ));
            }
            values.extend(
                statuses
                    .iter()
                    .map(|status| ("status".to_owned(), status.clone())),
            );
        }
        push_string(&mut values, "external_id", self.external_id.as_deref());
        push_string(&mut values, "project_id", self.project_id.as_deref());
        Ok(values)
    }
}

/// Filters accepted by the payment-channel deposit feed.
///
/// The endpoint rejects unknown query fields outright, so this set is closed.
#[derive(Clone, Debug, Default)]
pub struct PaymentChannelDepositFeedParams {
    /// Maximum items returned by one page.
    pub limit: Option<u32>,
    /// Opaque cursor returned by the preceding page. This is how a poller
    /// resumes.
    pub cursor: Option<String>,
    /// Project filter.
    pub project_id: Option<String>,
    /// Channel filter.
    pub payment_channel_id: Option<String>,
    /// Inclusive Unix-seconds lower confirmation bound. This answers "start me
    /// from this point in time" on the FIRST poll only; afterwards resume from
    /// the cursor the feed returned, or whole spans get re-read or skipped.
    pub confirmed_from: Option<i64>,
}

impl PaymentChannelDepositFeedParams {
    pub(crate) fn query_pairs(&self) -> Vec<(String, String)> {
        let mut values = Vec::new();
        push_option(&mut values, "limit", self.limit);
        push_string(&mut values, "cursor", self.cursor.as_deref());
        push_string(&mut values, "project_id", self.project_id.as_deref());
        push_string(
            &mut values,
            "payment_channel_id",
            self.payment_channel_id.as_deref(),
        );
        push_option(&mut values, "confirmed_from", self.confirmed_from);
        values
    }
}

/// A page returned from a cursor list endpoint.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CursorPage<T> {
    /// Items in this page.
    pub items: Vec<T>,
    /// Opaque cursor for the next page.
    pub next_cursor: Option<String>,
}

/// Merchant balance for one currency.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Balance {
    /// Currency symbol.
    pub currency: String,
    /// Available dot-decimal amount.
    pub available: String,
}

/// Server clock response.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerTime {
    /// Current server time in Unix seconds.
    pub server_time: i64,
}

/// Invoice sandbox simulation stage.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceSimulationStage {
    /// Fully pay the invoice.
    Paid,
    /// Pay more than expected.
    Overpaid,
    /// Pay less than expected.
    Underpay,
    /// Cancel the invoice.
    Cancel,
    /// invoice-late-payment: pay the full amount after the deadline — the invoice keeps its
    /// terminal decision and receives the late-payment mark.
    PaidLate,
}

fn push_string(values: &mut Vec<(String, String)>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        values.push((key.to_owned(), value.to_owned()));
    }
}

fn push_option<T: ToString>(values: &mut Vec<(String, String)>, key: &str, value: Option<T>) {
    if let Some(value) = value {
        values.push((key.to_owned(), value.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_unknown_statuses() {
        let status: InvoiceStatus = serde_json::from_str("\"future_status\"").unwrap();
        assert_eq!(status, InvoiceStatus::Unknown("future_status".to_owned()));
        assert_eq!(serde_json::to_string(&status).unwrap(), "\"future_status\"");
    }
}
