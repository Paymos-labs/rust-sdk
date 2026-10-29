//! Refund requests and responses through the public HTTP client.

use std::time::Duration;

use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use paymos::{
    ApiError, ApiErrorKind, Error, InvoiceSimulationStage, PaymosClient, Refund, RefundListParams,
    RefundMode, WithdrawalStatus,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const INVOICE_ID: &str = "inv_74BPZFhr9qy9Uz2fbRkdJX";
const REFUND_ID: &str = "wdr_74BPZFhr9qy9Uz2fbRkdJX";

struct Response {
    status: u16,
    body: Value,
}

impl Response {
    const fn new(status: u16, body: Value) -> Self {
        Self { status, body }
    }
}

struct TestServer {
    base_url: String,
    requests: mpsc::Receiver<String>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(responses: Vec<Option<Response>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel(responses.len().max(1));
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                sender.send(read_request(&mut socket).await).await.unwrap();
                let Some(response) = response else {
                    continue;
                };
                let body = response.body.to_string();
                let content_type = if response.status >= 400 {
                    "application/problem+json"
                } else {
                    "application/json"
                };
                let headers = format!(
                    "HTTP/1.1 {} Stub\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: private, no-store\r\nRetry-After: 0\r\nConnection: close\r\n\r\n",
                    response.status,
                    body.len()
                );
                socket.write_all(headers.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
            }
        });
        Self {
            base_url,
            requests,
            task,
        }
    }

    async fn raw_requests(mut self) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("all expected HTTP requests")
            .unwrap();
        let mut requests = Vec::new();
        while let Some(request) = self.requests.recv().await {
            requests.push(request);
        }
        requests
    }
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut buffer).await.unwrap();
        assert_ne!(read, 0, "request headers");
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
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = socket.read(&mut buffer).await.unwrap();
        assert_ne!(read, 0, "request body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(bytes).unwrap()
}

fn client(server: &TestServer) -> PaymosClient {
    PaymosClient::builder("pk_test_key", "sk_test_secret")
        .base_url(&server.base_url)
        .timeout(Duration::from_secs(2))
        .base_delay(Duration::ZERO)
        .max_retries(2)
        .build()
        .unwrap()
}

fn vectors() -> Value {
    let contract: Value =
        serde_json::from_str(include_str!("../conformance/contract.json")).unwrap();
    contract["vectors"]["refunds"].clone()
}

fn refund(id: &str, mode: &str, amount: &str, status: &str) -> Value {
    json!({
        "withdrawal_id": id, "invoice_id": INVOICE_ID, "mode": mode,
        "external_order_id": if mode == "overpayment" { "refund-order-2" } else { "refund-order-1" },
        "status": status,
        "is_final": status == "completed", "is_test": true,
        "amount": amount, "fee": "0.000001", "currency": "USDT", "network": "TRC20",
        "first_sender": "TFirstSender", "created_at": 1_700_000_000,
        "completed_at": if status == "completed" { Some(1_700_000_060) } else { None },
        "tx_hash": if status == "completed" { Some("tx-refund") } else { None }
    })
}

fn problem(status: u16, code: &str, field: Option<&str>) -> Value {
    json!({
        "type": "about:blank", "title": "Refund request failed", "status": status,
        "detail": "The refund request was rejected.", "code": code, "field": field
    })
}

fn api_error(error: Error, status: u16, kind: ApiErrorKind, code: &str) -> Box<ApiError> {
    let Error::Api(error) = error else {
        panic!("expected structured API error, got {error:?}");
    };
    assert_eq!(error.status, status);
    assert_eq!(error.kind, kind);
    assert_eq!(error.code.as_deref(), Some(code));
    assert_eq!(error.problem_type.as_deref(), Some("about:blank"));
    assert!(error.detail.is_some());
    error
}

fn header<'a>(request: &'a str, name: &str) -> &'a str {
    request
        .split_once("\r\n\r\n")
        .unwrap()
        .0
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
        .unwrap()
}

