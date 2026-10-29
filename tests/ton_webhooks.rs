//! Signed schema 1 TON webhook envelopes with raw and friendly addresses.

use paymos::{WebhookEvent, WebhookVerifier};
use serde_json::Value;

#[test]
fn signed_ton_webhook_raw_and_friendly_v1_preserve_fields() {
    let contract: Value =
        serde_json::from_str(include_str!("../conformance/contract.json")).unwrap();
    let fixtures = &contract["vectors"]["ton_webhooks"];
    let vectors = fixtures["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 8);
    let now = fixtures["now"].as_i64().unwrap();
    let verifier = WebhookVerifier::new(fixtures["secret"].as_str().unwrap()).unwrap();

    for vector in vectors {
        assert_eq!(vector["version"].as_u64(), Some(1));
        let body = vector["raw_body"].as_str().unwrap();
        let header = vector["header"].as_str().unwrap();
        let event: WebhookEvent<Value> = verifier
            .construct_event_at(header, body.as_bytes(), now)
            .unwrap();
        assert_eq!(event.event_id, vector["event_id"].as_str().unwrap());
        assert_eq!(event.event_type, vector["event_type"].as_str().unwrap());
        assert_eq!(
            u64::from(event.version),
            vector["version"].as_u64().unwrap()
        );
        assert_eq!(event.occurred_at, now);
        for expected in vector["expect"].as_array().unwrap() {
            let mut value = &event.data;
            for part in expected["path"].as_array().unwrap() {
                value = value
                    .get(part.as_str().unwrap())
                    .expect("missing signed webhook field");
            }
            assert_eq!(value, &expected["value"]);
        }
        assert!(verifier
            .verify_at(header, format!("{body} ").as_bytes(), now)
            .is_err());
    }
}
