//! Integration tests for [`akku::id`] — UUID, ULID and NanoID.

use akku::id::{
    nanoid, nanoid_custom, nanoid_len, IdError, ParseError, Ulid, Uuid, DEFAULT_ALPHABET,
};

#[test]
fn uuid_v4_sets_version_and_variant_bits() {
    let id = Uuid::v4();
    assert_eq!(id.version(), 4);
    let bytes = id.as_bytes();
    assert_eq!(bytes[6] & 0xF0, 0x40, "version nibble");
    assert_eq!(bytes[8] & 0xC0, 0x80, "variant bits encode RFC 4122");
}

#[test]
fn uuid_v4_is_unique_across_many_generations() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..1000 {
        assert!(seen.insert(Uuid::v4()), "duplicate v4 UUID");
    }
    assert_eq!(seen.len(), 1000);
}

#[test]
fn uuid_v7_sets_version_and_variant_bits() {
    let id = Uuid::v7();
    assert_eq!(id.version(), 7);
    let bytes = id.as_bytes();
    assert_eq!(bytes[6] & 0xF0, 0x70, "version nibble");
    assert_eq!(bytes[8] & 0xC0, 0x80, "variant bits encode RFC 4122");
}

#[test]
fn uuid_v7_exposes_its_timestamp() {
    let before = now_millis();
    let id = Uuid::v7();
    let after = now_millis();

    let millis = id
        .timestamp_millis()
        .expect("v7 always carries a timestamp");
    assert!(
        millis >= before,
        "timestamp {millis} predates the call ({before})"
    );
    assert!(
        millis <= after + 1,
        "timestamp {millis} is in the future ({after})"
    );

    assert!(Uuid::v4().timestamp_millis().is_none());
}

#[test]
fn uuid_v7_is_lexicographically_ordered_across_consecutive_calls() {
    let ids: Vec<Uuid> = (0..64).map(|_| Uuid::v7()).collect();
    let sorted = {
        let mut copy = ids.clone();
        copy.sort();
        copy == ids
    };
    assert!(sorted, "consecutive v7 calls must increase");
    for window in ids.windows(2) {
        assert!(window[0] < window[1]);
    }
}

#[test]
fn uuid_v7_stays_strictly_monotonic_over_five_thousand_calls() {
    const COUNT: usize = 5_000;
    let mut previous: Option<Uuid> = None;
    for index in 0..COUNT {
        let id = Uuid::v7();
        if let Some(last) = previous {
            assert!(
                last < id,
                "call {index} of {COUNT} did not increase: {last} >= {id}"
            );
        }
        previous = Some(id);
    }
}

#[test]
fn uuid_v7_at_is_deterministic() {
    let rand = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA];
    let id = Uuid::v7_at(1_700_000_000_000, rand);
    assert_eq!(id, Uuid::v7_at(1_700_000_000_000, rand));
    assert_eq!(id.timestamp_millis(), Some(1_700_000_000_000));
    assert_eq!(id.version(), 7);
    assert_eq!(
        id.as_bytes()[9..],
        rand[3..10],
        "rand_b tail bytes are stored verbatim"
    );
}

#[test]
fn uuid_repr_conversions_round_trip() {
    assert!(Uuid::NIL.timestamp_millis().is_none());
    assert_eq!(
        Uuid::NIL.to_string(),
        "00000000-0000-0000-0000-000000000000"
    );
    assert_eq!(
        Uuid::MAX.to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff"
    );
    assert_eq!(Uuid::MAX.as_u128(), u128::MAX);

    let id = Uuid::v4();
    assert_eq!(Uuid::from_bytes(*id.as_bytes()), id);
    assert_eq!(Uuid::from_bytes(id.into_bytes()), id);
    assert_eq!(Uuid::from_u128(id.as_u128()), id);
    assert_eq!(id.to_simple().len(), 32);
    assert_eq!(format!("{id:?}"), id.to_hyphenated());
}

#[test]
fn uuid_parse_accepts_every_documented_format() {
    let hyphenated = "550e8400-e29b-41d4-a716-446655440000";
    let expected = Uuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000);

    assert_eq!(Uuid::parse(hyphenated), Ok(expected));
    assert_eq!(
        Uuid::parse("550E8400E29B41D4A716446655440000"),
        Ok(expected)
    );
    assert_eq!(
        Uuid::parse("{550E8400-E29B-41D4-A716-446655440000}"),
        Ok(expected)
    );
    assert_eq!(
        Uuid::parse("urn:uuid:550E8400-E29B-41D4-A716-446655440000"),
        Ok(expected)
    );
    assert_eq!(
        Uuid::parse("URN:UUID:{550e8400-e29b-41d4-a716-446655440000}"),
        Ok(expected)
    );
    assert_eq!(
        Uuid::parse("  550e8400-e29b-41d4-a716-446655440000  "),
        Ok(expected)
    );
    assert_eq!(expected.to_hyphenated(), hyphenated);
    assert_eq!(
        "550e8400-e29b-41d4-a716-446655440000".parse::<Uuid>(),
        Ok(expected)
    );

    let nil = "00000000-0000-0000-0000-000000000000";
    assert_eq!(Uuid::parse(nil), Ok(Uuid::NIL));
    assert!(
        Uuid::parse("urn:uuid:urn:uuid:550e8400-e29b-41d4-a716-446655440000").is_err(),
        "the urn prefix is stripped once",
    );
}

