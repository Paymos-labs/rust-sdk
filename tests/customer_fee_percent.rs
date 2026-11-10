//! Regression coverage for the integer customer fee field sent to Merchant API.
use paymos::{CreateInvoiceRequest, Error, PaymosClient};

fn request(percent: Option<u8>) -> CreateInvoiceRequest {
    CreateInvoiceRequest {
        project_id: "prj_test".to_owned(),
        amount: "10".to_owned(),
        currency: "USD".to_owned(),
        external_order_id: "order-1".to_owned(),
        network: None,
        customer_fee_percent: percent,
        client_id: None,
    }
}

#[test]
fn customer_fee_percent_uses_integer_wire_numbers() {
    for percent in [0, 50, 100] {
        let body = serde_json::to_vec(&request(Some(percent))).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(value["customer_fee_percent"].is_u64());
        assert_eq!(
            value["customer_fee_percent"].as_u64(),
            Some(u64::from(percent))
        );
        assert!(String::from_utf8(body)
            .unwrap()
            .contains(&format!("\"customer_fee_percent\":{percent}")));
    }
}

#[test]
fn customer_fee_percent_none_remains_omitted() {
    let body = serde_json::to_value(request(None)).unwrap();
    assert!(body.get("customer_fee_percent").is_none());
}

#[tokio::test]
async fn customer_fee_percent_above_100_is_rejected_before_http() {
    let client = PaymosClient::builder("pk_test", "sk_test")
        .base_url("http://127.0.0.1:1")
        .max_retries(0)
        .build()
        .unwrap();
    for percent in [101, 255] {
        assert!(matches!(
            client.invoices().create(&request(Some(percent))).await,
            Err(Error::InvalidArgument(_))
        ));
    }
}
