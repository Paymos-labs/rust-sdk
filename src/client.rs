use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Serialize;
use zeroize::Zeroizing;

use crate::error::{ApiError, ApiErrorKind, ProblemDetails};
use crate::models::{
    Balance, CreateInvoiceRequest, CreateWithdrawalRequest, CursorPage, Invoice, InvoiceListItem,
    InvoiceListParams, InvoiceSimulationStage, ServerTime, Withdrawal, WithdrawalListParams,
};
use crate::signing::{authorization_header, build_query, encode_component};
use crate::{Error, SDK_VERSION};

const DEFAULT_BASE_URL: &str = "https://api.paymos.io";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_RETRIES: u32 = 2;
const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(150);
const DEFAULT_MAX_PAGES: usize = 100;

/// Async client for the Paymos Merchant API.
#[derive(Clone)]
pub struct PaymosClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    api_key: String,
    api_secret: Zeroizing<String>,
    base_url: String,
    http: reqwest::Client,
    max_retries: u32,
    base_delay: Duration,
}

/// Builder for [`PaymosClient`].
pub struct ClientBuilder {
    api_key: String,
    api_secret: String,
    base_url: String,
    timeout: Duration,
    max_retries: u32,
    base_delay: Duration,
}

impl PaymosClient {
    /// Creates a client using production defaults.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] for invalid credentials or client settings.
    pub fn new(api_key: impl Into<String>, api_secret: impl Into<String>) -> Result<Self, Error> {
        Self::builder(api_key, api_secret).build()
    }

    /// Starts a client builder.
    #[must_use]
    pub fn builder(api_key: impl Into<String>, api_secret: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            api_key: api_key.into(),
            api_secret: api_secret.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            timeout: DEFAULT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay: DEFAULT_BASE_DELAY,
        }
    }

    /// Accesses invoice operations.
    #[must_use]
    pub const fn invoices(&self) -> Invoices<'_> {
        Invoices { client: self }
    }

    /// Accesses withdrawal operations.
    #[must_use]
    pub const fn withdrawals(&self) -> Withdrawals<'_> {
        Withdrawals { client: self }
    }

    /// Accesses balance operations.
    #[must_use]
    pub const fn balances(&self) -> Balances<'_> {
        Balances { client: self }
    }

    /// Accesses system operations.
    #[must_use]
    pub const fn system(&self) -> System<'_> {
        System { client: self }
    }

    async fn request_empty<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &str,
    ) -> Result<T, Error> {
        self.request(method, path, query, &[]).await
    }

    async fn request_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        query: &str,
        payload: &B,
    ) -> Result<T, Error> {
        let body = serde_json::to_vec(payload).map_err(Error::Serialization)?;
        self.request(method, path, query, &body).await
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &str,
        body: &[u8],
    ) -> Result<T, Error> {
        let url = format!("{}{path}{query}", self.inner.base_url);
        let mut attempt = 0;
        loop {
            let timestamp = unix_now()?;
            let authorization = authorization_header(
                &self.inner.api_key,
                self.inner.api_secret.as_bytes(),
                timestamp,
                method.as_str(),
                path,
                query,
                body,
            );
            let mut request = self
                .inner
                .http
                .request(method.clone(), &url)
                .header(AUTHORIZATION, header_value(&authorization)?)
                .header("X-Request-Timestamp", timestamp.to_string())
                .header(ACCEPT, "application/json");
            if !body.is_empty() {
                request = request
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.to_vec());
            }

            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    if attempt < self.inner.max_retries && is_idempotent(&method) {
                        attempt += 1;
                        tokio::time::sleep(backoff(self.inner.base_delay, attempt)).await;
                        continue;
                    }
                    return Err(Error::Transport(error));
                }
            };

            let status = response.status();
            let response_headers = response.headers().clone();
            if attempt < self.inner.max_retries && should_retry(&method, status) {
                let retry_after = parse_retry_after(response_headers.get(RETRY_AFTER));
                let _ = response.bytes().await;
                attempt += 1;
                tokio::time::sleep(std::cmp::max(
                    backoff(self.inner.base_delay, attempt),
                    retry_after.unwrap_or_default(),
                ))
                .await;
                continue;
            }

            let bytes = response.bytes().await.map_err(Error::Transport)?;
            if !status.is_success() {
                return Err(ApiError::from_response(status, &response_headers, &bytes).into());
            }
            return serde_json::from_slice(&bytes).map_err(Error::InvalidResponse);
        }
    }
}

