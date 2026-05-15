#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn test_example_payloads() {
        let payloads = std::fs::read_to_string(
            "../rinha-de-backend-2026-main/resources/example-payloads.json",
        )
        .expect("missing example-payloads.json");

        let parsed: serde_json::Value = serde_json::from_str(&payloads).expect("invalid JSON");
        let array = parsed.as_array().expect("expected array");

        for (i, item) in array.iter().enumerate() {
            let body = serde_json::to_vec(item).expect("serialize failed");
            let mut query = [0i16; 16];
            let result = parse_query(&body, &mut query);
            assert!(result.is_ok(), "payload {} failed: {:?}", i, result);
        }
    }

    #[test]
    fn customer_first_payload_matches_transaction_first_payload() {
        let transaction_first = br#"{"id":"tx-1","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"last_transaction":{"timestamp":"2026-03-11T14:58:35Z","km_from_current":18.8626479774}}"#;
        let customer_first = br#"{"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"id":"tx-1","last_transaction":{"timestamp":"2026-03-11T14:58:35Z","km_from_current":18.8626479774},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"}}"#;

        let mut expected = [0i16; 16];
        let mut actual = [0i16; 16];

        parse_query(transaction_first, &mut expected).expect("transaction-first parse failed");
        parse_query(customer_first, &mut actual).expect("customer-first parse failed");

        assert_eq!(actual, expected);
    }
}
