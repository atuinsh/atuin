#[cfg(all(test, feature = "wasmbind"))]
mod test {
    extern crate chrono;
    extern crate wasm_bindgen_test;

    use self::chrono::prelude::*;
    use self::wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn now() {
        let utc: DateTime<Utc> = Utc::now();
        let local: DateTime<Local> = Local::now();

        // Ensure time set by the test script is correct
        let now = env!("NOW");
        let actual = Utc.datetime_from_str(&now, "%s").unwrap();
        let diff = utc - actual;
        assert!(
            diff < chrono::Duration::minutes(5),
            "expected {} - {} == {} < 5m (env var: {})",
            utc,
            actual,
            diff,
            now,
        );

        let tz = env!("TZ");
        eprintln!("testing with tz={}", tz);

        // Ensure offset retrieved when getting local time is correct
        let expected_offset = match tz {
            "ACST-9:30" => FixedOffset::east(19 * 30 * 60),
            "Asia/Katmandu" => FixedOffset::east(23 * 15 * 60), // No DST thankfully
            "EDT" | "EST4" | "-0400" => FixedOffset::east(-4 * 60 * 60),
            "EST" | "-0500" => FixedOffset::east(-5 * 60 * 60),
            "UTC0" | "+0000" => FixedOffset::east(0),
            tz => panic!("unexpected TZ {}", tz),
        };
        assert_eq!(
            &expected_offset,
            local.offset(),
            "expected: {:?} local: {:?}",
            expected_offset,
            local.offset(),
        );
    }

    #[wasm_bindgen_test]
    fn from_is_exact() {
        let now = js_sys::Date::new_0();

        let dt = DateTime::<Utc>::from(now.clone());

        assert_eq!(now.get_time() as i64, dt.timestamp_millis());
    }

    #[wasm_bindgen_test]
    fn local_from_local_datetime() {
        let now = Local::now();
        let ndt = now.naive_local();
        let res = match Local.from_local_datetime(&ndt).single() {
            Some(v) => v,
            None => panic! {"Required for test!"},
        };
        assert_eq!(now, res);
    }
}
