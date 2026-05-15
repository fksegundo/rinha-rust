#[cfg(test)]
mod tests {
    use crate::index::SpecialistIndex;
    use crate::vector;

    #[test]
    fn test_specialist_index_roundtrip() {
        let index_path = "/tmp/test.idx";
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
