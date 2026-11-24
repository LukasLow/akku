use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::thread;

use akku::time::clock::{Clock, Instant, MockClock, SystemClock};
use akku::time::datetime::{Date, DateTime, OffsetDateTime, Time, Weekday};
use akku::time::duration::Duration;
use akku::time::timezone::{TimeZone, UtcOffset};
use akku::time::{ParseError, TimeError};

#[test]
fn leap_years_and_days_in_month() {
    assert!(!Date::is_leap_year(1900));
    assert!(Date::is_leap_year(2000));
    assert!(Date::is_leap_year(2024));
    assert!(!Date::is_leap_year(2100));
    assert!(!Date::is_leap_year(2023));
    assert!(Date::is_leap_year(0));
    assert!(!Date::is_leap_year(-100));
    assert!(Date::is_leap_year(-400));

    assert_eq!(Date::days_in_month(2024, 2), 29);
    assert_eq!(Date::days_in_month(2023, 2), 28);
    assert_eq!(Date::days_in_month(1900, 2), 28);
    assert_eq!(Date::days_in_month(2000, 2), 29);
    assert_eq!(Date::days_in_month(2024, 4), 30);
    assert_eq!(Date::days_in_month(2024, 12), 31);
    assert_eq!(Date::days_in_month(2024, 0), 0);
    assert_eq!(Date::days_in_month(2024, 13), 0);

    assert!(Date::new(2024, 2, 29).is_ok());
    assert!(Date::new(2023, 2, 29).is_err());
    assert!(Date::new(1900, 2, 29).is_err());
    assert!(Date::new(2000, 2, 29).is_ok());
}

#[test]
fn hinnant_civil_days_round_trip_across_millennia_including_negative_years() {
    // Sweep every single day across >1200 years from -500-01-01 through +2500-12-31
    // in dense ranges around negative years, year 0, 1900, 1970, 2000, 2100, and 2500.
    let start_day = Date::new(-500, 1, 1).unwrap().to_unix_days();
    let end_day = Date::new(2500, 12, 31).unwrap().to_unix_days();
    assert!(end_day - start_day > 1_095_000);

    // Exhaustive day-by-day check across 1500 consecutive years (-500..=1000) + (1800..=2200).
    for range in [
        Date::new(-500, 1, 1).unwrap().to_unix_days()
            ..=Date::new(1000, 12, 31).unwrap().to_unix_days(),
        Date::new(1800, 1, 1).unwrap().to_unix_days()
            ..=Date::new(2200, 12, 31).unwrap().to_unix_days(),
    ] {
        let mut prev_date: Option<Date> = None;
        for day in range {
            let date = Date::from_unix_days(day).unwrap();
            assert_eq!(date.to_unix_days(), day);

            // Verify monotonic calendar progression and weekday cycling.
            if let Some(prev) = prev_date {
                assert!(date > prev);
                assert_eq!(prev.weekday().next(), date.weekday());
                assert_eq!(date.weekday().prev(), prev.weekday());
                assert_eq!(prev.add_days(1), Some(date));
                assert_eq!(date.add_days(-1), Some(prev));
            }
            prev_date = Some(date);
        }
    }

    // Spot-check anchor dates and weekdays.
    let epoch = Date::new(1970, 1, 1).unwrap();
    assert_eq!(epoch.to_unix_days(), 0);
    assert_eq!(epoch.weekday(), Weekday::Thu);
    assert_eq!(epoch.weekday().number_from_monday(), 4);

    let y0_leap = Date::new(0, 2, 29).unwrap();
    assert_eq!(y0_leap.ordinal(), 60);
    assert_eq!(
        Date::from_unix_days(y0_leap.to_unix_days()).unwrap(),
        y0_leap
    );
    assert_eq!(y0_leap.add_days(1), Some(Date::new(0, 3, 1).unwrap()));
    assert_eq!(
        Date::new(-1, 12, 31).unwrap().add_days(1),
        Some(Date::new(0, 1, 1).unwrap())
    );
    assert_eq!(Date::new(-1, 12, 31).unwrap().to_string(), "-0001-12-31");
    assert_eq!(
        "-0001-12-31".parse::<Date>().unwrap(),
        Date::new(-1, 12, 31).unwrap()
    );
}

