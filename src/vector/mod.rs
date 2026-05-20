use crate::{QueryVector, SCALE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    MissingField,
    InvalidValue,
    InvalidFormat,
}

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};
use std::borrow::Cow;
use std::fmt;

pub fn parse_query(payload: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    out.fill(0);

    if let Ok(()) = try_parse_transaction_first(payload, out) {
        return Ok(());
    }

    out.fill(0);

    if let Ok(()) = try_parse_customer_first(payload, out) {
        return Ok(());
    }

    out.fill(0);

    try_parse_serde(payload, out)
}

#[inline]
fn quantize(value: f64) -> i16 {
    if value <= -1.0 {
        -SCALE
    } else if value <= 0.0 {
        0
    } else if value >= 1.0 {
        SCALE
    } else {
        (value * SCALE as f64).round() as i16
    }
}

fn try_parse_transaction_first(json: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    let mut cursor: usize = 0;
    let mut known_hashes = [0u64; 64];
    let known_count: usize;

    let amount = find_and_read_double(json, b"\"amount\"", &mut cursor)?;
    out[0] = quantize(amount / 10_000.0);

    let installments = find_and_read_int(json, b"\"installments\"", &mut cursor)?;
    out[1] = quantize(installments as f64 / 12.0);

    let requested_at = find_and_read_string(json, b"\"requested_at\"", &mut cursor)?;
    let parsed = parse_datetime(requested_at)?;
    let requested_minute = parsed.epoch_minute;
    out[3] = quantize(parsed.hour as f64 / 23.0);
    out[4] = quantize(parsed.day_of_week as f64 / 6.0);

    let customer_avg_amount = find_and_read_double(json, b"\"avg_amount\"", &mut cursor)?;

    let tx_count_24h = find_and_read_int(json, b"\"tx_count_24h\"", &mut cursor)?;
    out[8] = quantize(tx_count_24h as f64 / 20.0);

    known_count = find_and_read_known_merchants(json, &mut cursor, &mut known_hashes)?;

    let merchant_id = find_and_read_string(json, b"\"id\"", &mut cursor)?;
    let merchant_hash = hash_bytes(merchant_id);

    let mcc = find_and_read_string(json, b"\"mcc\"", &mut cursor)?;
    out[12] = quantize(mcc_risk(parse_mcc(mcc)));

    let merchant_avg_amount = find_and_read_double(json, b"\"avg_amount\"", &mut cursor)?;
    out[13] = quantize(merchant_avg_amount / 10_000.0);

    let is_online = find_and_read_bool(json, b"\"is_online\"", &mut cursor)?;
    out[9] = if is_online { SCALE } else { 0 };

    let card_present = find_and_read_bool(json, b"\"card_present\"", &mut cursor)?;
    out[10] = if card_present { SCALE } else { 0 };

    let km_from_home = find_and_read_double(json, b"\"km_from_home\"", &mut cursor)?;
    out[7] = quantize(km_from_home / 1_000.0);

    let last_value = find_value_start(json, b"\"last_transaction\"", &mut cursor)?;

    if last_value < json.len() && json[last_value] == b'n' {
        out[5] = -SCALE;
        out[6] = -SCALE;
    } else {
        cursor = last_value;
        let last_timestamp = find_and_read_string(json, b"\"timestamp\"", &mut cursor)?;
        let last_km = find_and_read_double(json, b"\"km_from_current\"", &mut cursor)?;

        let last_parsed = parse_datetime(last_timestamp)?;
        let last_minute = last_parsed.epoch_minute;
        let minutes_diff = requested_minute.saturating_sub(last_minute);
        out[5] = quantize(minutes_diff as f64 / 1_440.0);
        out[6] = quantize(last_km / 1_000.0);
    }

    finish_vector(
        out,
        amount,
        customer_avg_amount,
        merchant_hash,
        &known_hashes[..known_count],
    );
    Ok(())
}