impl ClientBuilder {
    /// Overrides the API base URL. Plain HTTP is accepted only for loopback tests.
    #[must_use]
    pub fn base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = value.into();
        self
    }

    /// Sets the complete request timeout.
    #[must_use]
    pub const fn timeout(mut self, value: Duration) -> Self {
        self.timeout = value;
        self
    }

    /// Sets the maximum number of automatic retries.
    #[must_use]
    pub const fn max_retries(mut self, value: u32) -> Self {
        self.max_retries = value;
        self
    }

    /// Sets the exponential-backoff base delay.
    #[must_use]
    pub const fn base_delay(mut self, value: Duration) -> Self {
        self.base_delay = value;
        self
    }

    /// Validates the configuration and builds the client.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] when any builder setting is invalid, or
    /// [`Error::Transport`] when the underlying HTTP client cannot be created.
    pub fn build(self) -> Result<PaymosClient, Error> {
        validate_credential(&self.api_key, "api_key")?;
        validate_credential(&self.api_secret, "api_secret")?;
        if self.timeout.is_zero() {
            return Err(Error::Configuration(
                "timeout must be greater than zero".to_owned(),
            ));
        }
        let base_url = validate_base_url(&self.base_url)?;
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(format!("paymos-rust/{SDK_VERSION}"))
            .build()
            .map_err(Error::Transport)?;
        Ok(PaymosClient {
            inner: Arc::new(ClientInner {
                api_key: self.api_key,
                api_secret: Zeroizing::new(self.api_secret),
                base_url,
                http,
                max_retries: self.max_retries,
                base_delay: self.base_delay,
            }),
        })
    }
}

/// Invoice operations.
pub struct Invoices<'a> {
    client: &'a PaymosClient,
}

impl Invoices<'_> {
    /// Creates an invoice.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for serialization, transport, API, or response failures.
    pub async fn create(&self, request: &CreateInvoiceRequest) -> Result<Invoice, Error> {
        self.client
            .request_json(Method::POST, "/v1/invoices", "", request)
            .await
    }

    /// Gets an invoice by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the identifier or request fails validation or the API call fails.
    pub async fn get(&self, invoice_id: &str) -> Result<Invoice, Error> {
        let path = format!("/v1/invoices/{}", required_id(invoice_id)?);
        self.client.request_empty(Method::GET, &path, "").await
    }

    /// Lists one invoice page.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when filters are invalid or the API call fails.
    pub async fn list(
        &self,
        params: &InvoiceListParams,
    ) -> Result<CursorPage<InvoiceListItem>, Error> {
        let query = build_query(params.query_pairs()?)?;
        self.client
            .request_empty(Method::GET, "/v1/invoices", &query)
            .await
    }

    /// Creates a bounded async cursor pager.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Pagination`] when `max_pages` is zero.
    pub fn pager(
        &self,
        params: InvoiceListParams,
        max_pages: Option<usize>,
    ) -> Result<InvoicePager, Error> {
        InvoicePager::new(
            self.client.clone(),
            params,
            max_pages.unwrap_or(DEFAULT_MAX_PAGES),
        )
    }

    /// Cancels an invoice.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for an invalid identifier or reason, or when the API call fails.
    pub async fn cancel(&self, invoice_id: &str, reason: &str) -> Result<Invoice, Error> {
        let path = format!("/v1/invoices/{}/cancel", required_id(invoice_id)?);
        self.client
            .request_json(Method::POST, &path, "", &CancelRequest::new(reason)?)
            .await
    }

    /// Confirms the payment currency and network selected by the customer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for an invalid identifier or when the API call fails.
    pub async fn confirm_payment(
        &self,
        invoice_id: &str,
        currency: &str,
        network: &str,
    ) -> Result<Invoice, Error> {
        let path = format!("/v1/invoices/{}/confirm-payment", required_id(invoice_id)?);
        self.client
            .request_json(
                Method::POST,
                &path,
                "",
                &ConfirmPaymentRequest { currency, network },
            )
            .await
    }

    /// Advances a sandbox invoice through a simulated payment stage.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for an invalid identifier or when the API call fails.
    pub async fn simulate_payment(
        &self,
        invoice_id: &str,
        stage: InvoiceSimulationStage,
    ) -> Result<Invoice, Error> {
        let path = format!(
            "/v1/sandbox/invoices/{}/simulate-payment",
            required_id(invoice_id)?
        );
        self.client
            .request_json(Method::POST, &path, "", &SimulationRequest { stage })
            .await
    }
}