#[test]
fn pre_1970_negative_timestamps_floor_division() {
    // 1969-12-31T23:59:59Z -> unix timestamp -1
    let just_before_epoch = OffsetDateTime::from_unix_timestamp(-1, 0).unwrap();
    assert_eq!(just_before_epoch.format_rfc3339(), "1969-12-31T23:59:59Z");
    assert_eq!(just_before_epoch.unix_timestamp(), -1);
    assert_eq!(just_before_epoch.unix_timestamp_nanos(), -1_000_000_000);

    // 1969-12-31T23:59:59.5Z -> -0.5s before epoch (-500_000_000 ns)
    let half_sec_before = OffsetDateTime::from_unix_timestamp(-1, 500_000_000).unwrap();
    assert_eq!(half_sec_before.format_rfc3339(), "1969-12-31T23:59:59.5Z");
    assert_eq!(half_sec_before.unix_timestamp(), -1);
    assert_eq!(half_sec_before.unix_timestamp_nanos(), -500_000_000);

    let epoch = OffsetDateTime::from_unix_timestamp(0, 0).unwrap();
    assert_eq!(
        epoch.checked_add(Duration::from_millis(-500)),
        Some(half_sec_before)
    );
    assert_eq!(
        epoch.duration_since(half_sec_before),
        Duration::from_millis(500)
    );

    // 1900-01-01T00:00:00Z -> -2_208_988_800
    let y1900: OffsetDateTime = "1900-01-01T00:00:00Z".parse().unwrap();
    assert_eq!(y1900.unix_timestamp(), -2_208_988_800);
    assert_eq!(
        OffsetDateTime::from_unix_timestamp(-2_208_988_800, 0).unwrap(),
        y1900
    );
    assert_eq!(y1900.date().weekday(), Weekday::Mon);

    // 0001-01-01T00:00:00Z -> -62_135_596_800
    let y0001: OffsetDateTime = "0001-01-01T00:00:00Z".parse().unwrap();
    assert_eq!(y0001.unix_timestamp(), -62_135_596_800);
    assert_eq!(
        OffsetDateTime::from_unix_timestamp(-62_135_596_800, 0).unwrap(),
        y0001
    );
    assert_eq!(y0001.date().weekday(), Weekday::Mon);
    assert_eq!(y0001.format_rfc3339(), "0001-01-01T00:00:00Z");
}

#[test]
fn rfc3339_parse_and_format_round_trip() {
    let cases = [
        ("2024-06-01T12:00:00Z", "2024-06-01T12:00:00Z", 0u32),
        (
            "2024-06-01T12:00:00.5Z",
            "2024-06-01T12:00:00.5Z",
            500_000_000,
        ),
        (
            "2024-06-01T12:00:00.123456789Z",
            "2024-06-01T12:00:00.123456789Z",
            123_456_789,
        ),
        (
            "2024-06-01T12:00:00.000000001+05:30",
            "2024-06-01T12:00:00.000000001+05:30",
            1,
        ),
        (
            "2024-02-29t23:59:59.25-04:00",
            "2024-02-29T23:59:59.25-04:00",
            250_000_000,
        ),
        ("2024-06-01 08:30:15z", "2024-06-01T08:30:15Z", 0),
        (
            "2024-06-01T08:30:15.500000+00:00",
            "2024-06-01T08:30:15.5Z",
            500_000_000,
        ),
    ];

    for (input, expected_formatted, expected_nanos) in cases {
        let parsed = OffsetDateTime::parse_rfc3339(input).unwrap();
        assert_eq!(parsed.time().nanosecond(), expected_nanos);
        let formatted = parsed.format_rfc3339();
        assert_eq!(formatted, expected_formatted);
        let reparsed: OffsetDateTime = formatted.parse().unwrap();
        assert_eq!(parsed, reparsed);
        assert_eq!(parsed.offset(), reparsed.offset());
    }

    // Invalid RFC 3339 inputs must fail cleanly.
    let invalid = [
        "",
        "2023-02-29T00:00:00Z",            // non-leap year Feb 29
        "2024-04-31T00:00:00Z",            // April has 30 days
        "2024-13-01T00:00:00Z",            // month 13
        "2024-01-01T24:00:00Z",            // hour 24
        "2024-01-01T12:60:00Z",            // minute 60
        "2024-01-01T12:00:60Z",            // second 60
        "2024-01-01T12:00:00",             // missing offset
        "2024-01-01T12:00:00.Z",           // empty fractional seconds
        "2024-01-01T12:00:00.1234567890Z", // >9 fractional digits
        "2024-01-01T12:00:00+24:00",       // invalid offset hour
        "2024-01-01T12:00:00+02:60",       // invalid offset minute
        "2024-01-01X12:00:00Z",            // bad separator
        "2024-01-01T12:00:00Ztrailing",    // trailing bytes
    ];
    for bad in invalid {
        assert!(
            OffsetDateTime::parse_rfc3339(bad).is_err(),
            "expected error for {bad:?}"
        );
    }
}

