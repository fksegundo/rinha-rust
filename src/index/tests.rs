#[cfg(test)]
mod tests {
    use crate::index::SpecialistIndex;
    use crate::index::build::{Reference, build_index};
    use crate::vector;
    use crate::{PACKED_DIMS, SCALE};

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

        let payloads = std::fs::read_to_string(
            "../rinha-de-backend-2026-main/resources/example-payloads.json",
        )
        .expect("missing example-payloads.json");

        let parsed: serde_json::Value = serde_json::from_str(&payloads).expect("invalid JSON");
        let array = parsed.as_array().expect("expected array");

        for item in array.iter() {
            let body = serde_json::to_vec(item).expect("serialize failed");
            let mut query = [0i16; 16];
            vector::parse_query(&body, &mut query).expect("parse failed");

            let count = index.predict_fraud_count(&query);
            if count > 5 {
                panic!(
                    "fraud count {} for query: {:?}",
                    count,
                    std::str::from_utf8(&body).unwrap_or("???")
                );
            }
        }
    }
}
