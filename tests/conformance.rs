//! Cross-language contract, route, retry, pagination, and webhook tests.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use paymos::signing::{authorization_header, build_query, sign, string_to_sign};
use paymos::{
    ApiErrorKind, CreateInvoiceRequest, CreatePaymentChannelRequest, CreateWithdrawalRequest,
    Error, InvoiceListParams, InvoiceSimulationStage, InvoiceStatus,
    PaymentChannelDepositFeedParams, PaymentChannelListParams, PaymosClient,
    SimulateDepositRequest, SimulateDepositStage, WebhookError, WebhookEvent, WebhookVerifier,
    WithdrawalListParams,
};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[derive(Deserialize)]
struct Contract {
    vectors: Vectors,
}

#[derive(Deserialize)]
struct Vectors {
    post_signing: SigningVector,
    post_empty_body_signing: SigningVector,
    get_query_signing: SigningVector,
    webhook: WebhookVector,
}

#[derive(Deserialize)]
struct SigningVector {
    api_key: String,
    api_secret: String,
    timestamp: String,
    method: String,
    path: String,
    query: String,
    body: String,
    signature: Option<String>,
    authorization: Option<String>,
}

#[derive(Deserialize)]
struct WebhookVector {
    secret: String,
    timestamp: i64,
    now: i64,
    tolerance_seconds: u64,
    raw_body: String,
    header: String,
}

#[derive(Clone)]
struct Response {
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    body: &'static str,
}

impl Response {
    const fn ok(body: &'static str) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body,
        }
    }
}

struct TestServer {
    base_url: String,
    requests: mpsc::Receiver<String>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel(responses.len().max(1));
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let request = read_request(&mut socket).await;
                sender.send(request).await.unwrap();
                let reason = match response.status {
                    200 => "OK",
                    201 => "Created",
                    400 => "Bad Request",
                    429 => "Too Many Requests",
                    503 => "Service Unavailable",
                    _ => "Error",
                };
                let mut headers = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    response.body.len()
                );
                for (name, value) in response.headers {
                    write!(headers, "{name}: {value}\r\n").unwrap();
                }
                headers.push_str("\r\n");
                socket.write_all(headers.as_bytes()).await.unwrap();
                socket.write_all(response.body.as_bytes()).await.unwrap();
            }
        });
        Self {
            base_url,
            requests,
            task,
        }
    }

    async fn paths(self) -> Vec<String> {
        self.raw_requests()
            .await
            .iter()
            .map(|request| {
                let line = request.lines().next().unwrap();
                let mut parts = line.split_whitespace();
                format!("{} {}", parts.next().unwrap(), parts.next().unwrap())
            })
            .collect()
    }

    async fn raw_requests(mut self) -> Vec<String> {
        self.task.await.unwrap();
        let mut received = Vec::new();
        while let Some(request) = self.requests.recv().await {
            received.push(request);
        }
        received
    }
}

/// The JSON body a captured request carried, or `null` when it carried none.
fn request_body(request: &str) -> serde_json::Value {
    request
        .split_once("\r\n\r\n")
        .map_or(serde_json::Value::Null, |(_, body)| {
            serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
        })
}

fn contract() -> Contract {
    serde_json::from_str(include_str!("../conformance/contract.json")).unwrap()
}

#[test]
fn byte_exact_signing_and_query_conformance() {
    let contract = contract();
    let post = contract.vectors.post_signing;
    let authorization = authorization_header(
        &post.api_key,
        post.api_secret.as_bytes(),
        &post.timestamp,
        &post.method,
        &post.path,
        &post.query,
        post.body.as_bytes(),
    );
    assert_eq!(authorization, post.authorization.unwrap());

    // An absent body signs an EMPTY last segment, never sha256(""). This vector is a POST, so an
    // SDK that shortcuts on the method ("only GET is empty") fails here.
    let empty = contract.vectors.post_empty_body_signing;
    assert_eq!(
        authorization_header(
            &empty.api_key,
            empty.api_secret.as_bytes(),
            &empty.timestamp,
            &empty.method,
            &empty.path,
            &empty.query,
            empty.body.as_bytes(),
        ),
        empty.authorization.unwrap()
    );

    let get = contract.vectors.get_query_signing;
    let canonical = string_to_sign(
        &get.timestamp,
        &get.method,
        &get.path,
        &get.query,
        get.body.as_bytes(),
    );
    assert_eq!(
        sign(get.api_secret.as_bytes(), canonical.as_bytes()),
        get.signature.unwrap()
    );
    let query = build_query([
        ("status", "paid_over"),
        ("project_id", "prj/a"),
        ("limit", "50"),
        ("status", "paid"),
    ])
    .unwrap();
    assert_eq!(query, get.query);
}