#[test]
fn offset_datetime_utc_based_eq_ord_and_hash() {
    let utc: OffsetDateTime = "2024-06-01T12:00:00.123Z".parse().unwrap();
    let cest: OffsetDateTime = "2024-06-01T14:00:00.123+02:00".parse().unwrap();
    let edt: OffsetDateTime = "2024-06-01T08:00:00.123-04:00".parse().unwrap();
    let next_day_nz: OffsetDateTime = "2024-06-02T00:00:00.123+12:00".parse().unwrap();
    let later_utc: OffsetDateTime = "2024-06-01T12:00:00.124Z".parse().unwrap();

    assert_eq!(utc, cest);
    assert_eq!(utc, edt);
    assert_eq!(utc, next_day_nz);
    assert_eq!(cest.to_utc().format_rfc3339(), "2024-06-01T12:00:00.123Z");
    assert_eq!(
        utc.to_offset(UtcOffset::from_hms(12, 0, 0).unwrap())
            .format_rfc3339(),
        "2024-06-02T00:00:00.123+12:00"
    );

    assert!(cest < later_utc);
    assert!(later_utc > edt);
    assert_eq!(cest.duration_since(edt), Duration::ZERO);
    assert_eq!(later_utc.duration_since(edt), Duration::from_millis(1));
    assert_eq!(edt.duration_since(later_utc), Duration::from_millis(-1));

    // Verify Hash consistency with Eq across different offsets.
    let hash_of = |dt: OffsetDateTime| {
        let mut h = DefaultHasher::new();
        dt.hash(&mut h);
        h.finish()
    };
    assert_eq!(hash_of(utc), hash_of(cest));
    assert_eq!(hash_of(utc), hash_of(edt));
    assert_eq!(hash_of(utc), hash_of(next_day_nz));

    let mut set = HashSet::new();
    set.insert(utc);
    set.insert(cest);
    set.insert(edt);
    set.insert(next_day_nz);
    assert_eq!(set.len(), 1);

    let mut map = HashMap::new();
    map.insert(cest, "meeting");
    assert_eq!(map.get(&edt), Some(&"meeting"));
}

