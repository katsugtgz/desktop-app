//! Port of the subset of the `ms` npm package used by `selectors.ts`
//! (`ms(duration)`), `sagas.ts` (sleep length) and the snooze panel strings.
//!
//! `ms` accepts a leading signed number, an optional space, and one of its
//! short/long unit names (`ms`, `s`, `m`, `h`, `d`, `w`, `y` and their
//! plurals). A missing unit means milliseconds. Anything else is undefined.

/// Parses an `ms` duration string into milliseconds.
///
/// Returns `None` for the same inputs where the JS package returns
/// `undefined` (empty, over 100 characters, bad number, unknown unit, or a
/// negative/overflowing result).
pub fn parse_duration_ms(input: &str) -> Option<u64> {
    // `ms` bails on non-strings and strings longer than 100 chars.
    if input.is_empty() || input.len() > 100 {
        return None;
    }

    let (number, unit) = split_number_and_unit(input.trim())?;
    let value: f64 = number.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }

    let factor = match unit {
        "" | "ms" | "msec" | "msecs" | "millisecond" | "milliseconds" => 1.0,
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000.0,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000.0,
        "d" | "day" | "days" => 86_400_000.0,
        "w" | "week" | "weeks" => 604_800_000.0,
        // `ms` uses 365.25 days for a year.
        "y" | "yr" | "yrs" | "year" | "years" => 31_557_600_000.0,
        _ => return None,
    };

    let ms = (value * factor).round();
    if ms < 0.0 || ms > u64::MAX as f64 {
        return None;
    }
    Some(ms as u64)
}

/// Splits `21min` into (`21`, `min`), accepting a fraction (`1.5h`).
fn split_number_and_unit(input: &str) -> Option<(&str, &str)> {
    let unit_start = input
        .char_indices()
        .find(|(_, c)| c.is_ascii_alphabetic())
        .map(|(index, _)| index);
    let (number, unit) = match unit_start {
        Some(index) => (&input[..index], input[index..].trim()),
        None => (input, ""),
    };

    let number = number.trim();
    let digits = number.trim_start_matches(['+', '-']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    if unit.chars().any(|c| !c.is_ascii_alphabetic()) {
        return None;
    }
    Some((number, unit))
}

#[cfg(test)]
mod tests {
    use super::parse_duration_ms;

    #[test]
    fn parses_the_ms_unit_names() {
        assert_eq!(parse_duration_ms("250ms"), Some(250));
        assert_eq!(parse_duration_ms("2 msecs"), Some(2));
        assert_eq!(parse_duration_ms("90s"), Some(90_000));
        assert_eq!(parse_duration_ms("30m"), Some(1_800_000));
        assert_eq!(parse_duration_ms("21min"), Some(1_260_000));
        assert_eq!(parse_duration_ms("2 hours"), Some(7_200_000));
        assert_eq!(parse_duration_ms("1d"), Some(86_400_000));
        assert_eq!(parse_duration_ms("1w"), Some(604_800_000));
        assert_eq!(parse_duration_ms("1y"), Some(31_557_600_000));
        assert_eq!(parse_duration_ms("500"), Some(500));
        assert_eq!(parse_duration_ms("1.5h"), Some(5_400_000));
    }

    #[test]
    fn rejects_what_ms_rejects() {
        assert_eq!(parse_duration_ms(""), None);
        assert_eq!(parse_duration_ms("soon"), None);
        assert_eq!(parse_duration_ms("minutes"), None);
        assert_eq!(parse_duration_ms("-1m"), None);
        assert_eq!(parse_duration_ms("1m30s"), None);
    }
}
