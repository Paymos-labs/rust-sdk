//! invoice-late-payment wire contract (late-26): the Invoice model decodes `paid_late_at` (Unix
//! seconds | null | absent legacy) beside an unchanged terminal status and payment.paid, and the
//! `paid_late` simulate stage serializes verbatim.

use paymos::{Invoice, InvoiceSimulationStage, InvoiceStatus};

const EXPIRED: &str = r#"{
    "invoice_id": "inv_1",
    "project_id": "prj_1",
    "status": "expired",
    "is_final": true,
    "is_test": false,
    "payment_url": "https://pay.test/inv_1",
    "order": {"external_id": "order_1", "amount": "100.00", "currency": "USD"},
    "payment": {"currency": "USDT", "network": "tron", "chain_id": 728126428, "expected": "100.00", "paid": "60.00"},
    "created_at": 1700000000,
    "updated_at": 1700003600,
    "expires_at": 1700003600,
    "completed_at": 1700003600
}"#;

#[test]
fn decodes_unix_mark_beside_terminal_status() {
    let marked = format!(
        "{},\"paid_late_at\":1700007200}}",
        &EXPIRED[..EXPIRED.len() - 1]
    );
    let invoice: Invoice = serde_json::from_str(&marked).expect("decode");
    assert_eq!(invoice.status, InvoiceStatus::Expired);
    assert!(invoice.is_final);
    assert_eq!(invoice.paid_late_at, Some(1_700_007_200));
    assert_eq!(
        invoice.payment.as_ref().unwrap().paid.as_deref(),
        Some("60.00")
    );
}

#[test]
fn decodes_null_and_absent_marks() {
    let null_marked = format!("{},\"paid_late_at\":null}}", &EXPIRED[..EXPIRED.len() - 1]);
    let invoice: Invoice = serde_json::from_str(&null_marked).expect("decode");
    assert_eq!(invoice.paid_late_at, None);

    let legacy: Invoice = serde_json::from_str(EXPIRED).expect("legacy decode");
    assert_eq!(legacy.paid_late_at, None);
    assert_eq!(legacy.status, InvoiceStatus::Expired);
}

#[test]
fn serializes_paid_late_stage() {
    #[derive(serde::Serialize)]
    struct SimulationRequest {
        stage: InvoiceSimulationStage,
    }
    let body = serde_json::to_string(&SimulationRequest {
        stage: InvoiceSimulationStage::PaidLate,
    })
    .expect("encode");
    assert_eq!(body, r#"{"stage":"paid_late"}"#);
}