fn try_parse_customer_first(json: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    let mut cursor: usize = 0;
    let mut known_hashes = [0u64; 64];

    let customer_avg_amount = find_and_read_double(json, b"\"avg_amount\"", &mut cursor)?;

    let tx_count_24h = find_and_read_int(json, b"\"tx_count_24h\"", &mut cursor)?;
    out[8] = quantize(tx_count_24h as f64 / 20.0);

    let known_count = find_and_read_known_merchants(json, &mut cursor, &mut known_hashes)?;

    let last_value = find_value_start(json, b"\"last_transaction\"", &mut cursor)?;
    let last_info = if last_value < json.len() && json[last_value] == b'n' {
        None
    } else {
        cursor = last_value;
        let last_timestamp = find_and_read_string(json, b"\"timestamp\"", &mut cursor)?;
        let last_km = find_and_read_double(json, b"\"km_from_current\"", &mut cursor)?;
        Some((last_timestamp, last_km))
    };

    let merchant_id = find_and_read_string(json, b"\"id\"", &mut cursor)?;
    let merchant_hash = hash_bytes(merchant_id);

    let mcc = find_and_read_string(json, b"\"mcc\"", &mut cursor)?;
    out[12] = quantize(mcc_risk(parse_mcc(mcc)));

    let merchant_avg_amount = find_and_read_double(json, b"\"avg_amount\"", &mut cursor)?;
    out[13] = quantize(merchant_avg_amount / 10_000.0);

    let is_online = find_and_read_bool(json, b"\"is_online\"", &mut cursor)?;
    out[9] = if is_online { SCALE } else { 0 };

    let card_present = find_and_read_bool(json, b"\"card_present\"", &mut cursor)?;
    out[10] = if card_present { SCALE } else { 0 };

    let km_from_home = find_and_read_double(json, b"\"km_from_home\"", &mut cursor)?;
    out[7] = quantize(km_from_home / 1_000.0);

    let amount = find_and_read_double(json, b"\"amount\"", &mut cursor)?;
    out[0] = quantize(amount / 10_000.0);

    let installments = find_and_read_int(json, b"\"installments\"", &mut cursor)?;
    out[1] = quantize(installments as f64 / 12.0);

    let requested_at = find_and_read_string(json, b"\"requested_at\"", &mut cursor)?;
    let parsed = parse_datetime(requested_at)?;
    let requested_minute = parsed.epoch_minute;
    out[3] = quantize(parsed.hour as f64 / 23.0);
    out[4] = quantize(parsed.day_of_week as f64 / 6.0);

    if let Some((last_timestamp, last_km)) = last_info {
        let last_parsed = parse_datetime(last_timestamp)?;
        let last_minute = last_parsed.epoch_minute;
        let minutes_diff = requested_minute.saturating_sub(last_minute);
        out[5] = quantize(minutes_diff as f64 / 1_440.0);
        out[6] = quantize(last_km / 1_000.0);
    } else {
        out[5] = -SCALE;
        out[6] = -SCALE;
    }

    finish_vector(
        out,
        amount,
        customer_avg_amount,
        merchant_hash,
        &known_hashes[..known_count],
    );
    Ok(())
}

struct Payload<'a> {
    transaction: Transaction<'a>,
    customer: Customer<'a>,
    merchant: Merchant<'a>,
    terminal: Terminal,
    last_transaction: Option<LastTransaction<'a>>,
}

struct Transaction<'a> {
    amount: f64,
    installments: i32,
    requested_at: Cow<'a, str>,
}

struct Customer<'a> {
    avg_amount: f64,
    tx_count_24h: i32,
    known_merchants: Vec<Cow<'a, str>>,
}

struct Merchant<'a> {
    id: Cow<'a, str>,
    mcc: Cow<'a, str>,
    avg_amount: f64,
}

struct Terminal {
    is_online: bool,
    card_present: bool,
    km_from_home: f64,
}

struct LastTransaction<'a> {
    timestamp: Cow<'a, str>,
    km_from_current: f64,
}