#[test]
fn uuid_parse_rejects_degenerate_input() {
    assert_eq!(Uuid::parse(""), Err(ParseError::InvalidLength { len: 0 }));
    assert_eq!(
        Uuid::parse("not-a-uuid"),
        Err(ParseError::InvalidLength { len: 10 })
    );
    assert!(Uuid::parse("550e8400-e29b-41d4-a716-44665544000g").is_err());
    assert!(Uuid::parse(
        "550e8400e29b41d4a716446655440000"
            .replace("5", "x")
            .as_str()
    )
    .is_err());
    match Uuid::parse("550e84g0-e29b-41d4-a716-446655440000") {
        Err(ParseError::InvalidChar { pos: 6, ch: 'g' }) => {}
        other => panic!("expected InvalidChar at 6, got {other:?}"),
    }
    match Uuid::parse("550e8400-e29b-41d4-a7164-46655440000") {
        Err(ParseError::InvalidChar { pos: 23, ch: '4' }) => {}
        other => panic!("expected misplaced hyphen, got {other:?}"),
    }
}

#[test]
fn uuid_parse_rejects_non_ascii_input_without_decoding_it() {
    // Same character count as a valid hyphenated UUID, but the last character
    // is a multi-byte one: the length check passes, the ASCII check trips.
    assert_eq!(
        Uuid::parse("550e8400-e29b-41d4-a716-44665544000😀"),
        Err(ParseError::InvalidChar {
            pos: 35, ch: '😀'
        })
    );
    assert_eq!(
        Uuid::parse("urn:uuid:é"),
        Err(ParseError::InvalidLength { len: 1 })
    );
    assert_eq!(Uuid::parse("{}"), Err(ParseError::InvalidLength { len: 0 }));
    assert_eq!(Uuid::parse("{"), Err(ParseError::InvalidLength { len: 1 }));
    assert_eq!(Uuid::parse("}"), Err(ParseError::InvalidLength { len: 1 }));
    assert_eq!(
        Uuid::parse("urn:uuid:"),
        Err(ParseError::InvalidLength { len: 0 })
    );
    assert_eq!(
        Uuid::parse(&"é".repeat(16)),
        Err(ParseError::InvalidLength { len: 16 })
    );
}

#[test]
fn uuid_parse_rejects_oversized_input_before_allocating() {
    assert_eq!(
        Uuid::parse(&"a".repeat(10_000)),
        Err(ParseError::InvalidLength { len: 10_000 })
    );
    assert_eq!(
        Uuid::parse(&format!("urn:uuid:{}", "a".repeat(10_000))),
        Err(ParseError::InvalidLength { len: 10_000 })
    );
    assert_eq!(
        Uuid::parse(&"😀".repeat(10_000)),
        Err(ParseError::InvalidLength { len: 10_000 })
    );
}

#[test]
fn ulid_round_trips_through_crockford_base32() {
    let ulid = Ulid::new();
    let text = ulid.to_string();
    assert_eq!(text.len(), 26);
    assert!(text.chars().all(|ch| !matches!(ch, 'I' | 'L' | 'O' | 'U')));
    assert_eq!(Ulid::parse(&text), Ok(ulid));
    assert_eq!(text.parse::<Ulid>(), Ok(ulid));

    assert_eq!(
        Ulid::from_parts(0, 0).to_string(),
        "00000000000000000000000000"
    );
    assert_eq!(
        Ulid::from_parts(0, 32).to_string(),
        "00000000000000000000000010"
    );
    assert_eq!(
        Ulid::from_parts(1, 0).to_string(),
        "00000000010000000000000000"
    );
}

#[test]
fn ulid_decode_tolerates_i_l_o_and_case() {
    assert_eq!(Ulid::parse("00"), Ok(Ulid::from_parts(0, 0)));
    assert_eq!(Ulid::parse("10"), Ok(Ulid::from_parts(0, 32)));
    assert_eq!(Ulid::parse("1O"), Ok(Ulid::from_parts(0, 32)));
    assert_eq!(Ulid::parse("IO"), Ok(Ulid::from_parts(0, 32)));
    assert_eq!(Ulid::parse("LO"), Ok(Ulid::from_parts(0, 32)));
    assert_eq!(Ulid::parse("lo"), Ok(Ulid::from_parts(0, 32)));
    assert_eq!(Ulid::parse("  7z  "), Ok(Ulid::from_parts(0, 255)));
    assert_eq!(Ulid::parse("7Z"), Ok(Ulid::from_parts(0, 255)));
}

