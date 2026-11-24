use akku::time::Duration;
use akku::units::{
    Energy, Length, Mass, Pressure, Quantity, Speed, TempDelta, Temperature, TimeSpan, UnitError,
};

fn approx_eq(a: f64, b: f64, eps: f64) {
    assert!(
        (a - b).abs() <= eps,
        "expected {a} ≈ {b} (diff = {}, eps = {eps})",
        (a - b).abs()
    );
}

#[test]
fn length_mass_speed_exact_si_conversions() {
    // Length
    assert_eq!(Length::from_kilometers(1.5).as_meters(), 1500.0);
    approx_eq(Length::from_centimeters(250.0).as_meters(), 2.5, 1e-12);
    approx_eq(Length::from_millimeters(1000.0).as_meters(), 1.0, 1e-12);
    approx_eq(
        Length::from_micrometers(1_000_000.0).as_meters(),
        1.0,
        1e-12,
    );
    approx_eq(
        Length::from_nanometers(1_000_000_000.0).as_meters(),
        1.0,
        1e-12,
    );
    approx_eq(Length::from_inches(12.0).as_feet(), 1.0, 1e-12);
    approx_eq(Length::from_yards(1.0).as_feet(), 3.0, 1e-12);
    approx_eq(Length::from_miles(1.0).as_meters(), 1_609.344, 1e-12);
    approx_eq(Length::from_nautical_miles(1.0).as_meters(), 1_852.0, 1e-12);
    assert_eq!(Length::unit_symbol(), "m");

    // Mass
    assert_eq!(Mass::from_tonnes(1.0).as_kilograms(), 1_000.0);
    approx_eq(Mass::from_grams(500.0).as_kilograms(), 0.5, 1e-12);
    approx_eq(
        Mass::from_milligrams(1_000_000.0).as_kilograms(),
        1.0,
        1e-12,
    );
    approx_eq(Mass::from_micrograms(1e9).as_kilograms(), 1.0, 1e-12);
    approx_eq(Mass::from_pounds(1.0).as_ounces(), 16.0, 1e-12);
    approx_eq(Mass::from_pounds(1.0).as_kilograms(), 0.453_592_37, 1e-12);
    assert_eq!(Mass::unit_symbol(), "kg");

    // Speed
    approx_eq(Speed::from_kph(36.0).as_mps(), 10.0, 1e-12);
    approx_eq(Speed::from_mph(60.0).as_mps(), 26.8224, 1e-12);
    approx_eq(Speed::from_knots(1.0).as_kph(), 1.852, 1e-12);
    approx_eq(Speed::from_mach_sea_level(1.0).as_mps(), 340.29, 1e-12);
    assert_eq!(Speed::unit_symbol(), "m/s");
}

#[test]
fn dimensional_and_linear_operators() {
    let dist = Length::from_kilometers(120.0);
    let time = TimeSpan::from_hours(1.5);
    let speed: Speed = dist / time;
    approx_eq(speed.as_kph(), 80.0, 1e-9);

    let back_dist: Length = speed * time;
    approx_eq(back_dist.as_kilometers(), 120.0, 1e-9);

    let back_dist_comm: Length = time * speed;
    approx_eq(back_dist_comm.as_kilometers(), 120.0, 1e-9);

    let back_time: TimeSpan = dist / speed;
    approx_eq(back_time.as_hours(), 1.5, 1e-9);

    // Linear ops & dimensionless ratio.
    let a = Length::from_meters(10.0);
    let b = Length::from_meters(2.5);
    assert_eq!(a + b, Length::from_meters(12.5));
    assert_eq!(a - b, Length::from_meters(7.5));
    assert_eq!(-b, Length::from_meters(-2.5));
    assert_eq!(a * 2.0, Length::from_meters(20.0));
    assert_eq!(2.0 * a, Length::from_meters(20.0));
    assert_eq!(a / 2.0, Length::from_meters(5.0));
    assert_eq!(a / b, 4.0);
    assert_eq!(a.to_string(), "10 m");

    // Generic Quantity trait check.
    fn double_si<Q: Quantity>(q: Q) -> Q {
        Q::from_si(q.si_value() * 2.0)
    }
    assert_eq!(
        double_si(Length::from_meters(3.0)),
        Length::from_meters(6.0)
    );
}