impl<'de: 'a, 'a> Deserialize<'de> for Payload<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PayloadVisitor;

        impl<'de> Visitor<'de> for PayloadVisitor {
            type Value = Payload<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("fraud score payload object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut transaction = None;
                let mut customer = None;
                let mut merchant = None;
                let mut terminal = None;
                let mut last_transaction = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "transaction") {
                        transaction = Some(map.next_value()?);
                    } else if field_matches(&key, "customer") {
                        customer = Some(map.next_value()?);
                    } else if field_matches(&key, "merchant") {
                        merchant = Some(map.next_value()?);
                    } else if field_matches(&key, "terminal") {
                        terminal = Some(map.next_value()?);
                    } else if field_matches(&key, "last_transaction") {
                        last_transaction = map.next_value()?;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Payload {
                    transaction: transaction
                        .ok_or_else(|| de::Error::missing_field("transaction"))?,
                    customer: customer.ok_or_else(|| de::Error::missing_field("customer"))?,
                    merchant: merchant.ok_or_else(|| de::Error::missing_field("merchant"))?,
                    terminal: terminal.ok_or_else(|| de::Error::missing_field("terminal"))?,
                    last_transaction,
                })
            }
        }

        deserializer.deserialize_map(PayloadVisitor)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Transaction<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TransactionVisitor;

        impl<'de> Visitor<'de> for TransactionVisitor {
            type Value = Transaction<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("transaction object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut amount = None;
                let mut installments = None;
                let mut requested_at = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "amount") {
                        amount = Some(map.next_value()?);
                    } else if field_matches(&key, "installments") {
                        installments = Some(map.next_value()?);
                    } else if field_matches(&key, "requested_at") {
                        requested_at = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Transaction {
                    amount: amount.ok_or_else(|| de::Error::missing_field("amount"))?,
                    installments: installments
                        .ok_or_else(|| de::Error::missing_field("installments"))?,
                    requested_at: requested_at
                        .ok_or_else(|| de::Error::missing_field("requested_at"))?,
                })
            }
        }

        deserializer.deserialize_map(TransactionVisitor)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Customer<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CustomerVisitor;

        impl<'de> Visitor<'de> for CustomerVisitor {
            type Value = Customer<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("customer object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut avg_amount = None;
                let mut tx_count_24h = None;
                let mut known_merchants = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "avg_amount") {
                        avg_amount = Some(map.next_value()?);
                    } else if field_matches(&key, "tx_count_24h") {
                        tx_count_24h = Some(map.next_value()?);
                    } else if field_matches(&key, "known_merchants") {
                        known_merchants = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Customer {
                    avg_amount: avg_amount.ok_or_else(|| de::Error::missing_field("avg_amount"))?,
                    tx_count_24h: tx_count_24h
                        .ok_or_else(|| de::Error::missing_field("tx_count_24h"))?,
                    known_merchants: known_merchants
                        .ok_or_else(|| de::Error::missing_field("known_merchants"))?,
                })
            }
        }

        deserializer.deserialize_map(CustomerVisitor)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Merchant<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MerchantVisitor;

        impl<'de> Visitor<'de> for MerchantVisitor {
            type Value = Merchant<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("merchant object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut id = None;
                let mut mcc = None;
                let mut avg_amount = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "id") {
                        id = Some(map.next_value()?);
                    } else if field_matches(&key, "mcc") {
                        mcc = Some(map.next_value()?);
                    } else if field_matches(&key, "avg_amount") {
                        avg_amount = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Merchant {
                    id: id.ok_or_else(|| de::Error::missing_field("id"))?,
                    mcc: mcc.ok_or_else(|| de::Error::missing_field("mcc"))?,
                    avg_amount: avg_amount.ok_or_else(|| de::Error::missing_field("avg_amount"))?,
                })
            }
        }

        deserializer.deserialize_map(MerchantVisitor)
    }
}

impl<'de> Deserialize<'de> for Terminal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TerminalVisitor;

        impl<'de> Visitor<'de> for TerminalVisitor {
            type Value = Terminal;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("terminal object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut is_online = None;
                let mut card_present = None;
                let mut km_from_home = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "is_online") {
                        is_online = Some(map.next_value()?);
                    } else if field_matches(&key, "card_present") {
                        card_present = Some(map.next_value()?);
                    } else if field_matches(&key, "km_from_home") {
                        km_from_home = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(Terminal {
                    is_online: is_online.ok_or_else(|| de::Error::missing_field("is_online"))?,
                    card_present: card_present
                        .ok_or_else(|| de::Error::missing_field("card_present"))?,
                    km_from_home: km_from_home
                        .ok_or_else(|| de::Error::missing_field("km_from_home"))?,
                })
            }
        }

        deserializer.deserialize_map(TerminalVisitor)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for LastTransaction<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct LastTransactionVisitor;

        impl<'de> Visitor<'de> for LastTransactionVisitor {
            type Value = LastTransaction<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("last transaction object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut timestamp = None;
                let mut km_from_current = None;

                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if field_matches(&key, "timestamp") {
                        timestamp = Some(map.next_value()?);
                    } else if field_matches(&key, "km_from_current") {
                        km_from_current = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(LastTransaction {
                    timestamp: timestamp.ok_or_else(|| de::Error::missing_field("timestamp"))?,
                    km_from_current: km_from_current
                        .ok_or_else(|| de::Error::missing_field("km_from_current"))?,
                })
            }
        }

        deserializer.deserialize_map(LastTransactionVisitor)
    }
}

