use crate::{QueryVector, SCALE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    MissingField,
    InvalidValue,
    InvalidFormat,
}

use std::borrow::Cow;
use serde::Deserialize;

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

    // Fallback to serde path
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

// ─── Ordered Parse ───────────────────────────────────────────────────────────

fn try_parse_transaction_first(json: &[u8], out: &mut QueryVector) -> Result<(), ParseError> {
    let mut cursor: usize = 0;
    let mut known_hashes = [0u64; 64];
    let known_count: usize;

    // transaction.amount
    let amount = find_and_read_double(json, b"\"amount\"", &mut cursor)?;
    out[0] = quantize(amount / 10_000.0);

    // transaction.installments
    let installments = find_and_read_int(json, b"\"installments\"", &mut cursor)?;
    out[1] = quantize(installments as f64 / 12.0);

    // transaction.requested_at
    let requested_at = find_and_read_string(json, b"\"requested_at\"", &mut cursor)?;
    let parsed = parse_datetime(requested_at)?;
    let requested_minute = parsed.epoch_minute;
    out[3] = quantize(parsed.hour as f64 / 23.0);
    out[4] = quantize(parsed.day_of_week as f64 / 6.0);

    // customer.avg_amount
    let customer_avg_amount = find_and_read_double(json, b"\"avg_amount\"", &mut cursor)?;

    // customer.tx_count_24h
    let tx_count_24h = find_and_read_int(json, b"\"tx_count_24h\"", &mut cursor)?;
    out[8] = quantize(tx_count_24h as f64 / 20.0);

    // customer.known_merchants
    known_count = find_and_read_known_merchants(json, &mut cursor, &mut known_hashes)?;

    // merchant.id
    let merchant_id = find_and_read_string(json, b"\"id\"", &mut cursor)?;
    let merchant_hash = hash_bytes(merchant_id);

    // merchant.mcc
    let mcc = find_and_read_string(json, b"\"mcc\"", &mut cursor)?;
    out[12] = quantize(mcc_risk(parse_mcc(mcc)));

    // merchant.avg_amount
    let merchant_avg_amount = find_and_read_double(json, b"\"avg_amount\"", &mut cursor)?;
    out[13] = quantize(merchant_avg_amount / 10_000.0);

    // terminal.is_online
    let is_online = find_and_read_bool(json, b"\"is_online\"", &mut cursor)?;
    out[9] = if is_online { SCALE } else { 0 };

    // terminal.card_present
    let card_present = find_and_read_bool(json, b"\"card_present\"", &mut cursor)?;
    out[10] = if card_present { SCALE } else { 0 };

    // terminal.km_from_home
    let km_from_home = find_and_read_double(json, b"\"km_from_home\"", &mut cursor)?;
    out[7] = quantize(km_from_home / 1_000.0);

    // last_transaction
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

    // customer.avg_amount
    let customer_avg_amount = find_and_read_double(json, b"\"avg_amount\"", &mut cursor)?;

    // customer.tx_count_24h
    let tx_count_24h = find_and_read_int(json, b"\"tx_count_24h\"", &mut cursor)?;
    out[8] = quantize(tx_count_24h as f64 / 20.0);

    // customer.known_merchants
    let known_count = find_and_read_known_merchants(json, &mut cursor, &mut known_hashes)?;

    // last_transaction can appear before transaction in generated variants.
    let last_value = find_value_start(json, b"\"last_transaction\"", &mut cursor)?;
    let last_info = if last_value < json.len() && json[last_value] == b'n' {
        None
    } else {
        cursor = last_value;
        let last_timestamp = find_and_read_string(json, b"\"timestamp\"", &mut cursor)?;
        let last_km = find_and_read_double(json, b"\"km_from_current\"", &mut cursor)?;
        Some((last_timestamp, last_km))
    };

    // merchant.id
    let merchant_id = find_and_read_string(json, b"\"id\"", &mut cursor)?;
    let merchant_hash = hash_bytes(merchant_id);

    // merchant.mcc
    let mcc = find_and_read_string(json, b"\"mcc\"", &mut cursor)?;
    out[12] = quantize(mcc_risk(parse_mcc(mcc)));

    // merchant.avg_amount
    let merchant_avg_amount = find_and_read_double(json, b"\"avg_amount\"", &mut cursor)?;
    out[13] = quantize(merchant_avg_amount / 10_000.0);

    // terminal.is_online
    let is_online = find_and_read_bool(json, b"\"is_online\"", &mut cursor)?;
    out[9] = if is_online { SCALE } else { 0 };

    // terminal.card_present
    let card_present = find_and_read_bool(json, b"\"card_present\"", &mut cursor)?;
    out[10] = if card_present { SCALE } else { 0 };

    // terminal.km_from_home
    let km_from_home = find_and_read_double(json, b"\"km_from_home\"", &mut cursor)?;
    out[7] = quantize(km_from_home / 1_000.0);

    // transaction.amount
    let amount = find_and_read_double(json, b"\"amount\"", &mut cursor)?;
    out[0] = quantize(amount / 10_000.0);

    // transaction.installments
    let installments = find_and_read_int(json, b"\"installments\"", &mut cursor)?;
    out[1] = quantize(installments as f64 / 12.0);

    // transaction.requested_at
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

// ─── Serde Fallback Parse ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Payload<'a> {
    #[serde(borrow)]
    transaction: Transaction<'a>,
    #[serde(borrow)]
    customer: Customer<'a>,
    #[serde(borrow)]
    merchant: Merchant<'a>,
    terminal: Terminal,
    #[serde(default, borrow)]
    last_transaction: Option<LastTransaction<'a>>,
}