fn assert_request(request: &str, method: &str, target: &str, expected_body: &Value) {
    assert_eq!(
        request.lines().next().unwrap(),
        format!("{method} {target} HTTP/1.1")
    );
    let (_, body) = request.split_once("\r\n\r\n").unwrap();
    if expected_body.is_null() {
        assert!(body.is_empty());
    } else {
        assert_eq!(serde_json::from_str::<Value>(body).unwrap(), *expected_body);
        assert_eq!(header(request, "content-type"), "application/json");
    }
    let timestamp = header(request, "x-request-timestamp");
    assert!(timestamp.bytes().all(|byte| byte.is_ascii_digit()));
    assert!(timestamp.parse::<u64>().unwrap() > 0);
    let (path, query) = target
        .find('?')
        .map_or((target, ""), |i| (&target[..i], &target[i..]));
    let hash = if body.is_empty() {
        String::new()
    } else {
        hex::encode(Sha256::digest(body.as_bytes()))
    };
    let canonical = format!("{timestamp}\n{method}\n{path}\n{query}\n{hash}");
    let mut mac = Hmac::<Sha256>::new_from_slice(b"sk_test_secret").unwrap();
    mac.update(canonical.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    assert_eq!(
        header(request, "authorization"),
        format!("HMAC-SHA256 pk_test_key:{signature}")
    );
}

fn assert_refund(result: &Refund, id: &str, mode: RefundMode, amount: &str) {
    assert_eq!(result.withdrawal_id(), id);
    assert_eq!(result.invoice_id(), INVOICE_ID);
    assert_eq!(result.mode(), mode);
    assert_eq!(
        result.external_order_id(),
        Some(if mode == RefundMode::Overpayment {
            "refund-order-2"
        } else {
            "refund-order-1"
        })
    );
    assert_eq!(result.amount(), amount);
    assert_eq!(result.currency(), "USDT");
    assert_eq!(result.network(), "TRC20");
    assert_eq!(result.first_sender(), "TFirstSender");
    assert_eq!(result.destination_address(), None);
    assert_eq!(result.refund_type(), None);
    assert_eq!(result.project_id(), None);
    assert_eq!(result.fee(), Some("0.000001"));
    assert_eq!(result.created_at(), 1_700_000_000);
    assert!(result.is_test());
}

#[tokio::test]
async fn simulate_terminal_statuses_preserves_signed_request_and_current_refund_fields() {
    const ADDRESS: &str = "UQDKbjIcfM6ezt8KjKJJLshZJJSqX7XOA4ff-W72r5gqPuwA";
    let statuses = ["completed", "failed", "cancelled"];
    let responses = statuses
        .iter()
        .map(|status| {
            let mut wire = refund("wdr_1", "full", "20", status);
            wire["is_final"] = json!(true);
            wire["network"] = json!("TON");
            wire["destination_address"] = json!(ADDRESS);
            wire["first_sender"] = json!(ADDRESS);
            wire["type"] = json!("refund");
            wire["project_id"] = json!("prj_1");
            Some(Response::new(200, wire))
        })
        .collect();
    let server = TestServer::start(responses).await;
    let client = client(&server);
    for status in statuses {
        let result = client.refunds().simulate("wdr/1 ?", status).await.unwrap();
        assert_eq!(result.status().as_str(), status);
        assert_eq!(result.destination_address(), Some(ADDRESS));
        assert_eq!(result.first_sender(), ADDRESS);
        assert_eq!(result.refund_type(), Some("refund"));
        assert_eq!(result.project_id(), Some("prj_1"));
    }
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), statuses.len());
    for (request, status) in requests.iter().zip(statuses) {
        assert_request(
            request,
            "POST",
            "/v1/sandbox/refunds/wdr%2F1%20%3F/simulate",
            &json!({"status": status}),
        );
    }
}

#[tokio::test]
async fn full_create_and_same_key_replay_use_only_the_signed_contract_body() {
    let vector = vectors();
    let wire = refund(REFUND_ID, "full", "120", "created");
    let server = TestServer::start(vec![
        Some(Response::new(201, wire.clone())),
        Some(Response::new(200, wire)),
    ])
    .await;
    let client = client(&server);
    for _ in 0..2 {
        let result = client
            .refunds()
            .create(INVOICE_ID, RefundMode::Full, "refund-order-1")
            .await
            .unwrap();
        assert_refund(&result, REFUND_ID, RefundMode::Full, "120");
        assert_eq!(result.status(), &WithdrawalStatus::Created);
        assert_eq!(result.external_order_id(), Some("refund-order-1"));
        assert!(!result.is_final());
    }
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_request(
            &request,
            vector["create"]["method"].as_str().unwrap(),
            vector["create"]["path"].as_str().unwrap(),
            &vector["create"]["body"],
        );
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        for field in vector["create"]["rejected_fields"].as_array().unwrap() {
            assert!(body.get(field.as_str().unwrap()).is_none());
        }
    }
}