fn field_matches(actual: &str, expected: &str) -> bool {
    let mut actual = actual.bytes().filter(|&b| b != b'_' && b != b'-');
    let mut expected = expected.bytes().filter(|&b| b != b'_' && b != b'-');

    loop {
        match (actual.next(), expected.next()) {
            (Some(a), Some(e)) if a.eq_ignore_ascii_case(&e) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

fn try_parse_serde(payload: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    let parsed: Payload = serde_json::from_slice(payload).map_err(|_| ParseError::InvalidFormat)?;

    let requested_parsed = parse_datetime(parsed.transaction.requested_at.as_bytes())?;
    let requested_minute = requested_parsed.epoch_minute;

    out[0] = quantize(parsed.transaction.amount / 10_000.0);
    out[1] = quantize(parsed.transaction.installments as f64 / 12.0);
    out[3] = quantize(requested_parsed.hour as f64 / 23.0);
    out[4] = quantize(requested_parsed.day_of_week as f64 / 6.0);
    out[7] = quantize(parsed.terminal.km_from_home / 1_000.0);
    out[8] = quantize(parsed.customer.tx_count_24h as f64 / 20.0);
    out[9] = if parsed.terminal.is_online { SCALE } else { 0 };
    out[10] = if parsed.terminal.card_present {
        SCALE
    } else {
        0
    };
    out[12] = quantize(mcc_risk(parse_mcc(parsed.merchant.mcc.as_bytes())));
    out[13] = quantize(parsed.merchant.avg_amount / 10_000.0);

    if let Some(last_transaction) = parsed.last_transaction {
        let last_parsed = parse_datetime(last_transaction.timestamp.as_bytes())?;
        let last_minute = last_parsed.epoch_minute;
        let minutes_diff = requested_minute.saturating_sub(last_minute);
        out[5] = quantize(minutes_diff as f64 / 1_440.0);
        out[6] = quantize(last_transaction.km_from_current / 1_000.0);
    } else {
        out[5] = -SCALE;
        out[6] = -SCALE;
    }

    let mut known_hashes = [0u64; 64];
    let known_count = std::cmp::min(parsed.customer.known_merchants.len(), 64);
    for i in 0..known_count {
        known_hashes[i] = hash_bytes(parsed.customer.known_merchants[i].as_bytes());
    }

    finish_vector(
        out,
        parsed.transaction.amount,
        parsed.customer.avg_amount,
        hash_bytes(parsed.merchant.id.as_bytes()),
        &known_hashes[..known_count],
    );
    Ok(())
}

fn finish_vector(
    out: &mut QueryVector,
    amount: f64,
    customer_avg_amount: f64,
    merchant_hash: u64,
    known_hashes: &[u64],
) {
    out[2] = if customer_avg_amount > 0.0 {
        quantize((amount / customer_avg_amount) / 10.0)
    } else {
        SCALE
    };

    let mut known = false;
    for &h in known_hashes {
        if h == merchant_hash {
            known = true;
            break;
        }
    }
    out[11] = if known { 0 } else { SCALE };
}

fn find_value_start(json: &[u8], name: &[u8], cursor: &mut usize) -> Result<usize, ParseError> {
    if *cursor >= json.len() {
        return Err(ParseError::MissingField);
    }
    let rel = json[*cursor..]
        .windows(name.len())
        .position(|w| w == name)
        .ok_or(ParseError::MissingField)?;
    let after_name = *cursor + rel + name.len();
    let rel_colon = json[after_name..]
        .iter()
        .position(|&b| b == b':')
        .ok_or(ParseError::MissingField)?;
    let mut value_start = after_name + rel_colon + 1;
    while value_start < json.len() && is_json_whitespace(json[value_start]) {
        value_start += 1;
    }
    *cursor = value_start;
    Ok(value_start)
}

fn find_and_read_double(json: &[u8], name: &[u8], cursor: &mut usize) -> Result<f64, ParseError> {
    let start = find_value_start(json, name, cursor)?;
    read_double_at(json, start)
}

fn find_and_read_int(json: &[u8], name: &[u8], cursor: &mut usize) -> Result<i32, ParseError> {
    let start = find_value_start(json, name, cursor)?;
    read_int_at(json, start)
}

fn find_and_read_bool(json: &[u8], name: &[u8], cursor: &mut usize) -> Result<bool, ParseError> {
    let start = find_value_start(json, name, cursor)?;
    read_bool_at(json, start)
}

fn find_and_read_string<'a>(
    json: &'a [u8],
    name: &[u8],
    cursor: &mut usize,
) -> Result<&'a [u8], ParseError> {
    let start = find_value_start(json, name, cursor)?;
    read_string_at(json, start)
}

fn find_and_read_known_merchants(
    json: &[u8],
    cursor: &mut usize,
    hashes: &mut [u64; 64],
) -> Result<usize, ParseError> {
    let start = find_value_start(json, b"\"known_merchants\"", cursor)?;
    if start >= json.len() || json[start] != b'[' {
        return Err(ParseError::InvalidFormat);
    }
    let rel_end = json[start..]
        .iter()
        .position(|&b| b == b']')
        .ok_or(ParseError::InvalidFormat)?;
    let array_end = start + rel_end;
    let mut i = start + 1;
    let mut count = 0usize;
    while i < array_end {
        while i < array_end && json[i] != b'"' {
            i += 1;
        }
        if i >= array_end {
            break;
        }
        let content_start = i + 1;
        let rel = json[content_start..array_end]
            .iter()
            .position(|&b| b == b'"')
            .ok_or(ParseError::InvalidFormat)?;
        if count < hashes.len() {
            hashes[count] = hash_bytes(&json[content_start..content_start + rel]);
            count += 1;
        }
        i = content_start + rel + 1;
    }
    *cursor = array_end + 1;
    Ok(count)
}

fn read_double_at(json: &[u8], start: usize) -> Result<f64, ParseError> {
    let s = &json[start..];
    let mut end = 0usize;
    let mut seen_dot = false;
    let mut seen_digit = false;
    for &b in s {
        match b {
            b'0'..=b'9' => {
                seen_digit = true;
                end += 1;
            }
            b'-' | b'+' if end == 0 => end += 1,
            b'.' if !seen_dot => {
                seen_dot = true;
                end += 1;
            }
            b'e' | b'E' if seen_digit => {
                end += 1;
                if s.get(end).copied() == Some(b'-') || s.get(end).copied() == Some(b'+') {
                    end += 1;
                }
                while s.get(end).map_or(false, |&b| b.is_ascii_digit()) {
                    end += 1;
                }
                break;
            }
            _ => break,
        }
    }
    if end == 0 {
        return Err(ParseError::InvalidValue);
    }
    let text = std::str::from_utf8(&s[..end]).map_err(|_| ParseError::InvalidValue)?;
    text.parse::<f64>().map_err(|_| ParseError::InvalidValue)
}

fn read_int_at(json: &[u8], start: usize) -> Result<i32, ParseError> {
    let s = &json[start..];
    let mut end = 0usize;
    if s.first().copied() == Some(b'-') {
        end += 1;
    }
    while s.get(end).map_or(false, |&b| b.is_ascii_digit()) {
        end += 1;
    }
    if end == 0 || (end == 1 && s[0] == b'-') {
        return Err(ParseError::InvalidValue);
    }
    let text = std::str::from_utf8(&s[..end]).map_err(|_| ParseError::InvalidValue)?;
    text.parse::<i32>().map_err(|_| ParseError::InvalidValue)
}

fn read_bool_at(json: &[u8], start: usize) -> Result<bool, ParseError> {
    let s = &json[start..];
    if s.starts_with(b"true") {
        Ok(true)
    } else if s.starts_with(b"false") {
        Ok(false)
    } else {
        Err(ParseError::InvalidValue)
    }
}

fn read_string_at<'a>(json: &'a [u8], start: usize) -> Result<&'a [u8], ParseError> {
    if start >= json.len() || json[start] != b'"' {
        return Err(ParseError::InvalidValue);
    }
    let content_start = start + 1;
    let mut escaped = false;
    for i in content_start..json.len() {
        let b = json[i];
        if escaped {
            escaped = false;
            continue;
        }
        if b == b'\\' {
            escaped = true;
            continue;
        }
        if b == b'"' {
            return Ok(&json[content_start..i]);
        }
    }
    Err(ParseError::InvalidValue)
}