#[test]
fn temperature_conversions_affine_math_and_absolute_zero_guard() {
    // -40°C == -40°F
    let minus_40_c = Temperature::try_from_celsius(-40.0).unwrap();
    let minus_40_f = Temperature::try_from_fahrenheit(-40.0).unwrap();
    approx_eq(minus_40_c.as_kelvin(), minus_40_f.as_kelvin(), 1e-12);
    approx_eq(minus_40_c.as_fahrenheit(), -40.0, 1e-12);
    approx_eq(minus_40_f.as_celsius(), -40.0, 1e-12);

    // 0°C == 273.15 K == 32°F
    let freezing = Temperature::try_from_celsius(0.0).unwrap();
    approx_eq(freezing.as_kelvin(), 273.15, 1e-12);
    approx_eq(freezing.as_fahrenheit(), 32.0, 1e-12);

    // 100°C == 212°F == 373.15 K
    let boiling = Temperature::try_from_celsius(100.0).unwrap();
    approx_eq(boiling.as_fahrenheit(), 212.0, 1e-12);
    approx_eq(boiling.as_kelvin(), 373.15, 1e-12);

    // Exact absolute zero is accepted in all three scales.
    assert_eq!(Temperature::try_from_kelvin(0.0).unwrap().as_kelvin(), 0.0);
    assert_eq!(
        Temperature::try_from_celsius(-273.15).unwrap().as_kelvin(),
        0.0
    );
    assert_eq!(
        Temperature::try_from_fahrenheit(-459.67)
            .unwrap()
            .as_kelvin(),
        0.0
    );

    // Below absolute zero and non-finite inputs are rejected.
    assert!(matches!(
        Temperature::try_from_kelvin(-0.001),
        Err(UnitError::BelowAbsoluteZero { .. })
    ));
    assert!(matches!(
        Temperature::try_from_celsius(-273.16),
        Err(UnitError::BelowAbsoluteZero { .. })
    ));
    assert!(matches!(
        Temperature::try_from_fahrenheit(-459.68),
        Err(UnitError::BelowAbsoluteZero { .. })
    ));
    assert_eq!(
        Temperature::try_from_kelvin(f64::NAN),
        Err(UnitError::NotFinite)
    );
    assert_eq!(
        Temperature::try_from_kelvin(f64::INFINITY),
        Err(UnitError::NotFinite)
    );

    // Temperature - Temperature == TempDelta (Δ10°C == Δ18°F)
    let t1 = Temperature::try_from_celsius(20.0).unwrap();
    let t2 = Temperature::try_from_celsius(30.0).unwrap();
    let delta: TempDelta = t2 - t1;
    approx_eq(delta.as_celsius(), 10.0, 1e-12);
    approx_eq(delta.as_kelvin(), 10.0, 1e-12);
    approx_eq(delta.as_fahrenheit(), 18.0, 1e-12);

    let same_delta = TempDelta::from_fahrenheit(18.0);
    approx_eq(same_delta.as_celsius(), 10.0, 1e-12);
    approx_eq((t1 + same_delta).as_celsius(), 30.0, 1e-12);
    approx_eq((t2 - same_delta).as_celsius(), 20.0, 1e-12);

    // Checked subtraction below absolute zero returns Err.
    assert!(matches!(
        Temperature::from_kelvin(5.0).checked_sub(TempDelta::from_kelvin(10.0)),
        Err(UnitError::BelowAbsoluteZero { .. })
    ));
}