/// Withdrawal operations.
pub struct Withdrawals<'a> {
    client: &'a PaymosClient,
}

impl Withdrawals<'_> {
    /// Creates a withdrawal.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for serialization, transport, API, or response failures.
    pub async fn create(&self, request: &CreateWithdrawalRequest) -> Result<Withdrawal, Error> {
        self.client
            .request_json(Method::POST, "/v1/withdrawals", "", request)
            .await
    }

    /// Gets a withdrawal by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the identifier is invalid or the API call fails.
    pub async fn get(&self, withdrawal_id: &str) -> Result<Withdrawal, Error> {
        let path = format!("/v1/withdrawals/{}", required_id(withdrawal_id)?);
        self.client.request_empty(Method::GET, &path, "").await
    }

    /// Lists one withdrawal page.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when filters are invalid or the API call fails.
    pub async fn list(
        &self,
        params: &WithdrawalListParams,
    ) -> Result<CursorPage<Withdrawal>, Error> {
        let query = build_query(params.query_pairs()?)?;
        self.client
            .request_empty(Method::GET, "/v1/withdrawals", &query)
            .await
    }

    /// Creates a bounded async cursor pager.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Pagination`] when `max_pages` is zero.
    pub fn pager(
        &self,
        params: WithdrawalListParams,
        max_pages: Option<usize>,
    ) -> Result<WithdrawalPager, Error> {
        WithdrawalPager::new(
            self.client.clone(),
            params,
            max_pages.unwrap_or(DEFAULT_MAX_PAGES),
        )
    }

    /// Cancels a withdrawal.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for an invalid identifier or reason, or when the API call fails.
    pub async fn cancel(&self, withdrawal_id: &str, reason: &str) -> Result<Withdrawal, Error> {
        let path = format!("/v1/withdrawals/{}/cancel", required_id(withdrawal_id)?);
        self.client
            .request_json(Method::POST, &path, "", &CancelRequest::new(reason)?)
            .await
    }

    /// Completes a sandbox withdrawal simulation.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for an invalid identifier or when the API call fails.
    pub async fn simulate_completion(&self, withdrawal_id: &str) -> Result<Withdrawal, Error> {
        let path = format!(
            "/v1/sandbox/withdrawals/{}/simulate-completion",
            required_id(withdrawal_id)?
        );
        self.client.request_empty(Method::POST, &path, "").await
    }
}

/// Balance operations.
pub struct Balances<'a> {
    client: &'a PaymosClient,
}

impl Balances<'_> {
    /// Gets balances grouped by currency.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the API call or response decoding fails.
    pub async fn get(&self) -> Result<Vec<Balance>, Error> {
        self.client
            .request_empty(Method::GET, "/v1/balances", "")
            .await
    }
}

/// System operations.
pub struct System<'a> {
    client: &'a PaymosClient,
}

