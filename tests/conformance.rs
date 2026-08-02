//! Cross-language contract, route, retry, pagination, and webhook tests.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use paymos::signing::{authorization_header, build_query, sign, string_to_sign};
use paymos::{
    ApiErrorKind, CreateInvoiceRequest, CreateWithdrawalRequest, Error, InvoiceListParams,
    InvoiceSimulationStage, InvoiceStatus, PaymosClient, WebhookError, WebhookEvent,
    WebhookVerifier, WithdrawalListParams,
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

    async fn paths(mut self) -> Vec<String> {
        self.task.await.unwrap();
        let mut paths = Vec::new();
        while let Some(request) = self.requests.recv().await {
            let line = request.lines().next().unwrap();
            let mut parts = line.split_whitespace();
            paths.push(format!(
                "{} {}",
                parts.next().unwrap(),
                parts.next().unwrap()
            ));
        }
        paths
    }
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