#[tokio::test]
async fn overpayment_twenty_then_remaining_full_hundred_with_idempotent_sandbox_completion() {
    let over_created = refund(REFUND_ID, "overpayment", "20", "created");
    let over_completed = refund(REFUND_ID, "overpayment", "20", "completed");
    let full_id = "wdr_full_remaining";
    let full_created = refund(full_id, "full", "100", "created");
    let full_completed = refund(full_id, "full", "100", "completed");
    let server = TestServer::start(vec![
        Some(Response::new(201, over_created)),
        Some(Response::new(200, over_completed.clone())),
        Some(Response::new(200, over_completed)),
        Some(Response::new(201, full_created)),
        Some(Response::new(200, full_completed.clone())),
        Some(Response::new(200, full_completed)),
    ])
    .await;
    let client = client(&server);
    let over = client
        .refunds()
        .create(INVOICE_ID, RefundMode::Overpayment, "refund-order-2")
        .await
        .unwrap();
    assert_refund(&over, REFUND_ID, RefundMode::Overpayment, "20");
    for _ in 0..2 {
        let completed = client
            .refunds()
            .simulate_completion(over.withdrawal_id())
            .await
            .unwrap();
        assert_refund(&completed, REFUND_ID, RefundMode::Overpayment, "20");
        assert!(completed.is_final());
        assert_eq!(completed.status(), &WithdrawalStatus::Completed);
        assert_eq!(completed.completed_at(), Some(1_700_000_060));
        assert_eq!(completed.tx_hash(), Some("tx-refund"));
    }
    let full = client
        .refunds()
        .create(INVOICE_ID, RefundMode::Full, "refund-order-1")
        .await
        .unwrap();
    assert_refund(&full, full_id, RefundMode::Full, "100");
    let completed = client
        .refunds()
        .simulate_completion(full.withdrawal_id())
        .await
        .unwrap();
    assert_eq!(completed.status(), &WithdrawalStatus::Completed);
    let fetched = client.refunds().get(full.withdrawal_id()).await.unwrap();
    assert_refund(&fetched, full_id, RefundMode::Full, "100");
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 6);
    let vector = vectors();
    assert_request(
        &requests[0],
        "POST",
        "/v1/refunds",
        &vector["overpayment"]["body"],
    );
    for request in &requests[1..3] {
        assert_request(
            request,
            "POST",
            &format!("/v1/sandbox/refunds/{REFUND_ID}/simulate"),
            &json!({"status": "completed"}),
        );
    }
    assert_request(
        &requests[3],
        "POST",
        "/v1/refunds",
        &vector["create"]["body"],
    );
    assert_request(
        &requests[4],
        "POST",
        &format!("/v1/sandbox/refunds/{full_id}/simulate"),
        &json!({"status": "completed"}),
    );
    assert_request(
        &requests[5],
        "GET",
        &format!("/v1/refunds/{full_id}"),
        &Value::Null,
    );
}

#[tokio::test]
async fn sandbox_paid_over_120_for_order_100_returns_server_full_120() {
    let invoice = json!({
        "invoice_id": INVOICE_ID, "project_id": "prj_1", "status": "paid_over",
        "is_final": true, "is_test": true, "payment_url": "https://pay.test/invoice",
        "order": {"external_id": "order-1", "amount": "100", "currency": "USDT"},
        "payment": {"currency": "USDT", "network": "TRC20", "chain_id": 728_126_428, "expected": "100", "paid": "120"},
        "created_at": 1_700_000_000, "updated_at": 1_700_000_060
    });
    let server = TestServer::start(vec![
        Some(Response::new(200, invoice)),
        Some(Response::new(
            201,
            refund(REFUND_ID, "full", "120", "created"),
        )),
    ])
    .await;
    let client = client(&server);
    let invoice = client
        .invoices()
        .simulate_payment(INVOICE_ID, InvoiceSimulationStage::Overpaid)
        .await
        .unwrap();
    assert_eq!(invoice.order.amount, "100");
    assert_eq!(invoice.payment.unwrap().paid.as_deref(), Some("120"));
    let result = client
        .refunds()
        .create(&invoice.invoice_id, RefundMode::Full, "refund-order-1")
        .await
        .unwrap();
    assert_refund(&result, REFUND_ID, RefundMode::Full, "120");
    let requests = server.raw_requests().await;
    assert_request(
        &requests[0],
        "POST",
        &format!("/v1/sandbox/invoices/{INVOICE_ID}/simulate-payment"),
        &json!({"stage": "overpaid"}),
    );
    assert_request(
        &requests[1],
        "POST",
        "/v1/refunds",
        &vectors()["create"]["body"],
    );
}