impl System<'_> {
    /// Gets server time for diagnosing clock drift.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the API call or response decoding fails.
    pub async fn time(&self) -> Result<ServerTime, Error> {
        self.client.request_empty(Method::GET, "/v1/time", "").await
    }
}

/// Bounded asynchronous invoice cursor traversal.
pub struct InvoicePager {
    client: PaymosClient,
    params: InvoiceListParams,
    max_pages: usize,
    pages: usize,
    buffered: VecDeque<InvoiceListItem>,
    seen_cursors: HashSet<String>,
    done: bool,
}

impl InvoicePager {
    fn new(
        client: PaymosClient,
        params: InvoiceListParams,
        max_pages: usize,
    ) -> Result<Self, Error> {
        if max_pages == 0 {
            return Err(Error::Pagination("max_pages must be positive".to_owned()));
        }
        let seen_cursors = params.cursor.iter().cloned().collect();
        Ok(Self {
            client,
            params,
            max_pages,
            pages: 0,
            buffered: VecDeque::new(),
            seen_cursors,
            done: false,
        })
    }

    /// Returns the next item, or `None` at the end or configured page bound.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a page request fails or a cursor is repeated.
    pub async fn next(&mut self) -> Result<Option<InvoiceListItem>, Error> {
        loop {
            if let Some(item) = self.buffered.pop_front() {
                return Ok(Some(item));
            }
            if self.done || self.pages >= self.max_pages {
                return Ok(None);
            }
            let page = self.client.invoices().list(&self.params).await?;
            self.pages += 1;
            self.buffered.extend(page.items);
            match page.next_cursor {
                Some(cursor) if !cursor.is_empty() => {
                    if !self.seen_cursors.insert(cursor.clone()) {
                        return Err(Error::Pagination(
                            "API returned a pagination cursor more than once".to_owned(),
                        ));
                    }
                    self.params.cursor = Some(cursor);
                }
                _ => self.done = true,
            }
        }
    }
}

/// Bounded asynchronous withdrawal cursor traversal.
pub struct WithdrawalPager {
    client: PaymosClient,
    params: WithdrawalListParams,
    max_pages: usize,
    pages: usize,
    buffered: VecDeque<Withdrawal>,
    seen_cursors: HashSet<String>,
    done: bool,
}

impl WithdrawalPager {
    fn new(
        client: PaymosClient,
        params: WithdrawalListParams,
        max_pages: usize,
    ) -> Result<Self, Error> {
        if max_pages == 0 {
            return Err(Error::Pagination("max_pages must be positive".to_owned()));
        }
        let seen_cursors = params.cursor.iter().cloned().collect();
        Ok(Self {
            client,
            params,
            max_pages,
            pages: 0,
            buffered: VecDeque::new(),
            seen_cursors,
            done: false,
        })
    }

    /// Returns the next item, or `None` at the end or configured page bound.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a page request fails or a cursor is repeated.
    pub async fn next(&mut self) -> Result<Option<Withdrawal>, Error> {
        loop {
            if let Some(item) = self.buffered.pop_front() {
                return Ok(Some(item));
            }
            if self.done || self.pages >= self.max_pages {
                return Ok(None);
            }
            let page = self.client.withdrawals().list(&self.params).await?;
            self.pages += 1;
            self.buffered.extend(page.items);
            match page.next_cursor {
                Some(cursor) if !cursor.is_empty() => {
                    if !self.seen_cursors.insert(cursor.clone()) {
                        return Err(Error::Pagination(
                            "API returned a pagination cursor more than once".to_owned(),
                        ));
                    }
                    self.params.cursor = Some(cursor);
                }
                _ => self.done = true,
            }
        }
    }
}

#[derive(Serialize)]
struct CancelRequest<'a> {
    reason: &'a str,
}

