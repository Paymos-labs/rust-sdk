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