#[tokio::test]
async fn changed_mode_for_the_same_key_is_a_structured_409() {
    let server = TestServer::start(vec![
        Some(Response::new(
            201,
            refund(REFUND_ID, "full", "120", "created"),
        )),
        Some(Response::new(
            409,
            problem(409, "conflict", Some("external_order_id")),
        )),
    ])
    .await;
    let client = client(&server);
    client
        .refunds()
        .create(INVOICE_ID, RefundMode::Full, "refund-order-1")
        .await
        .unwrap();
    let error = api_error(
        client
            .refunds()
            .create(INVOICE_ID, RefundMode::Overpayment, "refund-order-1")
            .await
            .unwrap_err(),
        409,
        ApiErrorKind::Conflict,
        "conflict",
    );
    assert_eq!(error.field.as_deref(), Some("external_order_id"));
    assert!(error.errors.is_empty());
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 2);
    let mut changed = vectors()["create"]["body"].clone();
    changed["mode"] = json!("overpayment");
    assert_request(
        &requests[0],
        "POST",
        "/v1/refunds",
        &vectors()["create"]["body"],
    );
    assert_request(&requests[1], "POST", "/v1/refunds", &changed);
}

#[tokio::test]
async fn unknown_transport_outcome_is_replayed_explicitly_with_the_same_key() {
    let server = TestServer::start(vec![
        None,
        Some(Response::new(
            200,
            refund(REFUND_ID, "full", "120", "created"),
        )),
    ])
    .await;
    let client = client(&server);
    assert!(matches!(
        client
            .refunds()
            .create(INVOICE_ID, RefundMode::Full, "refund-order-1")
            .await,
        Err(Error::Transport(_))
    ));
    let result = client
        .refunds()
        .create(INVOICE_ID, RefundMode::Full, "refund-order-1")
        .await
        .unwrap();
    assert_refund(&result, REFUND_ID, RefundMode::Full, "120");
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_request(
            &request,
            "POST",
            "/v1/refunds",
            &vectors()["create"]["body"],
        );
    }
}

#[tokio::test]
async fn server_failure_does_not_retry_a_refund_post() {
    let server = TestServer::start(vec![Some(Response::new(
        503,
        problem(503, "unavailable", None),
    ))])
    .await;
    let client = client(&server);
    api_error(
        client
            .refunds()
            .create(INVOICE_ID, RefundMode::Full, "refund-order-1")
            .await
            .unwrap_err(),
        503,
        ApiErrorKind::Unavailable,
        "unavailable",
    );
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 1);
    assert_request(
        &requests[0],
        "POST",
        "/v1/refunds",
        &vectors()["create"]["body"],
    );
}

#[tokio::test]
async fn rate_limit_retry_preserves_the_mode_and_idempotency_key() {
    let server = TestServer::start(vec![
        Some(Response::new(429, problem(429, "rate_limited", None))),
        Some(Response::new(
            201,
            refund(REFUND_ID, "full", "120", "created"),
        )),
    ])
    .await;
    let client = client(&server);
    client
        .refunds()
        .create(INVOICE_ID, RefundMode::Full, "refund-order-1")
        .await
        .unwrap();
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_request(
            &request,
            "POST",
            "/v1/refunds",
            &vectors()["create"]["body"],
        );
    }
}