fn is_json_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t')
}

#[inline(always)]
fn hash_bytes(value: &[u8]) -> u64 {
    let mut hash = 0x517cc1b727220a95u64;
    let mut i = 0;
    while i + 8 <= value.len() {
        let word = u64::from_le_bytes(value[i..i + 8].try_into().unwrap());
        hash = hash.rotate_left(5) ^ word;
        hash = hash.wrapping_mul(0x517cc1b727220a95);
        i += 8;
    }
    for &b in &value[i..] {
        hash = hash.rotate_left(5) ^ (b as u64);
        hash = hash.wrapping_mul(0x517cc1b727220a95);
    }
    hash
}

fn parse_mcc(mcc: &[u8]) -> i32 {
    if mcc.len() != 4 {
        return 0;
    }
    let a = mcc[0].wrapping_sub(b'0');
    let b = mcc[1].wrapping_sub(b'0');
    let c = mcc[2].wrapping_sub(b'0');
    let d = mcc[3].wrapping_sub(b'0');
    if a > 9 || b > 9 || c > 9 || d > 9 {
        return 0;
    }
    (a as i32) * 1000 + (b as i32) * 100 + (c as i32) * 10 + (d as i32)
}

fn mcc_risk(mcc: i32) -> f64 {
    match mcc {
        5411 => 0.15,
        5812 => 0.30,
        5912 => 0.20,
        5944 => 0.45,
        7801 => 0.80,
        7802 => 0.75,
        7995 => 0.85,
        4511 => 0.35,
        5311 => 0.25,
        5999 => 0.50,
        _ => 0.50,
    }
}