#[test]
fn timezone_and_utc_offset_parsing() {
    assert_eq!(UtcOffset::UTC.to_string(), "Z");
    assert_eq!(UtcOffset::from_hms(5, 30, 0).unwrap().to_string(), "+05:30");
    assert_eq!(
        UtcOffset::from_hms(-5, -30, 0).unwrap().to_string(),
        "-05:30"
    );
    assert_eq!(
        UtcOffset::from_hms(0, -30, 0).unwrap().whole_seconds(),
        -1800
    );
    // Mixed signs rejected.
    assert_eq!(
        UtcOffset::from_hms(-5, 30, 0),
        Err(TimeError::InvalidOffset)
    );
    assert_eq!(UtcOffset::from_hms(24, 0, 0), Err(TimeError::InvalidOffset));
    assert_eq!(
        UtcOffset::from_whole_seconds(86_400),
        Err(TimeError::InvalidOffset)
    );

    assert_eq!("UTC".parse::<TimeZone>().unwrap(), TimeZone::Utc);
    assert_eq!("Z".parse::<TimeZone>().unwrap(), TimeZone::Utc);
    assert_eq!(
        "+02:00".parse::<TimeZone>().unwrap(),
        TimeZone::Fixed(UtcOffset::from_hms(2, 0, 0).unwrap())
    );
    assert_eq!(
        "-07:00".parse::<TimeZone>().unwrap(),
        TimeZone::Fixed(UtcOffset::from_hms(-7, 0, 0).unwrap())
    );
    // IANA Etc/GMT POSIX sign inversion.
    assert_eq!(
        "Etc/GMT+5".parse::<TimeZone>().unwrap().offset(),
        UtcOffset::from_hms(-5, 0, 0).unwrap()
    );
    assert_eq!(
        "Etc/GMT-9".parse::<TimeZone>().unwrap().offset(),
        UtcOffset::from_hms(9, 0, 0).unwrap()
    );
    assert_eq!("Etc/GMT+0".parse::<TimeZone>().unwrap(), TimeZone::Utc);
    assert!("Etc/GMT+13".parse::<TimeZone>().is_err());
    assert!("Etc/GMT-15".parse::<TimeZone>().is_err());
    assert!("Mars/Olympus".parse::<TimeZone>().is_err());
}

#[test]
fn duration_signed_arithmetic_overflow_and_human_format() {
    // Normalization & signed operations.
    let d1 = Duration::from_millis(1500);
    assert_eq!(d1.as_secs(), 1);
    assert_eq!(d1.subsec_nanos(), 500_000_000);
    assert_eq!(d1.as_millis(), 1500);
    assert_eq!(d1.as_nanos(), 1_500_000_000);

    let d2 = Duration::from_secs(2);
    let diff = d1 - d2;
    assert!(diff.is_negative());
    assert_eq!(diff.as_secs(), 0);
    assert_eq!(diff.subsec_nanos(), -500_000_000);
    assert_eq!(diff.abs(), Duration::from_millis(500));
    assert_eq!(-diff, Duration::from_millis(500));
    assert_eq!(diff * 3, Duration::from_millis(-1500));
    assert_eq!(3 * diff, Duration::from_millis(-1500));
    assert_eq!(
        Duration::from_millis(-1500) / 3,
        Duration::from_millis(-500)
    );
    assert_eq!(Duration::from_secs_f64(-1.5), Duration::from_millis(-1500));
    assert!((Duration::from_millis(-1500).as_secs_f64() - (-1.5)).abs() < 1e-12);

    // Checked overflow behavior.
    assert_eq!(Duration::MAX.checked_add(Duration::from_nanos(1)), None);
    assert_eq!(Duration::MIN.checked_sub(Duration::from_nanos(1)), None);
    assert_eq!(Duration::MAX.checked_mul(2), None);
    assert_eq!(Duration::from_secs(10).checked_div(0), None);
    assert_eq!(Duration::MIN.checked_neg(), None);

    // std::time::Duration conversions.
    let std_dur = std::time::Duration::new(42, 123_456_789);
    let akku_dur = Duration::try_from(std_dur).unwrap();
    assert_eq!(std::time::Duration::try_from(akku_dur).unwrap(), std_dur);
    assert_eq!(
        std::time::Duration::try_from(Duration::from_nanos(-1)),
        Err(TimeError::NegativeDuration)
    );
    assert_eq!(
        Duration::try_from(std::time::Duration::new(u64::MAX, 0)),
        Err(TimeError::Overflow)
    );

    // Human parse and Display round-trips.
    assert_eq!(Duration::ZERO.to_string(), "0s");
    assert_eq!(Duration::from_mins(90).to_string(), "1h30m");
    assert_eq!(Duration::from_millis(500).to_string(), "500ms");
    assert_eq!(Duration::from_secs(-2).to_string(), "-2s");

    let parsed = Duration::parse_human("1h30m500ms").unwrap();
    assert_eq!(parsed, Duration::from_millis(5_400_500));
    assert_eq!(parsed.to_string(), "1h30m500ms");

    let neg: Duration = "-250ms".parse().unwrap();
    assert_eq!(neg, Duration::from_millis(-250));
    assert_eq!(neg.to_string(), "-250ms");

    assert_eq!(
        Duration::parse_human("100µs").unwrap(),
        Duration::from_micros(100)
    );
    assert_eq!(
        Duration::parse_human("100us").unwrap(),
        Duration::from_micros(100)
    );
    assert_eq!(
        Duration::parse_human("2d3h4m5s6ms7us8ns")
            .unwrap()
            .to_string(),
        "2d3h4m5s6ms7us8ns"
    );
    assert_eq!(
        Duration::parse_human("1.5s").unwrap(),
        Duration::from_millis(1500)
    );

    assert_eq!(Duration::parse_human(""), Err(ParseError::Empty));
    assert!(Duration::parse_human("-").is_err());
    assert!(Duration::parse_human("123").is_err());
    assert!(Duration::parse_human("10lightyears").is_err());
    assert_eq!(
        Duration::parse_human("999999999999999999999999d"),
        Err(ParseError::Overflow)
    );
}

