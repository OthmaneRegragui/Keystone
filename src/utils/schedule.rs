use chrono::{DateTime, Duration, NaiveTime, TimeZone, Utc};

/// Parse a daily `HH:MM` schedule (24-hour, UTC).
pub fn parse_schedule_time(value: &str) -> Result<NaiveTime, String> {
    let value = value.trim();
    let (hour, minute) = value
        .split_once(':')
        .ok_or_else(|| format!("schedule time '{value}' must be HH:MM"))?;
    if hour.len() != 2 || minute.len() != 2 {
        return Err(format!("schedule time '{value}' must be HH:MM"));
    }
    let hour: u32 = hour
        .parse()
        .map_err(|_| format!("schedule time '{value}' has an invalid hour"))?;
    let minute: u32 = minute
        .parse()
        .map_err(|_| format!("schedule time '{value}' has an invalid minute"))?;
    if hour > 23 || minute > 59 {
        return Err(format!(
            "schedule time '{value}' is out of range (00:00-23:59)"
        ));
    }
    NaiveTime::from_hms_opt(hour, minute, 0)
        .ok_or_else(|| format!("schedule time '{value}' is invalid"))
}

/// Compute the first occurrence of `schedule` strictly after `now` (UTC).
pub fn next_run_after(schedule: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let time = parse_schedule_time(schedule)?;
    let now_naive = now.naive_utc();
    let mut candidate = now_naive.date().and_time(time);
    if candidate <= now_naive {
        candidate = (now_naive.date() + Duration::days(1)).and_time(time);
    }
    Ok(Utc.from_utc_datetime(&candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 2, hour, minute, second)
            .unwrap()
    }

    #[test]
    fn parses_valid_times() {
        assert_eq!(parse_schedule_time("00:00").unwrap().hour(), 0);
        assert_eq!(parse_schedule_time("23:59").unwrap().minute(), 59);
        assert_eq!(parse_schedule_time(" 07:05 ").unwrap().hour(), 7);
    }

    #[test]
    fn rejects_invalid_times() {
        for value in ["", "12", "24:00", "12:60", "1:00", "12:0", "ab:cd"] {
            assert!(parse_schedule_time(value).is_err(), "{value} should be invalid");
        }
    }

    #[test]
    fn next_run_is_today_when_still_ahead() {
        let next = next_run_after("12:00", at(9, 30, 0)).unwrap();
        assert_eq!(next, at(12, 0, 0));
    }

    #[test]
    fn next_run_is_tomorrow_when_time_passed() {
        let next = next_run_after("09:00", at(9, 30, 0)).unwrap();
        assert_eq!(next, at(9, 0, 0) + Duration::days(1));
    }

    #[test]
    fn next_run_is_tomorrow_at_exact_time() {
        let next = next_run_after("12:00", at(12, 0, 0)).unwrap();
        assert_eq!(next, at(12, 0, 0) + Duration::days(1));
    }
}