#[derive(Clone, Copy)]
struct ParsedDateTime {
    epoch_minute: i64,
    hour: i32,
    day_of_week: i32,
}

fn parse_datetime(iso: &[u8]) -> Result<ParsedDateTime, ParseError> {
    if iso.len() < 16 {
        return Err(ParseError::InvalidValue);
    }
    let y = parse4(iso, 0)?;
    let m = parse2(iso, 5)?;
    let d = parse2(iso, 8)?;
    let hh = parse2(iso, 11)?;
    let mm = parse2(iso, 14)?;

    let days = days_from_civil(y, m, d);
    let epoch_minute = days * 1_440 + (hh as i64) * 60 + (mm as i64);
    let day_of_week = ((days + 3) % 7) as i32;

    Ok(ParsedDateTime {
        epoch_minute,
        hour: hh,
        day_of_week,
    })
}

fn parse2(s: &[u8], offset: usize) -> Result<i32, ParseError> {
    if offset + 2 > s.len() {
        return Err(ParseError::InvalidValue);
    }
    let a = s[offset].wrapping_sub(b'0');
    let b = s[offset + 1].wrapping_sub(b'0');
    if a > 9 || b > 9 {
        return Err(ParseError::InvalidValue);
    }
    Ok((a as i32) * 10 + (b as i32))
}

fn parse4(s: &[u8], offset: usize) -> Result<i32, ParseError> {
    if offset + 4 > s.len() {
        return Err(ParseError::InvalidValue);
    }
    let a = s[offset].wrapping_sub(b'0');
    let b = s[offset + 1].wrapping_sub(b'0');
    let c = s[offset + 2].wrapping_sub(b'0');
    let d = s[offset + 3].wrapping_sub(b'0');
    if a > 9 || b > 9 || c > 9 || d > 9 {
        return Err(ParseError::InvalidValue);
    }
    Ok((a as i32) * 1000 + (b as i32) * 100 + (c as i32) * 10 + (d as i32))
}

fn days_from_civil(y: i32, m: i32, d: i32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u32;
    let shifted_month = m + if m > 2 { -3 } else { 9 };
    let doy = (153u32 * (shifted_month as u32) + 2) / 5 + (d as u32) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era as i64) * 146_097 + (doe as i64) - 719_468
}

#[cfg(test)]
mod tests;