#[derive(Deserialize)]
struct Transaction<'a> {
    amount: f64,
    installments: i32,
    #[serde(borrow)]
    requested_at: Cow<'a, str>,
}

#[derive(Deserialize)]
struct Customer<'a> {
    avg_amount: f64,
    tx_count_24h: i32,
    #[serde(borrow)]
    known_merchants: Vec<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Merchant<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(borrow)]
    mcc: Cow<'a, str>,
    avg_amount: f64,
}

#[derive(Deserialize)]
struct Terminal {
    is_online: bool,
    card_present: bool,
    km_from_home: f64,
}

#[derive(Deserialize)]
struct LastTransaction<'a> {
    #[serde(borrow)]
    timestamp: Cow<'a, str>,
    km_from_current: f64,
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
    out[10] = if parsed.terminal.card_present { SCALE } else { 0 };
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

// ─── JSON Helpers ──────────────────────────────────────────────────────────

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

fn read_double(json: &[u8], name: &[u8]) -> Option<f64> {
    let start = find_value_start(json, name, &mut 0).ok()?;
    read_double_at(json, start).ok()
}

fn read_int(json: &[u8], name: &[u8]) -> Option<i32> {
    let start = find_value_start(json, name, &mut 0).ok()?;
    read_int_at(json, start).ok()
}

fn read_bool(json: &[u8], name: &[u8]) -> Option<bool> {
    let start = find_value_start(json, name, &mut 0).ok()?;
    read_bool_at(json, start).ok()
}

fn read_string<'a>(json: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let start = find_value_start(json, name, &mut 0).ok()?;
    read_string_at(json, start).ok()
}

fn read_known_merchants(json: &[u8], hashes: &mut [u64; 64]) -> Option<usize> {
    let start = find_value_start(json, b"\"known_merchants\"", &mut 0).ok()?;
    if start >= json.len() || json[start] != b'[' {
        return None;
    }
    let rel_end = json[start..].iter().position(|&b| b == b']')?;
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
            .position(|&b| b == b'"')?;
        if count < hashes.len() {
            hashes[count] = hash_bytes(&json[content_start..content_start + rel]);
            count += 1;
        }
        i = content_start + rel + 1;
    }
    Some(count)
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

fn find_object<'a>(json: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let start = find_value_start(json, name, &mut 0).ok()?;
    slice_object_at(json, start)
}

fn slice_object_at<'a>(json: &'a [u8], start: usize) -> Option<&'a [u8]> {
    if start >= json.len() || json[start] != b'{' {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..json.len() {
        let b = json[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        if b == b'"' {
            in_string = true;
            continue;
        }
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(&json[start..=i]);
            }
        }
    }
    None
}

fn is_json_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t')
}

// ─── Hash ──────────────────────────────────────────────────────────────────

fn hash_bytes(value: &[u8]) -> u64 {
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for &b in value {
        hash ^= b as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

// ─── MCC ───────────────────────────────────────────────────────────────────

fn parse_mcc(mcc: &[u8]) -> i32 {
    if mcc.len() != 4 {
        return 0;
    }
    let mut result = 0i32;
    for &b in mcc {
        if !b.is_ascii_digit() {
            return 0;
        }
        result = result * 10 + ((b - b'0') as i32);
    }
    result
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

// ─── Datetime ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct ParsedDateTime {
    epoch_minute: i64,
    hour: i32,
    day_of_week: i32, // Monday = 0
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
    let a = (s[offset] - b'0') as i32;
    let b = (s[offset + 1] - b'0') as i32;
    Ok(a * 10 + b)
}

fn parse4(s: &[u8], offset: usize) -> Result<i32, ParseError> {
    let a = parse2(s, offset)?;
    let b = parse2(s, offset + 2)?;
    Ok(a * 100 + b)
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