impl<'a> CancelRequest<'a> {
    fn new(reason: &'a str) -> Result<Self, Error> {
        let length = reason.encode_utf16().count();
        if reason.trim().is_empty() || length > 500 {
            return Err(Error::InvalidArgument(
                "cancellation reason must contain 1 to 500 characters".to_owned(),
            ));
        }
        Ok(Self { reason })
    }
}

#[derive(Serialize)]
struct ConfirmPaymentRequest<'a> {
    currency: &'a str,
    network: &'a str,
}

#[derive(Serialize)]
struct SimulationRequest {
    stage: InvoiceSimulationStage,
}

impl ApiError {
    fn from_response(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Self {
        let problem = serde_json::from_slice::<ProblemDetails>(body)
            .ok()
            .filter(|problem| problem.status == status.as_u16());
        let (problem_type, title, detail, code, field, errors, trace_id) = match problem {
            Some(problem) => (
                Some(problem.problem_type),
                Some(problem.title),
                Some(problem.detail),
                Some(problem.code),
                problem.field,
                problem.errors,
                problem.trace_id,
            ),
            None => (None, None, None, None, None, Vec::new(), None),
        };
        Self {
            status: status.as_u16(),
            kind: ApiErrorKind::from_status(status.as_u16()),
            problem_type,
            title,
            detail,
            code,
            field,
            errors,
            retry_after: parse_retry_after(headers.get(RETRY_AFTER)),
            trace_id,
            raw_body: String::from_utf8_lossy(body).into_owned(),
        }
    }
}

fn validate_credential(value: &str, name: &str) -> Result<(), Error> {
    if value.trim().is_empty() {
        return Err(Error::Configuration(format!("{name} must be non-empty")));
    }
    if value.chars().any(char::is_control) {
        return Err(Error::Configuration(format!(
            "{name} must not contain control characters"
        )));
    }
    Ok(())
}

fn validate_base_url(value: &str) -> Result<String, Error> {
    let url = Url::parse(value)
        .map_err(|error| Error::Configuration(format!("base_url is invalid: {error}")))?;
    if url.cannot_be_a_base()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(Error::Configuration(
            "base_url must contain only scheme and authority".to_owned(),
        ));
    }
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(Error::Configuration(
            "base_url must use HTTPS (HTTP is allowed only for loopback tests)".to_owned(),
        ));
    }
    Ok(value.trim_end_matches('/').to_owned())
}

fn required_id(value: &str) -> Result<String, Error> {
    if value.trim().is_empty() {
        return Err(Error::InvalidArgument(
            "resource identifier must be non-empty".to_owned(),
        ));
    }
    Ok(encode_component(value))
}

fn header_value(value: &str) -> Result<HeaderValue, Error> {
    HeaderValue::from_str(value)
        .map_err(|_| Error::Configuration("api_key cannot be encoded as an HTTP header".to_owned()))
}

const fn is_idempotent(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn should_retry(method: &Method, status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || (status.is_server_error() && is_idempotent(method))
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    base.saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1)))
}

fn parse_retry_after(value: Option<&HeaderValue>) -> Option<Duration> {
    let value = value?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(date.duration_since(SystemTime::now()).unwrap_or_default())
}

fn unix_now() -> Result<i64, Error> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidClock)?
        .as_secs();
    i64::try_from(seconds).map_err(|_| Error::InvalidClock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_plain_http_for_remote_hosts() {
        let error = PaymosClient::builder("pk", "sk")
            .base_url("http://example.com")
            .build()
            .err()
            .unwrap();
        assert!(matches!(error, Error::Configuration(_)));
    }

    #[test]
    fn accepts_loopback_http_for_tests() {
        PaymosClient::builder("pk", "sk")
            .base_url("http://127.0.0.1:8080")
            .build()
            .unwrap();
    }

    #[test]
    fn maps_error_categories() {
        assert_eq!(ApiErrorKind::from_status(429), ApiErrorKind::RateLimit);
        assert_eq!(ApiErrorKind::from_status(503), ApiErrorKind::Unavailable);
    }
}
