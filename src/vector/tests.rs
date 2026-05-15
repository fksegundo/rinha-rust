#[cfg(test)]
mod tests {
    use super::super::*;

    const EXAMPLE_PAYLOADS: &[&[u8]] = &[
        br#"{"id":"tx-1329056812","transaction":{"amount":2508.13,"installments":7,"requested_at":"2026-03-11T03:45:53Z"},"customer":{"avg_amount":209.74,"tx_count_24h":13,"known_merchants":["MERC-003","MERC-016"]},"merchant":{"id":"MERC-089","mcc":"7801","avg_amount":25.15},"terminal":{"is_online":false,"card_present":true,"km_from_home":667.7296579973},"last_transaction":null}"#,
        br#"{"id":"tx-3576980410","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"last_transaction":{"timestamp":"2026-03-11T14:58:35Z","km_from_current":18.8626479774}}"#,
        br#"{"customer":{"avg_amount":68.88,"tx_count_24h":18,"known_merchants":["MERC-004","MERC-015","MERC-007"]},"id":"tx-1788243118","last_transaction":{"timestamp":"2026-03-17T01:58:06Z","km_from_current":660.9200962961},"merchant":{"id":"MERC-062","mcc":"7801","avg_amount":25.55},"terminal":{"is_online":true,"card_present":false,"km_from_home":881.6139684714},"transaction":{"amount":4368.82,"installments":8,"requested_at":"2026-03-17T02:04:06Z"}}"#,
    ];

    #[test]
    fn test_example_payloads() {
        for (i, body) in EXAMPLE_PAYLOADS.iter().enumerate() {
            let mut query = [0i16; 16];
            let result = parse_query(body, &mut query);
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