#[test]
fn raw_body_webhook_conformance_and_typed_event() {
    #[derive(Debug, Deserialize)]
    struct InvoiceData {
        invoice_id: String,
    }

    let vector = contract().vectors.webhook;
    let verifier =
        WebhookVerifier::with_tolerance(&vector.secret, vector.tolerance_seconds).unwrap();
    verifier
        .verify_at(&vector.header, vector.raw_body.as_bytes(), vector.now)
        .unwrap();
    let event: WebhookEvent<InvoiceData> = verifier
        .construct_event_at(&vector.header, vector.raw_body.as_bytes(), vector.now)
        .unwrap();
    assert_eq!(event.event_id, "evt_123");
    assert_eq!(event.data.invoice_id, "inv_123");

    let duplicate = format!("{},t={}", vector.header, vector.timestamp);
    assert!(matches!(
        verifier.verify_at(&duplicate, vector.raw_body.as_bytes(), vector.now),
        Err(WebhookError::MalformedHeader)
    ));
    let tolerance = i64::try_from(vector.tolerance_seconds).unwrap();
    let stale_now = vector
        .now
        .checked_add(tolerance)
        .and_then(|value| value.checked_add(1))
        .unwrap();
    assert!(matches!(
        verifier.verify_at(&vector.header, vector.raw_body.as_bytes(), stale_now),
        Err(WebhookError::TimestampOutsideTolerance)
    ));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn complete_resource_matrix_uses_the_frozen_routes() {
    const INVOICE: &str = r#"{"invoice_id":"inv_1","project_id":"prj_1","status":"awaiting_client","is_final":false,"is_test":true,"payment_url":"https://pay.paymos.io/inv_1","order":{"external_id":"order_1","amount":"10","currency":"USD"},"created_at":1700000000,"updated_at":1700000000}"#;
    const WITHDRAWAL: &str = r#"{"withdrawal_id":"wdr_1","external_order_id":"order_1","status":"created","is_final":false,"is_test":true,"amount":"10","currency":"USDT","network":"TRC20","destination_address":"TAddress","created_at":1700000000}"#;
    let server = TestServer::start(vec![
        Response::ok(r#"{"server_time":1700000000}"#),
        Response::ok(INVOICE),
        Response::ok(INVOICE),
        Response::ok(r#"{"items":[],"next_cursor":null}"#),
        Response::ok(INVOICE),
        Response::ok(INVOICE),
        Response::ok(INVOICE),
        Response::ok(WITHDRAWAL),
        Response::ok(WITHDRAWAL),
        Response::ok(r#"{"items":[],"next_cursor":null}"#),
        Response::ok(WITHDRAWAL),
        Response::ok(WITHDRAWAL),
        Response::ok("[]"),
    ])
    .await;
    let client = PaymosClient::builder("pk_test_key", "sk_test_secret")
        .base_url(&server.base_url)
        .max_retries(0)
        .build()
        .unwrap();

    client.system().time().await.unwrap();
    client
        .invoices()
        .create(&CreateInvoiceRequest {
            project_id: "prj_1".to_owned(),
            amount: "10".to_owned(),
            currency: "USD".to_owned(),
            external_order_id: "order_1".to_owned(),
            network: None,
            allow_multiple_payments: None,
            customer_fee_percent: None,
            client_id: None,
        })
        .await
        .unwrap();
    client.invoices().get("inv/1").await.unwrap();
    client
        .invoices()
        .list(&InvoiceListParams {
            limit: Some(1),
            ..InvoiceListParams::default()
        })
        .await
        .unwrap();
    client.invoices().cancel("inv_1", "reason").await.unwrap();
    client
        .invoices()
        .confirm_payment("inv_1", "USDT", "TRC20")
        .await
        .unwrap();
    client
        .invoices()
        .simulate_payment("inv_1", InvoiceSimulationStage::Paid)
        .await
        .unwrap();
    client
        .withdrawals()
        .create(&CreateWithdrawalRequest {
            destination_address: "TAddress".to_owned(),
            network: "TRC20".to_owned(),
            currency: "USDT".to_owned(),
            amount: "10".to_owned(),
            external_order_id: "order_1".to_owned(),
        })
        .await
        .unwrap();
    client.withdrawals().get("wdr_1").await.unwrap();
    client
        .withdrawals()
        .list(&WithdrawalListParams {
            limit: Some(1),
            ..WithdrawalListParams::default()
        })
        .await
        .unwrap();
    client
        .withdrawals()
        .cancel("wdr_1", "reason")
        .await
        .unwrap();
    client
        .withdrawals()
        .simulate_completion("wdr_1")
        .await
        .unwrap();
    client.balances().get().await.unwrap();

    assert_eq!(
        server.paths().await,
        vec![
            "GET /v1/time",
            "POST /v1/invoices",
            "GET /v1/invoices/inv%2F1",
            "GET /v1/invoices?limit=1",
            "POST /v1/invoices/inv_1/cancel",
            "POST /v1/invoices/inv_1/confirm-payment",
            "POST /v1/sandbox/invoices/inv_1/simulate-payment",
            "POST /v1/withdrawals",
            "GET /v1/withdrawals/wdr_1",
            "GET /v1/withdrawals?limit=1",
            "POST /v1/withdrawals/wdr_1/cancel",
            "POST /v1/sandbox/withdrawals/wdr_1/simulate-completion",
            "GET /v1/balances",
        ]
    );
}

#[tokio::test]
async fn retries_429_post_but_not_503_post() {
    const INVOICE: &str = r#"{"invoice_id":"inv_1","project_id":"prj_1","status":"awaiting_client","is_final":false,"is_test":true,"payment_url":"https://pay.paymos.io/inv_1","order":{"external_id":"order_1","amount":"10","currency":"USD"},"created_at":1700000000,"updated_at":1700000000}"#;
    let request = CreateInvoiceRequest {
        project_id: "prj_1".to_owned(),
        amount: "10".to_owned(),
        currency: "USD".to_owned(),
        external_order_id: "order_1".to_owned(),
        network: None,
        allow_multiple_payments: None,
        customer_fee_percent: None,
        client_id: None,
    };
    let retry_server = TestServer::start(vec![
        Response {
            status: 429,
            headers: vec![("Retry-After", "0")],
            body: r#"{"type":"about:blank","title":"Too Many Requests","status":429,"detail":"slow down","code":"rate_limited"}"#,
        },
        Response::ok(INVOICE),
    ])
    .await;
    let client = PaymosClient::builder("pk", "sk")
        .base_url(&retry_server.base_url)
        .base_delay(Duration::ZERO)
        .max_retries(2)
        .build()
        .unwrap();
    client.invoices().create(&request).await.unwrap();
    assert_eq!(retry_server.paths().await.len(), 2);

    let no_retry_server = TestServer::start(vec![Response {
        status: 503,
        headers: Vec::new(),
        body: r#"{"type":"about:blank","title":"Service Unavailable","status":503,"detail":"retry later","code":"unavailable"}"#,
    }])
    .await;
    let client = PaymosClient::builder("pk", "sk")
        .base_url(&no_retry_server.base_url)
        .base_delay(Duration::ZERO)
        .max_retries(2)
        .build()
        .unwrap();
    let error = client.invoices().create(&request).await.unwrap_err();
    match error {
        Error::Api(error) => {
            assert_eq!(error.kind, ApiErrorKind::Unavailable);
            assert_eq!(error.code.as_deref(), Some("unavailable"));
        }
        other => panic!("expected API error, got {other:?}"),
    }
    assert_eq!(no_retry_server.paths().await.len(), 1);
}

#[tokio::test]
async fn problem_details_uses_top_level_code_and_rejects_status_mismatch() {
    const MULTI: &str = r#"{"type":"about:blank","title":"Bad Request","status":400,"detail":"Validation failed.","code":"validation_failed","errors":[{"code":"field_required","field":"address","message":"Required."}]}"#;
    const MISMATCHED: &str = r#"{"type":"about:blank","title":"Bad Request","status":401,"detail":"Mismatch.","code":"unauthorized"}"#;
    let server = TestServer::start(vec![Response {
        status: 400,
        headers: Vec::new(),
        body: MULTI,
    }])
    .await;
    let client = PaymosClient::builder("pk", "sk")
        .base_url(&server.base_url)
        .max_retries(0)
        .build()
        .unwrap();
    match client.system().time().await.unwrap_err() {
        Error::Api(error) => {
            assert_eq!(error.code.as_deref(), Some("validation_failed"));
            assert_eq!(error.field, None);
            assert_eq!(error.errors[0].code, "field_required");
        }
        other => panic!("expected API error, got {other:?}"),
    }

    let server = TestServer::start(vec![Response {
        status: 400,
        headers: Vec::new(),
        body: MISMATCHED,
    }])
    .await;
    let client = PaymosClient::builder("pk", "sk")
        .base_url(&server.base_url)
        .max_retries(0)
        .build()
        .unwrap();
    match client.system().time().await.unwrap_err() {
        Error::Api(error) => assert_eq!(error.code, None),
        other => panic!("expected API error, got {other:?}"),
    }
}

#[tokio::test]
async fn mutating_transport_failure_is_never_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let accepts = Arc::new(AtomicUsize::new(0));
    let task_accepts = accepts.clone();
    let task = tokio::spawn(async move {
        while let Ok(Ok((mut socket, _))) =
            tokio::time::timeout(Duration::from_millis(300), listener.accept()).await
        {
            task_accepts.fetch_add(1, Ordering::SeqCst);
            let _ = read_request(&mut socket).await;
            drop(socket);
        }
    });
    let client = PaymosClient::builder("pk", "sk")
        .base_url(base_url)
        .base_delay(Duration::ZERO)
        .max_retries(5)
        .build()
        .unwrap();
    let error = client
        .invoices()
        .create(&CreateInvoiceRequest {
            project_id: "prj_1".to_owned(),
            amount: "10".to_owned(),
            currency: "USD".to_owned(),
            external_order_id: "order_1".to_owned(),
            network: None,
            allow_multiple_payments: None,
            customer_fee_percent: None,
            client_id: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Transport(_)));
    task.await.unwrap();
    assert_eq!(accepts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_a_cursor_repeated_from_the_initial_request() {
    let server =
        TestServer::start(vec![Response::ok(r#"{"items":[],"next_cursor":"same"}"#)]).await;
    let client = PaymosClient::builder("pk", "sk")
        .base_url(&server.base_url)
        .max_retries(0)
        .build()
        .unwrap();
    let mut pager = client
        .invoices()
        .pager(
            InvoiceListParams {
                cursor: Some("same".to_owned()),
                status: Some(vec![InvoiceStatus::Paid]),
                ..InvoiceListParams::default()
            },
            Some(3),
        )
        .unwrap();
    assert!(matches!(pager.next().await, Err(Error::Pagination(_))));
    assert_eq!(server.paths().await.len(), 1);
}

// ---------------------------------------------------------------------------
// Payment channels.
//
// Two paginations live here on purpose. The channel list is an ordinary keyset
// page whose cursor is null on the last page, so its pager terminates. The
// deposit feed's cursor is non-nullable and arrives even on an empty page, so
// it has no pager and no test asserting that it stops — the feed has no end.
// ---------------------------------------------------------------------------

/// A provisioned rail with an address, and one still provisioning without one.
const CHANNEL: &str = r#"{"id":"pc_1","project_id":"prj_1","external_id":"customer-42","status":"active","is_accepting_payments":true,"is_fully_provisioned":false,"is_test":true,"applied_fee_percent":1.5,"customer_fee_percent":0,"networks":[{"network":"TRC20","status":"active","address":"TAddress","tokens":[{"symbol":"USDT","minimum_deposit":"1.00"}]},{"network":"BEP20","status":"provisioning","tokens":[{"symbol":"USDT"}]}],"created_at":1700000000,"updated_at":1700000000}"#;
const BLOCKED_CHANNEL: &str = r#"{"id":"pc_1","project_id":"prj_1","external_id":"customer-42","status":"blocked","is_accepting_payments":false,"is_fully_provisioned":true,"is_test":true,"applied_fee_percent":1.5,"customer_fee_percent":0,"networks":[],"created_at":1700000000,"updated_at":1700000000}"#;
const DEPOSIT: &str = r#"{"id":"pcd_1","payment_channel_id":"pc_1","project_id":"prj_1","payment_channel_external_id":"customer-42","status":"confirmed","is_final":true,"is_test":true,"currency":"USDT","network":"TRC20","chain_id":728126428,"gross":"100.00","fee":"1.50","net":"98.50","applied_fee_percent":1.5,"customer_fee_percent":0,"tx_hash":"0xhash","transfer_id":"trf_1","destination_address":"TAddress","block_height":12345,"created_at":1700000000,"updated_at":1700000000,"confirmed_at":1700000100}"#;

fn test_client(base_url: &str) -> PaymosClient {
    PaymosClient::builder("pk_test_key", "sk_test_secret")
        .base_url(base_url)
        .max_retries(0)
        .build()
        .unwrap()
}

fn create_channel_request() -> CreatePaymentChannelRequest {
    CreatePaymentChannelRequest {
        project_id: "prj_1".to_owned(),
        external_id: "customer-42".to_owned(),
    }
}

const fn created(body: &'static str) -> Response {
    Response {
        status: 201,
        headers: Vec::new(),
        body,
    }
}

#[tokio::test]
async fn payment_channel_matrix_uses_the_frozen_routes() {
    let server = TestServer::start(vec![
        created(CHANNEL),
        Response::ok(CHANNEL),
        Response::ok(r#"{"items":[],"next_cursor":null}"#),
        Response::ok(BLOCKED_CHANNEL),
        Response::ok(CHANNEL),
        Response::ok(DEPOSIT),
        Response::ok(DEPOSIT),
        Response::ok(r#"{"items":[],"next_cursor":"cur_1"}"#),
    ])
    .await;
    let client = test_client(&server.base_url);

    client
        .payment_channels()
        .create(&create_channel_request())
        .await
        .unwrap();
    client.payment_channels().get("pc_1").await.unwrap();
    client
        .payment_channels()
        .list(&PaymentChannelListParams::default())
        .await
        .unwrap();
    client.payment_channels().block("pc_1").await.unwrap();
    client.payment_channels().unblock("pc_1").await.unwrap();
    client
        .payment_channels()
        .simulate_deposit(
            "pc_1",
            &SimulateDepositRequest {
                amount: "100.00".to_owned(),
                currency: "USDT".to_owned(),
                network: "TRC20".to_owned(),
                stage: None,
            },
        )
        .await
        .unwrap();
    client
        .payment_channel_deposits()
        .get("pcd_1")
        .await
        .unwrap();
    client
        .payment_channel_deposits()
        .read(&PaymentChannelDepositFeedParams::default())
        .await
        .unwrap();

    assert_eq!(
        server.paths().await,
        vec![
            "POST /v1/payment-channels",
            "GET /v1/payment-channels/pc_1",
            "GET /v1/payment-channels",
            "POST /v1/payment-channels/pc_1/block",
            "POST /v1/payment-channels/pc_1/unblock",
            "POST /v1/sandbox/payment-channels/pc_1/simulate-deposit",
            "GET /v1/payment-channel-deposits/pcd_1",
            "GET /v1/payment-channel-deposits",
        ]
    );
}

/// Creation is idempotent on `project_id` + `external_id`: the replay answers
/// 200 with the channel that already exists. An SDK that treats that as an
/// error breaks every checkout after the first.
#[tokio::test]
async fn creating_the_same_channel_twice_returns_the_channel_both_times() {
    let server = TestServer::start(vec![created(CHANNEL), Response::ok(CHANNEL)]).await;
    let client = test_client(&server.base_url);
    let request = create_channel_request();

    let fresh = client.payment_channels().create(&request).await.unwrap();
    let replay = client.payment_channels().create(&request).await.unwrap();

    assert_eq!(fresh.id, "pc_1");
    assert_eq!(replay.id, "pc_1");
    assert_eq!(server.paths().await.len(), 2);
}

/// Absent is not null and not a zero value: an unprovisioned rail has no
/// address, and a token without a quotable minimum has no minimum deposit.
#[tokio::test]
async fn absent_wire_members_arrive_as_none() {
    let server = TestServer::start(vec![Response::ok(CHANNEL)]).await;
    let client = test_client(&server.base_url);

    let channel = client.payment_channels().get("pc_1").await.unwrap();

    assert!(!channel.is_fully_provisioned);
    assert_eq!(channel.networks[0].address.as_deref(), Some("TAddress"));
    assert_eq!(
        channel.networks[0].tokens[0].minimum_deposit.as_deref(),
        Some("1.00")
    );
    assert!(channel.networks[1].address.is_none());
    assert!(channel.networks[1].tokens[0].minimum_deposit.is_none());
}

/// Block is not a void call.
#[tokio::test]
async fn block_returns_the_updated_channel() {
    let server = TestServer::start(vec![Response::ok(BLOCKED_CHANNEL)]).await;
    let client = test_client(&server.base_url);

    let channel = client.payment_channels().block("pc_1").await.unwrap();

    assert!(!channel.is_accepting_payments);
    assert_eq!(channel.status, "blocked");
}

/// The mirror of the block test: only asserting one direction has let the
/// other be a no-op before.
#[tokio::test]
async fn unblock_returns_the_updated_channel() {
    let server = TestServer::start(vec![Response::ok(CHANNEL)]).await;
    let client = test_client(&server.base_url);

    let channel = client.payment_channels().unblock("pc_1").await.unwrap();

    assert!(channel.is_accepting_payments);
    assert_eq!(channel.status, "active");
}

/// The test that fails if someone modelled the feed on the invoice list: feed
/// positions are global, so a page that matched nothing still advances past
/// everything this merchant's filter skipped.
#[tokio::test]
async fn an_empty_feed_page_still_yields_a_cursor_to_resume_from() {
    let server =
        TestServer::start(vec![Response::ok(r#"{"items":[],"next_cursor":"cur_2"}"#)]).await;
    let client = test_client(&server.base_url);

    let page = client
        .payment_channel_deposits()
        .read(&PaymentChannelDepositFeedParams {
            cursor: Some("cur_1".to_owned()),
            ..PaymentChannelDepositFeedParams::default()
        })
        .await
        .unwrap();

    assert!(page.items.is_empty());
    assert_eq!(page.next_cursor, "cur_2");
    assert!(page.blocked.is_none());
}

/// Without `blocked`, a stuck feed is byte-identical to a quiet day.
#[tokio::test]
async fn a_blocked_feed_page_surfaces_the_deposit_and_the_reason() {
    let server = TestServer::start(vec![Response::ok(
        r#"{"items":[],"next_cursor":"cur_2","blocked":{"deposit_id":"pcd_9","reason":"not_confirmed"}}"#,
    )])
    .await;
    let client = test_client(&server.base_url);

    let page = client
        .payment_channel_deposits()
        .read(&PaymentChannelDepositFeedParams::default())
        .await
        .unwrap();

    let blockage = page.blocked.expect("blocked must reach the caller");
    assert_eq!(blockage.deposit_id, "pcd_9");
    assert_eq!(blockage.reason, "not_confirmed");
    assert_eq!(page.next_cursor, "cur_2");
}

/// The API rejects unknown query fields outright, so a camelCase slip is a 400
/// at a merchant and a green test here if the wire is not asserted.
#[tokio::test]
async fn every_filter_reaches_the_wire_in_snake_case() {
    let server = TestServer::start(vec![
        Response::ok(r#"{"items":[],"next_cursor":null}"#),
        Response::ok(r#"{"items":[],"next_cursor":"cur_2"}"#),
    ])
    .await;
    let client = test_client(&server.base_url);

    client
        .payment_channels()
        .list(&PaymentChannelListParams {
            limit: Some(5),
            cursor: Some("cur_1".to_owned()),
            status: Some(vec!["active".to_owned(), "blocked".to_owned()]),
            external_id: Some("customer-42".to_owned()),
            project_id: Some("prj_1".to_owned()),
        })
        .await
        .unwrap();
    client
        .payment_channel_deposits()
        .read(&PaymentChannelDepositFeedParams {
            limit: Some(2),
            cursor: Some("cur_1".to_owned()),
            project_id: Some("prj_1".to_owned()),
            payment_channel_id: Some("pc_1".to_owned()),
            confirmed_from: Some(1_700_000_000),
        })
        .await
        .unwrap();

    assert_eq!(
        server.paths().await,
        vec![
            "GET /v1/payment-channels?cursor=cur_1&external_id=customer-42&limit=5&project_id=prj_1&status=active&status=blocked",
            "GET /v1/payment-channel-deposits?confirmed_from=1700000000&cursor=cur_1&limit=2&payment_channel_id=pc_1&project_id=prj_1",
        ]
    );
}

/// A space is `%20` in a path and `+` in a form-encoded query: the ids go
/// through the RFC 3986 path encoder the signature is computed over.
#[tokio::test]
async fn identifiers_are_path_encoded_not_query_encoded() {
    let server = TestServer::start(vec![Response::ok(CHANNEL), Response::ok(DEPOSIT)]).await;
    let client = test_client(&server.base_url);

    client.payment_channels().get("pc 1/2").await.unwrap();
    client
        .payment_channel_deposits()
        .get("pcd 1/2")
        .await
        .unwrap();

    assert_eq!(
        server.paths().await,
        vec![
            "GET /v1/payment-channels/pc%201%2F2",
            "GET /v1/payment-channel-deposits/pcd%201%2F2",
        ]
    );
}

/// `network` is required, and an unset stage is omitted from the body
/// entirely — not `null`, which the server reads as an unknown value.
#[tokio::test]
async fn simulate_sends_the_network_and_omits_an_unset_stage() {
    let server = TestServer::start(vec![Response::ok(DEPOSIT), Response::ok(DEPOSIT)]).await;
    let client = test_client(&server.base_url);

    client
        .payment_channels()
        .simulate_deposit(
            "pc_1",
            &SimulateDepositRequest {
                amount: "100.00".to_owned(),
                currency: "USDT".to_owned(),
                network: "TRC20".to_owned(),
                stage: None,
            },
        )
        .await
        .unwrap();
    client
        .payment_channels()
        .simulate_deposit(
            "pc_1",
            &SimulateDepositRequest {
                amount: "100.00".to_owned(),
                currency: "USDT".to_owned(),
                network: "TRC20".to_owned(),
                stage: Some(SimulateDepositStage::Confirming),
            },
        )
        .await
        .unwrap();

    let requests = server.raw_requests().await;
    let default_stage = request_body(&requests[0]);
    assert_eq!(default_stage["amount"], "100.00");
    assert_eq!(default_stage["currency"], "USDT");
    assert_eq!(default_stage["network"], "TRC20");
    assert!(default_stage.get("stage").is_none());
    assert_eq!(request_body(&requests[1])["stage"], "confirming");
}

/// The channel list IS an ordinary keyset page, so its pager stops on a null
/// cursor. The deposit feed deliberately has no pager to stop.
#[tokio::test]
async fn the_channel_pager_terminates_on_a_null_cursor() {
    let server = TestServer::start(vec![
        Response::ok(
            r#"{"items":[{"id":"pc_1","project_id":"prj_1","external_id":"customer-42","status":"active","is_accepting_payments":true,"is_fully_provisioned":true,"is_test":true,"applied_fee_percent":1.5,"customer_fee_percent":0,"networks":[],"created_at":1700000000,"updated_at":1700000000}],"next_cursor":"cur_2"}"#,
        ),
        Response::ok(
            r#"{"items":[{"id":"pc_2","project_id":"prj_1","external_id":"customer-43","status":"active","is_accepting_payments":true,"is_fully_provisioned":true,"is_test":true,"applied_fee_percent":1.5,"customer_fee_percent":0,"networks":[],"created_at":1700000000,"updated_at":1700000000}],"next_cursor":null}"#,
        ),
    ])
    .await;
    let client = test_client(&server.base_url);
    let mut pager = client
        .payment_channels()
        .pager(PaymentChannelListParams::default(), Some(10))
        .unwrap();

    let first = pager.next().await.unwrap().unwrap();
    let second = pager.next().await.unwrap().unwrap();

    assert_eq!(first.id, "pc_1");
    assert_eq!(second.id, "pc_2");
    assert!(pager.next().await.unwrap().is_none());
    assert_eq!(
        server.paths().await,
        vec![
            "GET /v1/payment-channels",
            "GET /v1/payment-channels?cursor=cur_2",
        ]
    );
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut buffer).await.unwrap();
        if read == 0 {
            return String::from_utf8_lossy(&bytes).into_owned();
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(position) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = socket.read(&mut buffer).await.unwrap();
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