#[test]
fn timespan_lossless_round_trip_with_duration_down_to_1ns() {
    let one_ns = Duration::from_nanos(1);
    let span_one_ns = TimeSpan::from(one_ns);
    assert_eq!(span_one_ns.to_duration(), Some(one_ns));

    // A large duration plus 1 nanosecond cannot be represented losslessly in f64,
    // yet TimeSpan::from(Duration) preserves it down to the exact nanosecond.
    let huge_plus_1ns = Duration::new(10_000_000_000, 1);
    let span_huge = TimeSpan::from(huge_plus_1ns);
    assert_eq!(span_huge.to_duration(), Some(huge_plus_1ns));

    // Exact arithmetic preserves exactness when both operands are Exact.
    let combined = span_huge + span_one_ns;
    assert_eq!(
        combined.to_duration(),
        Some(Duration::new(10_000_000_000, 2))
    );
    let subtracted = combined - span_one_ns;
    assert_eq!(subtracted.to_duration(), Some(huge_plus_1ns));

    // Negative exact duration round-trip.
    let neg_ns = Duration::new(-5, -1);
    assert_eq!(TimeSpan::from(neg_ns).to_duration(), Some(neg_ns));

    // Float-backed TimeSpan conversion.
    let float_span = TimeSpan::from_millis(1250.0);
    assert_eq!(float_span.to_duration(), Some(Duration::from_millis(1250)));
    assert_eq!(TimeSpan::from_secs(f64::INFINITY).to_duration(), None);
}

#[test]
fn energy_conversions_and_physics_constructors() {
    approx_eq(Energy::from_kilojoules(1.0).as_joules(), 1_000.0, 1e-12);
    approx_eq(Energy::from_megajoules(1.0).as_kilojoules(), 1_000.0, 1e-12);
    approx_eq(Energy::from_calories(1.0).as_joules(), 4.184, 1e-12);
    approx_eq(Energy::from_kilocalories(1.0).as_calories(), 1_000.0, 1e-12);
    approx_eq(Energy::from_watt_hours(1.0).as_joules(), 3_600.0, 1e-12);
    approx_eq(Energy::from_kilowatt_hours(1.0).as_megajoules(), 3.6, 1e-12);
    approx_eq(
        Energy::from_electronvolts(1.0).as_joules(),
        1.602_176_634e-19,
        1e-30,
    );

    // Kinetic energy: 0.5 * 2 kg * (10 m/s)^2 = 100 J
    let ke = Energy::kinetic(Mass::from_kilograms(2.0), Speed::from_mps(10.0));
    approx_eq(ke.as_joules(), 100.0, 1e-12);

    // Rest mass-energy: 1 kg * c^2 ≈ 8.987551787368176e16 J
    let mc2 = Energy::mass_energy(Mass::from_kilograms(1.0));
    approx_eq(mc2.as_joules(), 299_792_458.0 * 299_792_458.0, 1.0);

    // Gravitational potential energy: 10 kg * 9.81 m/s^2 * 5 m = 490.5 J
    let pe = Energy::potential(Mass::from_kilograms(10.0), Length::from_meters(5.0), 9.81);
    approx_eq(pe.as_joules(), 490.5, 1e-12);
}

#[test]
fn pressure_conversions() {
    let one_atm = Pressure::from_atm(1.0);
    approx_eq(one_atm.as_pascals(), 101_325.0, 1e-12);
    approx_eq(one_atm.as_kilopascals(), 101.325, 1e-12);
    approx_eq(one_atm.as_megapascals(), 0.101_325, 1e-12);
    approx_eq(one_atm.as_bar(), 1.01325, 1e-12);
    approx_eq(one_atm.as_millibar(), 1_013.25, 1e-12);
    approx_eq(one_atm.as_psi(), 14.695_948_775_5, 1e-6);
    approx_eq(one_atm.as_mmhg(), 760.0, 1e-3);

    let one_bar = Pressure::from_bar(1.0);
    approx_eq(one_bar.as_pascals(), 100_000.0, 1e-12);
    approx_eq(Pressure::from_psi(14.695_948_775_5).as_atm(), 1.0, 1e-6);
    assert_eq!(Pressure::unit_symbol(), "Pa");
}