#[tokio::test]
async fn list_preserves_filters_and_opaque_cursors_across_100_100_5_items() {
    let vector = vectors();
    let sizes = vector["list"]["page_sizes"].as_array().unwrap();
    let cursors = [Some("page/+?1"), Some("page/+?2"), None];
    let mut responses = Vec::new();
    let mut offset = 0;
    for (size, cursor) in sizes.iter().zip(cursors) {
        let size = size.as_u64().unwrap();
        let items: Vec<_> = (offset..offset + size)
            .map(|i| refund(&format!("wdr_{i}"), "full", "120", "completed"))
            .collect();
        responses.push(Some(Response::new(
            200,
            json!({"items": items, "next_cursor": cursor}),
        )));
        offset += size;
    }
    let server = TestServer::start(responses).await;
    let client = client(&server);
    let mut params = RefundListParams {
        invoice_id: Some(INVOICE_ID.to_owned()),
        status: Some(vec![
            WithdrawalStatus::PendingReview,
            WithdrawalStatus::Completed,
        ]),
        created_from: Some("2026-10-01T00:00:00+03:00".to_owned()),
        created_to: Some("2026-11-01T00:00:00Z".to_owned()),
        limit: Some(100),
        cursor: None,
    };
    let mut ids = Vec::new();
    for (size, cursor) in sizes.iter().zip(cursors) {
        let page = client.refunds().list(&params).await.unwrap();
        assert_eq!(page.items.len() as u64, size.as_u64().unwrap());
        assert_eq!(page.next_cursor.as_deref(), cursor);
        for item in page.items {
            assert_eq!(item.invoice_id(), INVOICE_ID);
            assert_eq!(item.amount(), "120");
            ids.push(item.withdrawal_id().to_owned());
        }
        params.cursor = page.next_cursor;
    }
    assert_eq!(
        ids,
        (0..205).map(|i| format!("wdr_{i}")).collect::<Vec<_>>()
    );
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 3);
    for (request, cursor) in
        requests
            .iter()
            .zip(["", "&cursor=page%2F%2B%3F1", "&cursor=page%2F%2B%3F2"])
    {
        let target = format!("/v1/refunds?created_from=2026-10-01T00%3A00%3A00%2B03%3A00&created_to=2026-11-01T00%3A00%3A00Z{cursor}&invoice_id={INVOICE_ID}&limit=100&status=completed&status=pending_review");
        assert_request(request, "GET", &target, &Value::Null);
    }
}

#[tokio::test]
async fn get_uses_the_current_withdrawal_id_and_preserves_decimal_precision() {
    let amount = "120.123456789123456789";
    let wire = refund(REFUND_ID, "full", amount, "completed");
    for field in vectors()["wire"]["required_fields"].as_array().unwrap() {
        assert!(wire.get(field.as_str().unwrap()).is_some());
    }
    let server = TestServer::start(vec![Some(Response::new(200, wire))]).await;
    let result = client(&server).refunds().get(REFUND_ID).await.unwrap();
    assert_refund(&result, REFUND_ID, RefundMode::Full, amount);
    assert_eq!(result.explorer_url(), None);
    assert_eq!(result.failed_at(), None);
    assert_eq!(result.cancelled_at(), None);
    let requests = server.raw_requests().await;
    assert_request(
        &requests[0],
        "GET",
        &format!("/v1/refunds/{REFUND_ID}"),
        &Value::Null,
    );
}

#[tokio::test]
async fn foreign_and_missing_refund_ids_return_structured_404() {
    let server = TestServer::start(
        (0..2)
            .map(|_| {
                Some(Response::new(
                    404,
                    problem(404, "withdrawal_not_found", None),
                ))
            })
            .collect(),
    )
    .await;
    let client = client(&server);
    for id in ["wdr_foreign", "wdr_missing"] {
        api_error(
            client.refunds().get(id).await.unwrap_err(),
            404,
            ApiErrorKind::NotFound,
            "withdrawal_not_found",
        );
    }
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 2);
    assert_request(&requests[0], "GET", "/v1/refunds/wdr_foreign", &Value::Null);
    assert_request(&requests[1], "GET", "/v1/refunds/wdr_missing", &Value::Null);
}

