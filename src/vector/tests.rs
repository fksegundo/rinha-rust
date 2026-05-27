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

    #[test]
    fn fast_path_matches_legacy_for_example_payloads() {
        for (i, body) in EXAMPLE_PAYLOADS.iter().enumerate() {
            let mut fast = [0i16; 16];
            let mut legacy = [0i16; 16];

            fast.fill(0);
            assert!(
                try_parse_single_pass(body, &mut fast).is_ok(),
                "payload {} failed on fast path",
                i
            );

            legacy.fill(0);
            try_parse_serde(body, &mut legacy).expect("serde failed on example payload");

            assert_eq!(fast, legacy, "payload {} mismatch", i);
        }
    }

    #[test]
    fn equivalent_fallback_payloads_match_baseline() {
        let baseline = br#"{"id":"tx-1","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"last_transaction":{"timestamp":"2026-03-11T14:58:35Z","km_from_current":18.8626479774}}"#;
        let extra_fields = br#"{"id":"tx-1","unexpected_top_level":"ignored","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z","extra_transaction_field":123},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"],"extra_customer_field":true},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95,"extra_merchant_field":{"nested":"ignored"}},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965,"extra_terminal_field":["ignored"]},"last_transaction":{"timestamp":"2026-03-11T14:58:35Z","km_from_current":18.8626479774,"extra_last_transaction_field":"ignored"}}"#;
        let pascal_case = br#"{"Id":"tx-1","Transaction":{"Amount":384.88,"Installments":3,"Requested_at":"2026-03-11T20:23:35Z"},"Customer":{"Avg_amount":769.76,"Tx_count_24h":3,"Known_merchants":["MERC-009","MERC-001"]},"Merchant":{"Id":"MERC-001","Mcc":"5912","Avg_amount":298.95},"Terminal":{"Is_online":false,"Card_present":true,"Km_from_home":13.7090520965},"Last_transaction":{"Timestamp":"2026-03-11T14:58:35Z","Km_from_current":18.8626479774}}"#;
        let mixed_case_separators = br#"{"ID":"tx-1","TRANSACTION":{"AMOUNT":384.88,"installments":3,"requestedAt":"2026-03-11T20:23:35Z"},"CUSTOMER":{"avg-amount":769.76,"txCount24h":3,"knownMerchants":["MERC-009","MERC-001"]},"MERCHANT":{"ID":"MERC-001","MCC":"5912","AVG-AMOUNT":298.95},"TERMINAL":{"isOnline":false,"card-present":true,"kmFromHome":13.7090520965},"lastTransaction":{"TIMESTAMP":"2026-03-11T14:58:35Z","kmFromCurrent":18.8626479774}}"#;

        let expected = parse_vec(baseline);

        assert_eq!(parse_vec(extra_fields), expected);
        assert_eq!(parse_vec(pascal_case), expected);
        assert_eq!(parse_vec(mixed_case_separators), expected);
    }

    #[test]
    fn malformed_or_incomplete_payloads_are_rejected() {
        let invalid_payloads: &[&[u8]] = &[
            b"",
            b"{",
            b"{}",
            br#"{"id":"tx-1","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"last_transaction":null}"#,
            br#"{"id":"tx-1","transaction":{"amount":"384.88","installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"last_transaction":null}"#,
            br#"{"id":"tx-1","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":null,"avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"last_transaction":null}"#,
        ];

        for payload in invalid_payloads {
            let mut query = [0i16; 16];
            assert!(
                parse_query(payload, &mut query).is_err(),
                "payload should be rejected: {}",
                String::from_utf8_lossy(payload)
            );
        }
    }

    fn parse_vec(payload: &[u8]) -> [i16; 16] {
        let mut query = [0i16; 16];
        parse_query(payload, &mut query).expect("payload should parse");
        query
    }
}
