#[cfg(test)]
mod tests {
    use crate::index::SpecialistIndex;
    use crate::index::build::{Reference, build_index};
    use crate::vector;
    use crate::{PACKED_DIMS, SCALE};

    const EXAMPLE_PAYLOADS: &[&[u8]] = &[
        br#"{"id":"tx-1329056812","transaction":{"amount":2508.13,"installments":7,"requested_at":"2026-03-11T03:45:53Z"},"customer":{"avg_amount":209.74,"tx_count_24h":13,"known_merchants":["MERC-003","MERC-016"]},"merchant":{"id":"MERC-089","mcc":"7801","avg_amount":25.15},"terminal":{"is_online":false,"card_present":true,"km_from_home":667.7296579973},"last_transaction":null}"#,
        br#"{"id":"tx-3576980410","transaction":{"amount":384.88,"installments":3,"requested_at":"2026-03-11T20:23:35Z"},"customer":{"avg_amount":769.76,"tx_count_24h":3,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-001","mcc":"5912","avg_amount":298.95},"terminal":{"is_online":false,"card_present":true,"km_from_home":13.7090520965},"last_transaction":{"timestamp":"2026-03-11T14:58:35Z","km_from_current":18.8626479774}}"#,
        br#"{"customer":{"avg_amount":68.88,"tx_count_24h":18,"known_merchants":["MERC-004","MERC-015","MERC-007"]},"id":"tx-1788243118","last_transaction":{"timestamp":"2026-03-17T01:58:06Z","km_from_current":660.9200962961},"merchant":{"id":"MERC-062","mcc":"7801","avg_amount":25.55},"terminal":{"is_online":true,"card_present":false,"km_from_home":881.6139684714},"transaction":{"amount":4368.82,"installments":8,"requested_at":"2026-03-17T02:04:06Z"}}"#,
    ];

    #[test]
    fn test_specialist_index_roundtrip() {
        let index_path = "/tmp/rinha-rust-test.idx";
        let references = vec![
            Reference {
                vector: [0i16; PACKED_DIMS],
                label: 0,
            },
            Reference {
                vector: [SCALE; PACKED_DIMS],
                label: 1,
            },
        ];
        let index_bytes = build_index(references, 64, 0).expect("failed to build index");
        std::fs::write(index_path, index_bytes).expect("failed to write test index");

        let index = SpecialistIndex::open(index_path).expect("failed to open index");

        for body in EXAMPLE_PAYLOADS.iter() {
            let mut query = [0i16; 16];
            vector::parse_query(body, &mut query).expect("parse failed");

            let count = index.predict_fraud_count(&query);
            if count > 5 {
                panic!(
                    "fraud count {} for query: {:?}",
                    count,
                    std::str::from_utf8(body).unwrap_or("???")
                );
            }
        }
    }
}