#[test]
fn ulid_rejects_overflow_beyond_128_bits() {
    let max_text = "7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
    let max_ulid = Ulid::from_bytes([0xFF; 16]);
    assert_eq!(Ulid::parse(max_text), Ok(max_ulid));
    assert_eq!(max_ulid.to_string(), max_text);

    let overflow = "80000000000000000000000000";
    assert_eq!(Ulid::parse(overflow), Err(ParseError::Overflow));
    assert!(Ulid::parse("8000000000000000000000000Z").is_err());

    assert_eq!(
        Ulid::from_bytes([0xFF; 16]).timestamp_ms(),
        (1u128 << 48) as u64 - 1
    );
    assert_eq!(Ulid::parse(""), Err(ParseError::InvalidLength { len: 0 }));
    match Ulid::parse("000000000000000000000000000") {
        Err(ParseError::InvalidLength { len: 27 }) => {}
        other => panic!("expected InvalidLength 27, got {other:?}"),
    }
    match Ulid::parse("0000000000000000000000000U") {
        Err(ParseError::InvalidChar { pos: 25, ch: 'U' }) => {}
        other => panic!("expected InvalidChar for U, got {other:?}"),
    }
}

#[test]
fn ulid_parse_rejects_non_ascii_input_without_decoding_it() {
    assert_eq!(
        Ulid::parse("é"),
        Err(ParseError::InvalidChar { pos: 0, ch: 'é' })
    );
    assert_eq!(
        Ulid::parse("0😀"),
        Err(ParseError::InvalidChar { pos: 1, ch: '😀' })
    );
    assert_eq!(
        Ulid::parse(&"é".repeat(26)),
        Err(ParseError::InvalidChar { pos: 0, ch: 'é' })
    );
}

#[test]
fn ulid_parse_rejects_oversized_input_before_allocating() {
    assert_eq!(
        Ulid::parse(&"a".repeat(10_000)),
        Err(ParseError::InvalidLength { len: 10_000 })
    );
    assert_eq!(
        Ulid::parse(&"😀".repeat(10_000)),
        Err(ParseError::InvalidLength { len: 10_000 })
    );
    assert_eq!(
        Ulid::parse(&format!("  {}", "7".repeat(27))),
        Err(ParseError::InvalidLength { len: 27 })
    );
}

#[test]
fn ulid_consecutive_calls_are_monotonically_ordered() {
    let ids: Vec<Ulid> = (0..64).map(|_| Ulid::new()).collect();
    for window in ids.windows(2) {
        assert!(
            window[0] < window[1],
            "same-millisecond ULIDs must increment"
        );
    }
}

#[test]
fn ulid_parts_and_bytes_round_trip() {
    let ulid = Ulid::from_parts(0x0000_FFFF_FFFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF_FFFF);
    assert_eq!(ulid.timestamp_ms(), 0xFFFF_FFFF_FFFF);
    assert_eq!(ulid.random(), 0xFFFF_FFFF_FFFF_FFFF_FFFF);
    assert_eq!(Ulid::from_bytes(ulid.to_bytes()), ulid);

    // Timestamps beyond 48 bits are masked.
    assert_eq!(Ulid::from_parts(1 << 48, 0).timestamp_ms(), 0);
}

#[test]
fn nanoid_uses_the_default_alphabet_and_length() {
    let id = nanoid();
    assert_eq!(id.len(), 21);
    assert!(
        id.chars().all(|ch| DEFAULT_ALPHABET.contains(&(ch as u8))),
        "id: {id}"
    );
    assert_eq!(nanoid_len(0), "");
    for length in [1usize, 5, 21, 64, 300] {
        let id = nanoid_len(length);
        assert_eq!(id.chars().count(), length);
        assert!(id.chars().all(|ch| DEFAULT_ALPHABET.contains(&(ch as u8))));
    }

    let mut seen = std::collections::HashSet::new();
    for _ in 0..256 {
        assert!(seen.insert(nanoid()), "duplicate NanoID");
    }
}

#[test]
fn nanoid_custom_alphabets_are_validated() {
    let binary = nanoid_custom(16, &['0', '1']).unwrap();
    assert_eq!(binary.len(), 16);
    assert!(binary.chars().all(|ch| ch == '0' || ch == '1'));

    let single = nanoid_custom(5, &['x']).unwrap();
    assert_eq!(single, "xxxxx");

    let huge: Vec<char> = ('a'..).take(257).collect();
    assert_eq!(
        nanoid_custom(4, &huge),
        Err(IdError::AlphabetTooLarge { len: 257 })
    );
    assert_eq!(nanoid_custom(4, &[]), Err(IdError::EmptyAlphabet));
    assert!(format!("{}", IdError::EmptyAlphabet).contains("at least one symbol"));
    let err = nanoid_custom(4, &huge).unwrap_err();
    assert!(format!("{err}").contains("at most 256"));
}

/// Current Unix time in milliseconds, mirroring the crate helper.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_millis() as u64)
        .unwrap_or(0)
}