#[tokio::test]
async fn invalid_cursor_and_multi_validation_keep_top_level_problem_details() {
    let mut multi = problem(400, "validation_failed", None);
    multi["errors"] = json!([
        {"code": "field_required", "field": "invoice_id", "message": "Required."},
        {"code": "field_required", "field": "external_order_id", "message": "Required."}
    ]);
    let server = TestServer::start(vec![
        Some(Response::new(
            400,
            problem(400, "pagination_cursor_invalid", Some("cursor")),
        )),
        Some(Response::new(400, multi)),
    ])
    .await;
    let client = client(&server);
    let error = api_error(
        client
            .refunds()
            .list(&RefundListParams {
                cursor: Some("invalid/+".to_owned()),
                ..RefundListParams::default()
            })
            .await
            .unwrap_err(),
        400,
        ApiErrorKind::Validation,
        "pagination_cursor_invalid",
    );
    assert_eq!(error.field.as_deref(), Some("cursor"));
    let error = api_error(
        client
            .refunds()
            .create("", RefundMode::Full, "")
            .await
            .unwrap_err(),
        400,
        ApiErrorKind::Validation,
        "validation_failed",
    );
    assert_eq!(error.field, None);
    assert_eq!(error.errors.len(), 2);
    assert_eq!(error.errors[0].code, "field_required");
    assert_eq!(error.errors[1].field.as_deref(), Some("external_order_id"));
    let requests = server.raw_requests().await;
    assert_request(
        &requests[0],
        "GET",
        "/v1/refunds?cursor=invalid%2F%2B",
        &Value::Null,
    );
    assert_request(
        &requests[1],
        "POST",
        "/v1/refunds",
        &json!({"invoice_id": "", "mode": "full", "external_order_id": ""}),
    );
}

#[tokio::test]
async fn sandbox_scope_errors_are_not_retried_or_rerouted() {
    let server = TestServer::start(vec![
        Some(Response::new(403, problem(403, "not_sandbox", None))),
        Some(Response::new(
            404,
            problem(404, "withdrawal_not_found", None),
        )),
    ])
    .await;
    let client = client(&server);
    api_error(
        client
            .refunds()
            .simulate_completion(REFUND_ID)
            .await
            .unwrap_err(),
        403,
        ApiErrorKind::Permission,
        "not_sandbox",
    );
    api_error(
        client
            .refunds()
            .simulate_completion("wdr_foreign")
            .await
            .unwrap_err(),
        404,
        ApiErrorKind::NotFound,
        "withdrawal_not_found",
    );
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 2);
    assert_request(
        &requests[0],
        "POST",
        &format!("/v1/sandbox/refunds/{REFUND_ID}/simulate"),
        &json!({"status": "completed"}),
    );
    assert_request(
        &requests[1],
        "POST",
        "/v1/sandbox/refunds/wdr_foreign/simulate",
        &json!({"status": "completed"}),
    );
}

#[tokio::test]
async fn rejects_numeric_money_in_a_successful_response() {
    let mut wire = refund(REFUND_ID, "full", "120", "created");
    wire["amount"] = json!(120);
    let server = TestServer::start(vec![Some(Response::new(201, wire))]).await;
    assert!(matches!(
        client(&server)
            .refunds()
            .create(INVOICE_ID, RefundMode::Full, "refund-order-1")
            .await,
        Err(Error::InvalidResponse(_))
    ));
    let requests = server.raw_requests().await;
    assert_request(
        &requests[0],
        "POST",
        "/v1/refunds",
        &vectors()["create"]["body"],
    );
}

#[tokio::test]
async fn validates_empty_ids_and_statuses_and_encodes_path_segments() {
    let server = TestServer::start(vec![Some(Response::new(
        200,
        refund(REFUND_ID, "full", "120", "created"),
    ))])
    .await;
    let client = client(&server);
    assert!(matches!(
        client.refunds().get(" ").await,
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        client.refunds().simulate_completion("").await,
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        client
            .refunds()
            .list(&RefundListParams {
                status: Some(vec![]),
                ..RefundListParams::default()
            })
            .await,
        Err(Error::InvalidArgument(_))
    ));
    client.refunds().get("wdr/a ?").await.unwrap();
    let requests = server.raw_requests().await;
    assert_eq!(requests.len(), 1);
    assert_request(
        &requests[0],
        "GET",
        "/v1/refunds/wdr%2Fa%20%3F",
        &Value::Null,
    );
}