#[test]
fn mock_clock_advances_deterministically_across_threads() {
    let start: OffsetDateTime = "2024-01-01T00:00:00Z".parse().unwrap();
    let clock = MockClock::new(start);
    let t0: Instant = clock.monotonic();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let c = clock.clone();
            thread::spawn(move || {
                for _ in 0..100 {
                    c.advance(Duration::from_millis(10));
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    // 8 threads * 100 iterations * 10ms = 8000ms = 8s exactly.
    assert_eq!(t0.elapsed_from(&clock), Duration::from_secs(8));
    assert_eq!(clock.now().format_rfc3339(), "2024-01-01T00:00:08Z");

    // Wall-clock `set` changes `now()` without disturbing monotonic elapsed time.
    let jumped: OffsetDateTime = "2030-05-05T10:00:00Z".parse().unwrap();
    clock.set(jumped);
    assert_eq!(clock.now(), jumped);
    assert_eq!(t0.elapsed_from(&clock), Duration::from_secs(8));

    // Verify trait object compatibility and SystemClock sanity.
    let dyn_clock: &dyn Clock = &clock;
    assert_eq!(t0.elapsed_from(dyn_clock), Duration::from_secs(8));

    let sys = SystemClock::new();
    let s0 = sys.monotonic();
    let now = sys.now();
    assert!(now.date().year() >= 2024);
    assert!(s0.elapsed_from(&sys) >= Duration::ZERO);

    // DateTime & Time parsing sanity.
    let t: Time = "14:05:09.005".parse().unwrap();
    assert_eq!(t.to_string(), "14:05:09.005");
    let dt = DateTime::new(Date::new(2024, 6, 1).unwrap(), t);
    assert_eq!(dt.to_string(), "2024-06-01T14:05:09.005");
}

#[test]
fn non_ascii_inputs_fail_without_panicking() {
    assert!(OffsetDateTime::parse_rfc3339("2024-06-0中12:00:00Z").is_err());
    assert!(OffsetDateTime::parse_rfc3339("2024-06-01T12:00:0中Z").is_err());
    assert!("00:00:0中".parse::<Time>().is_err());
    assert!("2024-06-01中".parse::<Date>().is_err());
    assert!("+02:0中".parse::<UtcOffset>().is_err());
}

#[test]
fn checked_to_offset_handles_year_extremes() {
    let max_date = Date::new(i32::MAX, 12, 31).unwrap();
    let max_instant = OffsetDateTime::new(
        DateTime::new(max_date, Time::new(23, 59, 59, 0).unwrap()),
        UtcOffset::UTC,
    );
    assert_eq!(
        max_instant.checked_to_offset(UtcOffset::from_hms(1, 0, 0).unwrap()),
        Err(TimeError::OutOfRange)
    );
    let shifted_west = max_instant
        .checked_to_offset(UtcOffset::from_hms(-1, 0, 0).unwrap())
        .unwrap();
    assert_eq!(shifted_west, max_instant);
    assert_eq!(shifted_west.date(), max_date);
}
